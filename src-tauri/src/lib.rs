//! `aitools-desktop` — Tauri v2 host shell (`SPEC-001`, `SPEC-006`, `SPEC-014`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use aitools_ai::context_fusion::{suggest_zero_query_actions, ContextEnvelope, QuickAgentAction};
use aitools_core::{Cmd, FeatureEntry};
use aitools_native::context::{capture_pre_focus_context, ContextChip, SystemClipboard};
use aitools_telemetry::{TelemetryConfig, TelemetryHandle};
use anyhow::Context;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_aitools::AitoolsState;
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

#[derive(Default)]
struct WakeChannelHolder {
    channel: Mutex<Option<Channel<WakePayload>>>,
}

#[tauri::command]
fn register_wake_channel(
    channel: Channel<WakePayload>,
    holder: tauri::State<'_, WakeChannelHolder>,
) -> Result<(), String> {
    let mut guard = holder
        .channel
        .lock()
        .map_err(|_| "wake channel lock poisoned".to_string())?;
    *guard = Some(channel);
    Ok(())
}

#[tauri::command]
fn hide_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn default_builtin_features() -> Vec<FeatureEntry> {
    vec![
        FeatureEntry {
            code: "clipboard-history".into(),
            plugin_id: "builtin".into(),
            plugin_name: "Built-in".into(),
            label: "剪贴板历史 Clipboard".into(),
            icon: None,
            explain: "Search recent clipboard entries".into(),
            cmds: vec![Cmd::Text("剪贴板".into()), Cmd::Text("clipboard".into())],
            main_push: true,
            priority: 100,
        },
        FeatureEntry {
            code: "json-formatter".into(),
            plugin_id: "builtin".into(),
            plugin_name: "Built-in".into(),
            label: "JSON 格式化 Formatter".into(),
            icon: None,
            explain: "Format and validate JSON payloads".into(),
            cmds: vec![Cmd::Text("json".into()), Cmd::Text("格式化".into())],
            main_push: false,
            priority: 90,
        },
        FeatureEntry {
            code: "color-picker".into(),
            plugin_id: "builtin".into(),
            plugin_name: "Built-in".into(),
            label: "颜色转换 Color".into(),
            icon: None,
            explain: "Convert HEX / RGB / HSL colors".into(),
            cmds: vec![Cmd::Text("color".into()), Cmd::Text("颜色".into())],
            main_push: false,
            priority: 80,
        },
    ]
}

fn handle_wake_hotkey<R: Runtime>(app: &AppHandle<R>) {
    let seq = WAKE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let trace_id = format!("wake-{seq}");
    let _span = info_span!("wake_to_paint", trace_id = %trace_id).entered();

    // SPEC-006-R4: capture PreFocusContext synchronously before focusing main window.
    let clipboard = SystemClipboard;
    let ctx = capture_pre_focus_context(&trace_id, &clipboard, None, None, None);
    let chips = ctx.to_chips();
    let envelope = ContextEnvelope {
        trace_id: trace_id.clone(),
        selection: ctx.selection,
        clipboard: ctx.clipboard,
        active_app: ctx.active_app,
        active_path: ctx.active_path,
        image_handle: None,
    };
    let suggestions = suggest_zero_query_actions(&envelope);

    if let Some(state) = app.try_state::<AitoolsState>() {
        if let Ok(mut guard) = state.latest_context.lock() {
            *guard = Some(envelope);
        }
    }

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }

    let payload = WakePayload {
        trace_id,
        chips,
        suggestions,
    };

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
    let _ = app.emit("aitools://toggle-dsh", ());
}

/// Configures and launches the Tauri v2 desktop application.
///
/// # Errors
///
/// Returns an error if initializing [`AitoolsState`] or starting the Tauri runtime fails.
pub fn run() -> anyhow::Result<()> {
    let app_data = std::env::temp_dir().join("aitools-desktop");
    let log_dir = app_data.join("logs");
    let db_dir = app_data.join("lmdb");

    // SPEC-000: initialize aitools-telemetry first in main().
    let telemetry: Option<std::sync::Arc<TelemetryHandle>> =
        aitools_telemetry::init(TelemetryConfig::new(log_dir)).ok();

    let state = AitoolsState::open(&db_dir, telemetry, &default_builtin_features())
        .with_context(|| format!("failed to initialize AitoolsState at {}", db_dir.display()))?;

    let wake_shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    let dsh_shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyD);

    tauri::Builder::default()
        .manage(state)
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
        ])
        .setup(move |app| {
            if let Err(err) = app.global_shortcut().register(wake_shortcut) {
                tracing::warn!(error = %err, "failed to register wake shortcut");
            }
            if let Err(err) = app.global_shortcut().register(dsh_shortcut) {
                tracing::warn!(error = %err, "failed to register DSH shortcut");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("error while running aitools desktop shell")?;

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
        assert_eq!(plugin_perms.len(), 4);
        assert!(plugin_perms.contains(&serde_json::json!("aitools:default")));
        assert!(plugin_perms.contains(&serde_json::json!("dialog:default")));
        assert!(plugin_perms.contains(&serde_json::json!("notification:default")));
        assert!(plugin_perms.contains(&serde_json::json!("opener:default")));
    }
}
