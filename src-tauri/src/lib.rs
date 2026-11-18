//! `aitools-desktop` — Tauri v2 host shell (`SPEC-001`, `SPEC-006`, `SPEC-014`, `SPEC-016`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use aitools_ai::context_fusion::{suggest_zero_query_actions, ContextEnvelope, QuickAgentAction};
use aitools_native::app_catalog::AppCatalog;
use aitools_native::context::{
    capture_live_pre_focus_context, ClipboardProvider, ContextChip, SystemClipboard,
};
use aitools_native::vibrancy::apply_hud_vibrancy;
use aitools_telemetry::{TelemetryConfig, TelemetryHandle};
use anyhow::Context;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime, WindowEvent};
use tauri_plugin_aitools::{ensure_host_webview, AitoolsState};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tracing::info_span;

static WAKE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Payload streamed to the frontend over `tauri::ipc::Channel` on global wake (`SPEC-006-R4`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakePayload {
    /// Correlation trace identifier for the wake event.
    pub trace_id: String,
    /// Pre-focus context chips captured before focusing the main window.
    pub chips: Vec<ContextChip>,
    /// Zero-query agent action suggestions derived from the captured context.
    pub suggestions: Vec<QuickAgentAction>,
}

/// Startup indexing counts reported after loading bundled plugins, user ZTools directory, and OS apps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupIndexSummary {
    /// Number of desktop applications registered from [`AppCatalog`].
    pub apps_indexed: usize,
    /// Total number of plugins indexed from built-in and user ZTools directories.
    pub plugins_indexed: usize,
    /// Total searchable features indexed across plugins.
    pub plugin_features_indexed: usize,
}

#[derive(Default)]
struct WakeChannelHolder {
    channel: Mutex<Option<Channel<WakePayload>>>,
}

#[tauri::command]
fn register_wake_channel<R: Runtime>(
    channel: Channel<WakePayload>,
    webview: tauri::Webview<R>,
    holder: tauri::State<'_, WakeChannelHolder>,
) -> Result<(), String> {
    ensure_host_webview(webview.label())?;
    let mut guard = holder
        .channel
        .lock()
        .map_err(|_| "wake channel lock poisoned".to_string())?;
    *guard = Some(channel);
    Ok(())
}

#[tauri::command]
fn hide_main_window<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
) -> Result<(), String> {
    ensure_host_webview(webview.label())?;
    if let Some(win) = app.get_webview_window("main") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn set_focus_hide_suppressed<R: Runtime>(
    suppressed: bool,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> Result<(), String> {
    ensure_host_webview(webview.label())?;
    state.set_focus_hide_suppressed(suppressed);
    Ok(())
}

/// Resolves the bundled `plugins/builtin` root from resource paths or dev workspace fallback (`SPEC-016-R1`).
#[must_use]
pub fn resolve_builtin_plugins_dir(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(res) = resource_dir {
        candidates.push(res.join("plugins").join("builtin"));
        candidates.push(res.join("_up_").join("plugins").join("builtin"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins")
            .join("builtin"),
    );
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("plugins").join("builtin"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// Populates [`AitoolsState`] from `plugins/builtin/*`, optional `~/.aitools/plugins/ztools`, and [`AppCatalog`] (`SPEC-016-R1`).
pub fn index_startup_sources(
    state: &AitoolsState,
    builtin_dir: Option<&Path>,
    ztools_dir: Option<&Path>,
) -> StartupIndexSummary {
    let mut summary = StartupIndexSummary::default();
    if let Some(dir) = builtin_dir.filter(|d| d.is_dir()) {
        let res = state.sessions.sync_plugins_directory(dir);
        summary.plugins_indexed += res.indexed_plugins;
        summary.plugin_features_indexed += res.indexed_features;
    }
    if let Some(dir) = ztools_dir.filter(|d| d.is_dir()) {
        let res = state.sessions.sync_plugins_directory(dir);
        summary.plugins_indexed += res.indexed_plugins;
        summary.plugin_features_indexed += res.indexed_features;
    }
    if let Ok(mut catalog) = AppCatalog::default_platform() {
        if let Ok(apps) = catalog.snapshot(std::time::Instant::now()) {
            summary.apps_indexed = apps.len();
            let _ = state.sessions.register_apps(apps.to_vec());
        }
    }
    summary
}

/// Captures live frontmost app/selection/clipboard context before focusing the launcher (`SPEC-016-R3`).
#[must_use]
pub fn capture_wake_context(
    trace_id: &str,
    clipboard: &dyn ClipboardProvider,
    state: Option<&AitoolsState>,
) -> (WakePayload, ContextEnvelope) {
    let ctx = capture_live_pre_focus_context(trace_id, clipboard);
    let chips = ctx.to_chips();
    let envelope = ContextEnvelope {
        trace_id: trace_id.to_string(),
        app_name: ctx.app_name,
        bundle_id: ctx.bundle_id,
        window_title: ctx.window_title,
        selected_text: ctx.selected_text,
        clipboard_preview: ctx.clipboard_preview,
        active_path: ctx.active_path,
        image_handle: ctx.image_handle,
        captured_at_ms: Some(ctx.captured_at_ms),
    };
    let suggestions = suggest_zero_query_actions(&envelope);
    if let Some(st) = state {
        if let Ok(mut guard) = st.latest_context.lock() {
            *guard = Some(envelope.clone());
        }
    }
    let payload = WakePayload {
        trace_id: trace_id.to_string(),
        chips,
        suggestions,
    };
    (payload, envelope)
}

/// Returns `true` when `main` focus loss should hide the launcher window (`SPEC-016-R3`).
#[must_use]
pub fn should_auto_hide_on_focus_loss(state: Option<&AitoolsState>) -> bool {
    !state.is_some_and(AitoolsState::is_focus_hide_suppressed)
}

fn handle_wake_hotkey<R: Runtime>(app: &AppHandle<R>) {
    let seq = WAKE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let trace_id = format!("wake-{seq}");
    let _span = info_span!("wake_to_paint", trace_id = %trace_id).entered();

    // SPEC-006-R4 / SPEC-016-R3: capture live frontmost context before focusing main window.
    let clipboard = SystemClipboard;
    let state_ref = app.try_state::<AitoolsState>();
    let (payload, _env) = capture_wake_context(&trace_id, &clipboard, state_ref.as_deref());

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }

    if let Some(holder) = app.try_state::<WakeChannelHolder>() {
        if let Ok(guard) = holder.channel.lock() {
            if let Some(ref ch) = *guard {
                let _ = ch.send(payload.clone());
            }
        }
    }
    let _ = app.emit("aitools://wake", payload);
}

fn handle_dsh_hotkey<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
    let _ = app.emit("aitools://toggle-dsh", ());
}

/// Configures and launches the Tauri v2 desktop application.
///
/// # Errors
///
/// Returns an error if initializing [`AitoolsState`] or starting the Tauri runtime fails.
pub fn run() -> anyhow::Result<()> {
    let wake_shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    let dsh_shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyD);

    let app = tauri::Builder::default()
        .manage(WakeChannelHolder::default())
        .plugin(tauri_plugin_aitools::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if shortcut == &wake_shortcut {
                        handle_wake_hotkey(app);
                    } else if shortcut == &dsh_shortcut {
                        handle_dsh_hotkey(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            register_wake_channel,
            hide_main_window,
            set_focus_hide_suppressed,
        ])
        .setup(move |app| {
            // SPEC-016-R1: persist logs & LMDB under platform `app_data_dir()` instead of temp_dir.
            let app_data = app
                .path()
                .app_data_dir()
                .context("failed to resolve platform app_data_dir")?;
            let log_dir = app_data.join("logs");
            let db_dir = app_data.join("lmdb");
            std::fs::create_dir_all(&log_dir)?;
            std::fs::create_dir_all(&db_dir)?;

            // SPEC-000: initialize aitools-telemetry first.
            let telemetry: Option<std::sync::Arc<TelemetryHandle>> =
                aitools_telemetry::init(TelemetryConfig::new(log_dir)).ok();

            let state = AitoolsState::open(&db_dir, telemetry, &[]).with_context(|| {
                format!("failed to initialize AitoolsState at {}", db_dir.display())
            })?;
            state.attach_app_handle(app.handle().clone());

            let resource_dir = app.path().resource_dir().ok();
            let builtin_dir = resolve_builtin_plugins_dir(resource_dir.as_deref());
            let ztools_dir = app
                .path()
                .home_dir()
                .ok()
                .map(|home| home.join(".aitools").join("plugins").join("ztools"));
            let _summary =
                index_startup_sources(&state, builtin_dir.as_deref(), ztools_dir.as_deref());
            app.manage(state);

            if let Some(main_win) = app.get_webview_window("main") {
                if let Err(err) = apply_hud_vibrancy(&main_win) {
                    tracing::warn!(error = %err, "HUD vibrancy unavailable on this host");
                }
                let app_handle = app.handle().clone();
                main_win.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        let st = app_handle.try_state::<AitoolsState>();
                        if should_auto_hide_on_focus_loss(st.as_deref()) {
                            if let Some(win) = app_handle.get_webview_window("main") {
                                let _ = win.hide();
                            }
                        }
                    }
                });
            }

            let show_item = MenuItemBuilder::with_id("tray-show", "Show Launcher").build(app)?;
            let dsh_item =
                MenuItemBuilder::with_id("tray-dsh", "Developer Harness (DSH)").build(app)?;
            let quit_item = MenuItemBuilder::with_id("tray-quit", "Quit aitools").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .items(&[&show_item, &dsh_item, &quit_item])
                .build()?;
            let mut tray_builder = TrayIconBuilder::with_id("aitools-tray")
                .menu(&tray_menu)
                .tooltip("aitools")
                .on_menu_event(|app_handle, event| match event.id().as_ref() {
                    "tray-show" => handle_wake_hotkey(app_handle),
                    "tray-dsh" => handle_dsh_hotkey(app_handle),
                    "tray-quit" => {
                        if let Some(st) = app_handle.try_state::<AitoolsState>() {
                            st.sessions.shutdown_all();
                        }
                        app_handle.exit(0);
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let _tray = tray_builder.build(app)?;

            if let Err(err) = app.global_shortcut().register(wake_shortcut) {
                tracing::warn!(error = %err, "failed to register wake shortcut");
            }
            if let Err(err) = app.global_shortcut().register(dsh_shortcut) {
                tracing::warn!(error = %err, "failed to register DSH shortcut");
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("error while building aitools desktop shell")?;

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(st) = app_handle.try_state::<AitoolsState>() {
                st.sessions.shutdown_all();
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn spec_006_r1_and_r2_capabilities_isolate_plugins() {
        let main_cap: Value =
            serde_json::from_str(include_str!("../capabilities/main-shell.json")).unwrap();
        let plugin_cap: Value =
            serde_json::from_str(include_str!("../capabilities/plugin-sandbox.json")).unwrap();

        assert_eq!(main_cap["windows"], serde_json::json!(["main", "dsh"]));
        assert_eq!(plugin_cap["webviews"], serde_json::json!(["plugin-*"]));

        let plugin_perms = plugin_cap["permissions"].as_array().unwrap();
        assert!(plugin_perms.contains(&serde_json::json!("aitools:guest-sandbox")));
        assert!(!plugin_perms.contains(&serde_json::json!("aitools:default")));
        assert!(plugin_perms.contains(&serde_json::json!("core:window:allow-close")));
        assert!(plugin_perms.contains(&serde_json::json!("core:event:default")));
        assert!(plugin_perms.contains(&serde_json::json!("dialog:default")));
        assert!(plugin_perms.contains(&serde_json::json!("notification:default")));
        assert!(plugin_perms.contains(&serde_json::json!("opener:default")));
    }

    #[test]
    fn spec_016_r4_guest_sandbox_blocks_privileged_commands_and_enforces_labels() {
        let guest_toml =
            include_str!("../../crates/tauri-plugin-aitools/permissions/guest-sandbox.toml");
        for allowed in [
            "allow-db-get",
            "allow-db-put",
            "allow-db-remove",
            "allow-db-all-docs",
            "allow-ai-chat",
            "allow-ai-embed",
            "allow-telemetry-log",
        ] {
            assert!(guest_toml.contains(allowed), "missing {allowed}");
        }
        for forbidden in [
            "dsh",
            "run-agent-dag",
            "resolve-hitl-step",
            "submit-agent-prompt",
            "launch-item",
            "sync-ztools-dir",
            "search-features",
        ] {
            assert!(
                !guest_toml.contains(forbidden),
                "guest-sandbox.toml must not grant {forbidden}"
            );
        }

        assert!(tauri_plugin_aitools::ensure_host_webview("main").is_ok());
        assert!(tauri_plugin_aitools::ensure_host_webview("dsh").is_ok());
        assert!(tauri_plugin_aitools::ensure_host_webview("plugin-todo")
            .unwrap_err()
            .contains("403"));
        for forbidden_new in ["search-list-mode", "select-list-item", "dsh-preload-status"] {
            assert!(
                !guest_toml.contains(forbidden_new),
                "guest-sandbox.toml must not grant {forbidden_new}"
            );
        }
    }

    #[test]
    fn spec_016_r1_appdata_lmdb_persists_and_indexes_apps_and_plugins() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_dir = dir.path().join("lmdb");
        let builtin = super::resolve_builtin_plugins_dir(None).expect("builtin dir must exist");

        {
            let state = super::AitoolsState::open(&db_dir, None, &[]).unwrap();
            let summary = super::index_startup_sources(&state, Some(&builtin), None);
            assert_eq!(summary.plugins_indexed, 8);
            assert!(summary.plugin_features_indexed >= 8);
            let idx = state.search_index.read().unwrap();
            assert!(!idx.is_empty());
            assert!(state.mounts.get_root("clipboard-history").is_some());
            assert!(state.mounts.get_root("json-codec").is_some());

            state
                .storage
                .put_doc(
                    "clipboard-history",
                    aitools_core::Document {
                        id: "persist-1".to_string(),
                        rev: None,
                        data: serde_json::json!({ "text": "saved-across-restart" }),
                    },
                )
                .unwrap();
        }

        // Reopen at same path and confirm LMDB persistence.
        let reopened = super::AitoolsState::open(&db_dir, None, &[]).unwrap();
        let loaded = reopened
            .storage
            .get_doc("clipboard-history", "persist-1")
            .unwrap()
            .expect("persisted LMDB doc must survive reopen");
        assert_eq!(loaded.data["text"], "saved-across-restart");
    }

    #[test]
    fn spec_016_r3_pre_focus_context_tray_and_focus_hide_guard() {
        let dir = tempfile::TempDir::new().unwrap();
        let state = super::AitoolsState::open(dir.path(), None, &[]).unwrap();
        let clipboard = aitools_native::context::MemoryClipboard::with_text("hello context");
        let (wake, envelope) = super::capture_wake_context("wake-test-1", &clipboard, Some(&state));
        assert_eq!(wake.trace_id, "wake-test-1");
        assert_eq!(envelope.clipboard_preview.as_deref(), Some("hello context"));
        assert!(state.latest_context.lock().unwrap().is_some());

        // Normal focus-loss auto-hides, while active HITL or drag guard suppresses hide.
        assert!(super::should_auto_hide_on_focus_loss(Some(&state)));
        state
            .hitl_active
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(!super::should_auto_hide_on_focus_loss(Some(&state)));
        state
            .hitl_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        state.set_focus_hide_suppressed(true);
        assert!(!super::should_auto_hide_on_focus_loss(Some(&state)));
        state.set_focus_hide_suppressed(false);
        assert!(super::should_auto_hide_on_focus_loss(Some(&state)));
    }

    #[test]
    fn spec_017_r1_window_sizing_drag_capabilities_and_dsh_route() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main_cap: Value =
            serde_json::from_str(include_str!("../capabilities/main-shell.json")).unwrap();
        let app_svelte = include_str!("../../packages/renderer-svelte/src/App.svelte");
        let ipc_ts = include_str!("../../packages/renderer-svelte/src/lib/ipc.ts");

        assert_eq!(conf["app"]["macOSPrivateApi"], serde_json::json!(true));
        let windows = conf["app"]["windows"].as_array().unwrap();
        let main_win = windows.iter().find(|w| w["label"] == "main").unwrap();
        assert_eq!(main_win["width"], serde_json::json!(780));
        assert_eq!(main_win["height"], serde_json::json!(62));
        assert_eq!(main_win["transparent"], serde_json::json!(true));
        assert_eq!(main_win["decorations"], serde_json::json!(false));

        let dsh_win = windows.iter().find(|w| w["label"] == "dsh").unwrap();
        assert_eq!(dsh_win["url"], serde_json::json!("/#dsh"));

        let perms = main_cap["permissions"].as_array().unwrap();
        assert!(perms.contains(&serde_json::json!("core:window:allow-set-size")));
        assert!(perms.contains(&serde_json::json!("core:window:allow-start-dragging")));

        assert!(ipc_ts.contains("window.location.hash === \"#dsh\""));
        assert!(ipc_ts.contains("new LogicalSize(780, clamped)"));
        assert!(app_svelte.contains("height: 62px;"));
        assert!(app_svelte.contains("data-tauri-drag-region"));
        assert!(app_svelte.contains("backdrop-filter: blur("));
        assert!(app_svelte.contains("min-height: 0;"));
        assert!(app_svelte.contains("overflow-y: auto;"));
        assert!(app_svelte.contains("<DshPanel"));
    }

    #[test]
    fn spec_017_r2_launcher_aria_keymap_and_ime_composition_guard() {
        let app_svelte = include_str!("../../packages/renderer-svelte/src/App.svelte");
        for required in [
            "role=\"combobox\"",
            "aria-autocomplete=\"list\"",
            "aria-expanded",
            "aria-controls",
            "aria-activedescendant",
            "role=\"listbox\"",
            "role=\"option\"",
            "aria-selected",
            "role=\"alertdialog\"",
            "isComposing",
            "e.keyCode === 229",
            "ArrowDown",
            "ArrowUp",
            "Enter",
            "Escape",
        ] {
            assert!(
                app_svelte.contains(required),
                "App.svelte missing required ARIA/keymap/IME symbol: {required}"
            );
        }
    }

    #[test]
    fn spec_017_r3_enriched_launcher_hit_list_mode_and_tier2_hitl_contract() {
        let dir = tempfile::TempDir::new().unwrap();
        let state = super::AitoolsState::open(dir.path(), None, &[]).unwrap();
        let builtin = super::resolve_builtin_plugins_dir(None).expect("builtin dir must exist");
        let _sync = state.sessions.sync_plugins_directory(&builtin);
        let idx = state.search_index.read().unwrap();
        let decision =
            aitools_ai::launcher::evaluate_launcher_input(&idx, "json", false, false).unwrap();
        let aitools_ai::launcher::LauncherDecision::DeterministicResults { mut matches, .. } =
            decision
        else {
            panic!("expected deterministic hit for 'json'");
        };
        let hit = matches
            .iter_mut()
            .find(|h| h.plugin_id == "json-codec")
            .expect("json-codec hit must exist");
        hit.kind = state
            .sessions
            .classify_hit_kind(&hit.plugin_id, &hit.feature_code)
            .or_else(|| hit.kind.clone());
        assert_eq!(hit.kind.as_deref(), Some("wasm"));
        hit.icon
            .get_or_insert_with(|| "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=".to_string());
        let serialized = serde_json::to_value(&*hit).unwrap();
        for key in [
            "feature_code",
            "plugin_id",
            "label",
            "plugin_name",
            "description",
            "icon",
            "kind",
            "score",
            "match_type",
            "confidence",
            "explain",
        ] {
            assert!(serialized.get(key).is_some(), "missing LauncherHit.{key}");
        }

        let ipc_ts = include_str!("../../packages/renderer-svelte/src/lib/ipc.ts");
        let store_ts = include_str!("../../packages/renderer-svelte/src/stores/launcher.svelte.ts");
        let app_svelte = include_str!("../../packages/renderer-svelte/src/App.svelte");
        assert!(ipc_ts.contains("kind: \"list_mode\""));
        assert!(ipc_ts.contains("plugin:aitools|search_list_mode"));
        assert!(ipc_ts.contains("plugin:aitools|select_list_item"));
        assert!(store_ts.contains("hitl_suspended"));
        assert!(app_svelte.contains("{#if !tier2}"));
        assert!(app_svelte.contains("Approve Session"));
    }

    fn srgb_luminance(hex: &str) -> f64 {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
        let lin = |c: u8| {
            let x = f64::from(c) / 255.0;
            if x <= 0.03928 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    fn wcag_contrast(fg: &str, bg: &str) -> f64 {
        let l1 = srgb_luminance(fg);
        let l2 = srgb_luminance(bg);
        let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn spec_017_r4_design_tokens_contrast_theme_cascade_and_mark_scale() {
        let global_css = include_str!("../../packages/renderer-svelte/src/styles/global.css");
        let mark_svelte =
            include_str!("../../packages/renderer-svelte/src/components/AToolsMark.svelte");
        let dsh_svelte =
            include_str!("../../packages/renderer-svelte/src/components/DshPanel.svelte");

        for required in [
            ":root[data-atools-theme=\"light\"]",
            ":root[data-atools-theme=\"dark\"]",
            ":root:not([data-atools-theme=\"light\"])",
            "\"PingFang SC\"",
            "\"Hiragino Sans GB\"",
            "\"Microsoft YaHei\"",
            ":focus-visible",
            "prefers-reduced-motion: reduce",
        ] {
            assert!(
                global_css.contains(required),
                "global.css missing {required}"
            );
        }

        // Light semantic colors on light bg (`#ebeef2` / `#ffffff`) and dark colors on dark bg (`#2c3340` / `#161a21`).
        for (fg, bg) in [
            ("#1f2328", "#ebeef2"),
            ("#47505c", "#ebeef2"),
            ("#57606a", "#ebeef2"),
            ("#5b21b6", "#ebeef2"),
            ("#b42318", "#ebeef2"),
            ("#0d684b", "#ebeef2"),
            ("#854d0e", "#ebeef2"),
            ("#f0f3f6", "#2c3340"),
            ("#bcc5d1", "#2c3340"),
            ("#98a3b3", "#2c3340"),
            ("#c4b5fd", "#2c3340"),
            ("#ffa3a3", "#2c3340"),
            ("#5cd49b", "#2c3340"),
            ("#f0b959", "#2c3340"),
        ] {
            assert!(
                global_css.contains(fg),
                "expected palette color {fg} in global.css"
            );
            let ratio = wcag_contrast(fg, bg);
            assert!(
                ratio >= 4.5,
                "WCAG AA contrast failed for {fg} on {bg}: {ratio:.2}"
            );
        }

        assert!(mark_svelte.contains("--mark-size: 20px;"));
        assert!(mark_svelte.contains("--mark-size: 32px;"));
        assert!(mark_svelte.contains("--mark-size: 72px;"));
        assert!(dsh_svelte.contains("overflow-x: auto;"));
        assert!(dsh_svelte.contains("min-height: 0;"));
    }
}
