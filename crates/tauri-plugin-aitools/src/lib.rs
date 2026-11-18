//! `tauri-plugin-aitools` — Integrates core, native, AI, JS bridge, and Wasm host into Tauri v2 (`SPEC-000`, `SPEC-004`, `SPEC-006`, `SPEC-015`, `SPEC-016`).

pub mod protocols;
pub mod session;
mod session_ops;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use aitools_ai::broker::{ModelBroker, ModelProfile, ModelProfileKind, ProviderKind};
use aitools_ai::context_fusion::{suggest_zero_query_actions, ContextEnvelope, QuickAgentAction};
use aitools_ai::hitl::PolicyGuard;
use aitools_ai::launcher::{evaluate_launcher_input, LauncherDecision};
use aitools_ai::planner::{
    execute_agent_prompt, execute_dag, AgentPromptChip, AgentPromptRequest, AgentStreamEvent,
    AgentTurnResult, DagEvent, DagPlan, HitlBroker, HitlResolution,
};
use aitools_ai::registry::ToolRegistry;
use aitools_core::{
    Document, FeatureEntry, FeatureSearchIndex, PluginDataQuery, PluginDataRemove,
    PluginDataWriteResult, StorageEngine,
};
use aitools_js_bridge::{
    build_initialization_script_with_preload_props, invoke_preload_list_feature, CallbackTable,
    ExportedWindowProp, JsBridgeHost, ListPushEvent, ListPushSink,
};
use aitools_native::context::{ClipboardProvider, SystemClipboard};
use aitools_telemetry::{LogRecord, TelemetryHandle};
use aitools_wasm_host::WasmPluginHost;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::http::{header::CONTENT_TYPE, Response};
use tauri::ipc::Channel;
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use thiserror::Error;

use crate::protocols::{
    handle_plugin_asset_request, handle_sync_request_with_session, FaultInjectionConfig,
    PluginMountRegistry, ProtocolReply,
};
pub use crate::session::{
    DshPreloadSummary, HostWindowController, KeychainApiKeyProvider, LaunchResult,
    ListSearchResponse, NoopWindowController, PluginSessionManager, PluginWindowSpec,
    PreloadPluginStatus, SelectListResult, SessionDependencies, SyncZtoolsResult,
};

/// Errors returned by public `tauri-plugin-aitools` setup and window builders.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginError {
    /// Core storage, search index, or manifest failure.
    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    /// Sandboxed Wasmtime host initialization or execution failure.
    #[error("wasm host error: {0}")]
    Wasm(#[from] aitools_wasm_host::WasmHostError),

    /// Filesystem or I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Underlying Tauri runtime or window creation failure.
    #[error("tauri error: {0}")]
    Tauri(#[from] tauri::Error),

    /// Plugin custom-protocol URL could not be constructed or parsed safely.
    #[error("invalid plugin URL `{url}`: {reason}")]
    InvalidPluginUrl {
        /// The raw URL string that failed validation.
        url: String,
        /// Reason the URL was rejected.
        reason: String,
    },
}

/// Convenience result alias for `tauri-plugin-aitools` operations.
pub type Result<T> = std::result::Result<T, PluginError>;

/// Shared desktop runtime state managed by `tauri-plugin-aitools`.
pub struct AitoolsState {
    /// LMDB-backed persistent storage engine.
    pub storage: Arc<StorageEngine>,
    /// Optional structured telemetry handle for diagnostics.
    pub telemetry: Option<Arc<TelemetryHandle>>,
    /// Registry mapping plugin IDs to canonicalized asset directories.
    pub mounts: PluginMountRegistry,
    /// Hybrid Pinyin and fuzzy feature search index.
    pub search_index: Arc<RwLock<FeatureSearchIndex>>,
    /// Table of active streaming list callbacks for legacy preload features.
    pub callback_table: CallbackTable,
    /// QuickJS-backed host for legacy Tier 1 preload plugins.
    pub js_bridge: Mutex<JsBridgeHost>,
    /// Sandboxed Wasmtime host for Tier 3 plugins.
    pub wasm_host: Arc<WasmPluginHost>,
    /// Registry of built-in and plugin-provided agent tools.
    pub tool_registry: RwLock<ToolRegistry>,
    /// Centralized multi-provider AI model & quota broker.
    pub model_broker: Arc<RwLock<ModelBroker>>,
    /// Risk-tier policy guard enforcing HITL approvals.
    pub policy_guard: Arc<PolicyGuard>,
    /// Broker coordinating pending HITL confirmation steps.
    pub hitl_broker: Arc<HitlBroker>,
    /// Runtime DSH fault injection toggles.
    pub faults: Arc<Mutex<FaultInjectionConfig>>,
    /// Most recent pre-focus context captured on global wake.
    pub latest_context: Mutex<Option<ContextEnvelope>>,
    /// Shared HTTP client reused across Keychain AI calls and preload fetch bridges.
    pub http_client: reqwest::Client,
    /// Coordinates loaded ZTools/built-in plugins, 3-tier preloads, and OS bridges.
    pub sessions: Arc<PluginSessionManager>,
    /// Indicates an active HITL approval blocking focus-loss auto-hide (`SPEC-016-R3`).
    pub hitl_active: Arc<AtomicBool>,
    /// Manual guard suppressing focus-loss auto-hide during window drag/modal state (`SPEC-016-R3`).
    pub focus_hide_suppressed: Arc<AtomicBool>,
}

impl fmt::Debug for AitoolsState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AitoolsState")
            .field("has_telemetry", &self.telemetry.is_some())
            .field("mounts", &self.mounts)
            .finish_non_exhaustive()
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AitoolsState>();
};

impl AitoolsState {
    /// Initializes a new [`AitoolsState`] backed by LMDB at `db_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Core`] if opening the LMDB environment or building the search index
    /// fails, or [`PluginError::Wasm`] if initializing the Wasmtime host fails.
    pub fn open(
        db_dir: impl AsRef<Path>,
        telemetry: Option<Arc<TelemetryHandle>>,
        initial_features: &[FeatureEntry],
    ) -> Result<Self> {
        let db_path = db_dir.as_ref();
        let storage = Arc::new(StorageEngine::open(db_path)?);
        let search_index = Arc::new(RwLock::new(FeatureSearchIndex::try_build(
            initial_features,
        )?));
        let mounts = PluginMountRegistry::new();
        let default_icon_cache = if db_path.file_name().is_some_and(|n| n == "lmdb") {
            db_path.parent().unwrap_or(db_path).join("icon-cache")
        } else {
            db_path.join("icon-cache")
        };
        mounts.set_icon_cache_dir(default_icon_cache);
        let wasm_host = Arc::new(WasmPluginHost::with_defaults()?);
        let mut default_broker = ModelBroker::new();
        default_broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Fast,
            provider: ProviderKind::OpenAiCompatible,
            base_url: std::env::var("AITOOLS_OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            model: std::env::var("AITOOLS_FAST_MODEL")
                .unwrap_or_else(|_| "gpt-4o-mini".to_string()),
            keychain_account: "aitools.ai.fast".to_string(),
        });
        default_broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Balanced,
            provider: ProviderKind::OpenAiCompatible,
            base_url: std::env::var("AITOOLS_OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            model: std::env::var("AITOOLS_BALANCED_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            keychain_account: "aitools.ai.balanced".to_string(),
        });
        default_broker.register_profile(ModelProfile {
            kind: ModelProfileKind::Deep,
            provider: ProviderKind::Anthropic,
            base_url: std::env::var("AITOOLS_ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            model: std::env::var("AITOOLS_DEEP_MODEL")
                .unwrap_or_else(|_| "claude-3-5-sonnet-latest".to_string()),
            keychain_account: "aitools.ai.deep".to_string(),
        });
        let model_broker = Arc::new(RwLock::new(default_broker));
        let faults = Arc::new(Mutex::new(FaultInjectionConfig::default()));

        let mut registry = ToolRegistry::new();
        registry.register_builtin_os_defaults();

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        let sessions = PluginSessionManager::new(SessionDependencies {
            storage: Arc::clone(&storage),
            telemetry: telemetry.clone(),
            mounts: mounts.clone(),
            search_index: Arc::clone(&search_index),
            wasm_host: Arc::clone(&wasm_host),
            model_broker: Arc::clone(&model_broker),
            faults: Arc::clone(&faults),
            initial_features,
        });

        Ok(Self {
            storage,
            telemetry,
            mounts,
            search_index,
            callback_table: CallbackTable::new(),
            js_bridge: Mutex::new(JsBridgeHost::new()),
            wasm_host,
            tool_registry: RwLock::new(registry),
            model_broker,
            policy_guard: Arc::new(PolicyGuard::new()),
            hitl_broker: Arc::new(HitlBroker::new()),
            faults,
            latest_context: Mutex::new(None),
            http_client,
            sessions,
            hitl_active: Arc::new(AtomicBool::new(false)),
            focus_hide_suppressed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Attaches a live Tauri [`AppHandle`] so plugin windows, notifications, dialogs, and shell openers run natively.
    pub fn attach_app_handle<R: Runtime>(&self, app: AppHandle<R>) {
        self.sessions
            .set_window_controller(Arc::new(TauriHostController { app }));
    }

    /// Returns `true` when focus-loss auto-hide on the main launcher window should be suppressed (`SPEC-016-R3`).
    #[must_use]
    pub fn is_focus_hide_suppressed(&self) -> bool {
        self.hitl_active.load(Ordering::Relaxed)
            || self.focus_hide_suppressed.load(Ordering::Relaxed)
    }

    /// Updates manual focus-loss hide suppression (e.g. during active drag/dialog state).
    pub fn set_focus_hide_suppressed(&self, suppressed: bool) {
        self.focus_hide_suppressed
            .store(suppressed, Ordering::Relaxed);
    }
}

fn parse_plugin_url(plugin_id: &str, entry_path: &str) -> Result<Url> {
    if plugin_id.is_empty()
        || !plugin_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(PluginError::InvalidPluginUrl {
            url: format!("aitools-plugin://{plugin_id}"),
            reason: "plugin id must be non-empty ASCII alphanumeric, '-', '_', or '.'".to_string(),
        });
    }
    let raw_url = format!(
        "aitools-plugin://{plugin_id}/{}",
        entry_path.trim_start_matches('/')
    );
    raw_url
        .parse()
        .map_err(|err| PluginError::InvalidPluginUrl {
            url: raw_url,
            reason: format!("{err}"),
        })
}

/// Builds a sandboxed child plugin window with deterministic pre-navigation script injection (`INV-SEC-02`).
///
/// # Errors
///
/// Returns [`PluginError::InvalidPluginUrl`] if `plugin_id` or `entry_path` does not form a valid
/// `aitools-plugin://` URL, or [`PluginError::Tauri`] if creating the webview window fails.
pub fn build_plugin_window<R: Runtime>(
    app: &AppHandle<R>,
    plugin_id: &str,
    entry_path: &str,
    title: &str,
    width: f64,
    height: f64,
) -> Result<WebviewWindow<R>> {
    build_plugin_window_with_props(app, plugin_id, entry_path, title, width, height, &[])
}

/// Builds (or returns) a sandboxed child plugin window injecting both triple-global shim and
/// preload `window.<prop>` RPC stubs (`INV-SEC-02`, `SPEC-015-R4`).
///
/// # Errors
///
/// Returns [`PluginError::InvalidPluginUrl`] if `plugin_id` or `entry_path` fails validation,
/// or [`PluginError::Tauri`] if building the window fails.
pub fn build_plugin_window_with_props<R: Runtime>(
    app: &AppHandle<R>,
    plugin_id: &str,
    entry_path: &str,
    title: &str,
    width: f64,
    height: f64,
    exported_props: &[ExportedWindowProp],
) -> Result<WebviewWindow<R>> {
    let label = format!("plugin-{plugin_id}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(existing);
    }

    let url = if entry_path.starts_with("http://") || entry_path.starts_with("https://") {
        let parsed: Url = entry_path
            .parse()
            .map_err(|err| PluginError::InvalidPluginUrl {
                url: entry_path.to_string(),
                reason: format!("{err}"),
            })?;
        WebviewUrl::External(parsed)
    } else {
        WebviewUrl::CustomProtocol(parse_plugin_url(plugin_id, entry_path)?)
    };
    let init_script = build_initialization_script_with_preload_props(plugin_id, exported_props);

    Ok(WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(width, height)
        .initialization_script(&init_script)
        .build()?)
}

struct TauriHostController<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> HostWindowController for TauriHostController<R> {
    fn open_plugin_window(
        &self,
        spec: PluginWindowSpec<'_>,
    ) -> std::result::Result<String, String> {
        let win = build_plugin_window_with_props(
            &self.app,
            spec.plugin_id,
            spec.entry_path,
            spec.title,
            spec.width,
            spec.height,
            spec.exported_props,
        )
        .map_err(|e| e.to_string())?;
        let label = win.label().to_string();
        let _ = win.show();
        let _ = win.set_focus();
        if let Some(action) = spec.init_action {
            let js = format!(
                "if (typeof globalThis.__aitools_dispatch_lifecycle__ === 'function') {{ globalThis.__aitools_dispatch_lifecycle__('pluginEnter', {}); }}",
                serde_json::to_string(&action).unwrap_or_else(|_| "{}".to_string())
            );
            let _ = win.eval(&js);
        }
        Ok(label)
    }

    fn close_plugin_window(&self, plugin_id: &str) -> std::result::Result<(), String> {
        let label = format!("plugin-{plugin_id}");
        if let Some(win) = self.app.get_webview_window(&label) {
            win.close().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn hide_main_window(&self) -> std::result::Result<(), String> {
        if let Some(win) = self.app.get_webview_window("main") {
            win.hide().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn show_main_window(&self) -> std::result::Result<(), String> {
        if let Some(win) = self.app.get_webview_window("main") {
            win.show().map_err(|e| e.to_string())?;
            win.set_focus().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn set_expand_height(&self, height: f64) -> std::result::Result<(), String> {
        if let Some(win) = self.app.get_webview_window("main") {
            let clamped_h = if height.is_finite() {
                height.clamp(58.0, 594.0)
            } else {
                58.0
            };
            let anchor_pos = win.outer_position().ok();
            let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
                800.0, clamped_h,
            )));
            if let Some(pos) = anchor_pos {
                let _ = win.set_position(pos);
            }
        }
        Ok(())
    }

    fn emit_event(&self, event: &str, payload: Value) {
        let _ = self.app.emit(event, payload);
    }

    fn eval_plugin_lifecycle(&self, plugin_id: &str, event: &str, arg1: Value, arg2: Value) {
        let label = format!("plugin-{plugin_id}");
        if let Some(win) = self.app.get_webview_window(&label) {
            let ev_json = serde_json::to_string(event).unwrap_or_else(|_| "\"\"".to_string());
            let a1_json = serde_json::to_string(&arg1).unwrap_or_else(|_| "null".to_string());
            let a2_json = serde_json::to_string(&arg2).unwrap_or_else(|_| "null".to_string());
            let script = format!(
                "if (typeof globalThis.__aitools_dispatch_lifecycle__ === 'function') {{ globalThis.__aitools_dispatch_lifecycle__({ev_json}, {a1_json}, {a2_json}); }}"
            );
            let _ = win.eval(&script);
        }
    }

    fn show_notification(
        &self,
        title: &str,
        body: &str,
        _click_feature_code: Option<&str>,
    ) -> std::result::Result<(), String> {
        self.app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }

    fn open_external(&self, url: &str) -> std::result::Result<(), String> {
        self.app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())
    }

    fn open_path(&self, path: &str) -> std::result::Result<(), String> {
        self.app
            .opener()
            .open_path(path, None::<&str>)
            .map_err(|e| e.to_string())
    }

    fn show_item_in_folder(&self, path: &str) -> std::result::Result<(), String> {
        self.app
            .opener()
            .reveal_item_in_dir(path)
            .map_err(|e| e.to_string())
    }

    fn beep(&self) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("afplay")
                .arg("/System/Library/Sounds/Tink.aiff")
                .spawn();
        }
    }

    fn show_open_dialog(
        &self,
        options: &Value,
    ) -> std::result::Result<Option<Vec<String>>, String> {
        let mut builder = self.app.dialog().file();
        if let Some(title) = options.get("title").and_then(Value::as_str) {
            builder = builder.set_title(title);
        }
        if let Some(dir) = options.get("defaultPath").and_then(Value::as_str) {
            builder = builder.set_directory(dir);
        }
        let multi = options
            .get("properties")
            .and_then(Value::as_array)
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("multiSelections")));
        if multi {
            Ok(builder
                .blocking_pick_files()
                .map(|list| list.into_iter().map(|p| p.to_string()).collect()))
        } else {
            Ok(builder.blocking_pick_file().map(|p| vec![p.to_string()]))
        }
    }

    fn show_save_dialog(&self, options: &Value) -> std::result::Result<Option<String>, String> {
        let mut builder = self.app.dialog().file();
        if let Some(title) = options.get("title").and_then(Value::as_str) {
            builder = builder.set_title(title);
        }
        if let Some(dir) = options.get("defaultPath").and_then(Value::as_str) {
            builder = builder.set_directory(dir);
        }
        Ok(builder.blocking_save_file().map(|p| p.to_string()))
    }
}

struct ChannelListSink {
    channel: Channel<ListPushEvent>,
}

impl ListPushSink for ChannelListSink {
    fn push_list(&self, event: ListPushEvent) {
        let _ = self.channel.send(event);
    }
}

/// Structured frontend log entry forwarded to DSH telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendLogPayload {
    /// Severity level (for example, `"info"`, `"warn"`, or `"error"`).
    pub level: String,
    /// Origin component or module emitting the log.
    pub source: String,
    /// Optional plugin identifier associated with the log entry.
    pub plugin_id: Option<String>,
    /// Optional trace correlation identifier.
    pub trace_id: Option<String>,
    /// Human-readable log message.
    pub msg: String,
    /// Additional structured key-value fields.
    #[serde(default)]
    pub fields: Map<String, Value>,
}

#[tauri::command]
fn search_features<R: Runtime>(
    query: String,
    tab_pressed: bool,
    submitted: bool,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<LauncherDecision, String> {
    ensure_host_webview(webview.label())?;
    let index = state
        .search_index
        .read()
        .map_err(|_| "search index lock poisoned".to_string())?;
    let mut decision = evaluate_launcher_input(&index, &query, tab_pressed, submitted)
        .map_err(|e| e.to_string())?;
    if let LauncherDecision::DeterministicResults {
        ref mut matches, ..
    } = decision
    {
        for hit in matches.iter_mut() {
            if let Some(kind) = state
                .sessions
                .classify_hit_kind(&hit.plugin_id, &hit.feature_code)
            {
                hit.kind = Some(kind);
            }
        }
    }
    Ok(decision)
}

#[tauri::command]
fn featured_home_items<R: Runtime>(
    limit: Option<usize>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<aitools_ai::launcher::LauncherHit>, String> {
    ensure_host_webview(webview.label())?;
    let clamped = limit.unwrap_or(27).clamp(1, 54);
    Ok(state.sessions.featured_home_items(clamped))
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri IPC command parameters map directly to flat JSON invoke keys"
)]
fn launch_item<R: Runtime>(
    feature_code: String,
    plugin_id: String,
    query: String,
    kind: Option<String>,
    trace_id: Option<String>,
    on_list_event: Channel<ListPushEvent>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<LaunchResult, String> {
    ensure_host_webview(webview.label())?;
    let sink: Arc<dyn ListPushSink> = Arc::new(ChannelListSink {
        channel: on_list_event,
    });
    state.sessions.launch_item(
        &feature_code,
        &plugin_id,
        &query,
        kind.as_deref(),
        trace_id.as_deref(),
        Some(sink),
    )
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri IPC command parameters map directly to flat JSON invoke keys"
)]
fn search_list_mode<R: Runtime>(
    session_id: String,
    plugin_id: String,
    feature_code: String,
    query: String,
    trace_id: Option<String>,
    on_list_event: Channel<ListPushEvent>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<ListSearchResponse, String> {
    ensure_host_webview(webview.label())?;
    let _ = trace_id;
    let sink: Arc<dyn ListPushSink> = Arc::new(ChannelListSink {
        channel: on_list_event,
    });
    state
        .sessions
        .search_list_mode(&session_id, &plugin_id, &feature_code, &query, Some(sink))
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri IPC command parameters map directly to flat JSON invoke keys"
)]
fn select_list_item<R: Runtime>(
    session_id: String,
    plugin_id: String,
    feature_code: String,
    item: Value,
    index: usize,
    trace_id: Option<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<SelectListResult, String> {
    ensure_host_webview(webview.label())?;
    let _ = trace_id;
    state
        .sessions
        .select_list_item(&session_id, &plugin_id, &feature_code, item, index)
}

#[tauri::command]
fn sync_ztools_dir<R: Runtime>(
    dir_path: Option<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<SyncZtoolsResult, String> {
    ensure_host_webview(webview.label())?;
    let target = match dir_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aitools")
            .join("plugins")
            .join("ztools"),
    };
    Ok(state.sessions.sync_plugins_directory(&target))
}

#[tauri::command]
fn dsh_preload_status<R: Runtime>(
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<DshPreloadSummary, String> {
    ensure_host_webview(webview.label())?;
    Ok(state.sessions.preload_status_summary())
}

#[tauri::command]
fn zero_query_suggestions<R: Runtime>(
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<QuickAgentAction>, String> {
    ensure_host_webview(webview.label())?;
    let envelope = state
        .latest_context
        .lock()
        .map_err(|_| "context lock poisoned".to_string())?
        .clone()
        .unwrap_or_default();
    Ok(suggest_zero_query_actions(&envelope))
}

struct ResetBoolGuard(Arc<AtomicBool>);

impl Drop for ResetBoolGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

fn build_tool_executor(
    sessions: Arc<PluginSessionManager>,
) -> Arc<dyn Fn(String, Value) -> aitools_ai::planner::ToolExecFuture + Send + Sync> {
    Arc::new(move |tool_name: String, args: Value| {
        let sessions = Arc::clone(&sessions);
        Box::pin(async move { execute_host_tool(&sessions, &tool_name, args) })
            as aitools_ai::planner::ToolExecFuture
    })
}

fn execute_host_tool(
    _sessions: &PluginSessionManager,
    tool_name: &str,
    args: Value,
) -> std::result::Result<Value, aitools_ai::AiError> {
    match tool_name {
        "os.clipboard.read" => {
            let text = SystemClipboard
                .get_text()
                .ok()
                .flatten()
                .unwrap_or_default();
            Ok(json!({ "text": text }))
        }
        "os.files.find" => {
            let query = args
                .get("pattern")
                .or_else(|| args.get("query"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let root = args
                .get("root")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."));
            let mut matches = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&root) {
                for entry in entries.flatten().take(200) {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    if query.is_empty() || name.to_ascii_lowercase().contains(&query) {
                        matches.push(path.to_string_lossy().to_string());
                        if matches.len() >= 25 {
                            break;
                        }
                    }
                }
            }
            Ok(json!({ "root": root.to_string_lossy(), "matches": matches }))
        }
        "os.shell.run" => {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if command.is_empty() {
                return Ok(json!({ "ok": false, "stdout": "", "stderr": "empty command" }));
            }
            #[cfg(target_os = "windows")]
            let out = std::process::Command::new("cmd")
                .args(["/C", command])
                .output();
            #[cfg(not(target_os = "windows"))]
            let out = std::process::Command::new("sh")
                .args(["-c", command])
                .output();
            match out {
                Ok(output) => Ok(json!({
                    "ok": output.status.success(),
                    "status": output.status.code(),
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                })),
                Err(err) => Err(aitools_ai::AiError::Provider(format!(
                    "shell execution failed: {err}"
                ))),
            }
        }
        _ => Ok(json!({ "executed": tool_name, "args": args })),
    }
}

#[tauri::command]
async fn submit_agent_prompt<R: Runtime>(
    prompt: String,
    trace_id: String,
    chips: Vec<AgentPromptChip>,
    profile: Option<String>,
    on_event: Channel<AgentStreamEvent>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<AgentTurnResult, String> {
    ensure_host_webview(webview.label())?;
    let faults = state
        .faults
        .lock()
        .map_err(|_| "fault lock poisoned".to_string())?
        .clone();
    if faults.inject_llm_stream_abort {
        return Err("DSH fault injected: LLM stream aborted".to_string());
    }

    let _guard_reset = ResetBoolGuard(Arc::clone(&state.hitl_active));
    let hitl_flag = Arc::clone(&state.hitl_active);
    let broker_snapshot = state
        .model_broker
        .read()
        .map_err(|_| "model broker lock poisoned".to_string())?
        .clone();
    let registry_snapshot = state
        .tool_registry
        .read()
        .map_err(|_| "tool registry lock poisoned".to_string())?
        .clone();
    let policy_guard = Arc::clone(&state.policy_guard);
    let hitl_broker = Arc::clone(&state.hitl_broker);
    let executor = Arc::new({
        let sessions = Arc::clone(&state.sessions);
        move |tool_name: String, args: Value| {
            let sessions = Arc::clone(&sessions);
            Box::pin(async move { execute_host_tool(&sessions, &tool_name, args) })
                as aitools_ai::planner::ToolExecFuture
        }
    });
    let profile_kind = ModelProfileKind::parse_lenient(profile.as_deref().unwrap_or("balanced"));
    let keys = KeychainApiKeyProvider;

    execute_agent_prompt(
        AgentPromptRequest {
            prompt: &prompt,
            trace_id: &trace_id,
            chips: &chips,
            profile: profile_kind,
        },
        &state.http_client,
        &broker_snapshot,
        &keys,
        &registry_snapshot,
        policy_guard,
        hitl_broker,
        move |ev| {
            if matches!(ev, AgentStreamEvent::HitlSuspended { .. }) {
                hitl_flag.store(true, Ordering::Relaxed);
            } else if matches!(
                ev,
                AgentStreamEvent::NodeCompleted { .. }
                    | AgentStreamEvent::NodeFailed { .. }
                    | AgentStreamEvent::Completed { .. }
            ) {
                hitl_flag.store(false, Ordering::Relaxed);
            }
            let _ = on_event.send(ev);
        },
        executor,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_agent_dag<R: Runtime>(
    plan: DagPlan,
    on_event: Channel<DagEvent>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<HashMapStringValue, String> {
    ensure_host_webview(webview.label())?;
    let faults = state
        .faults
        .lock()
        .map_err(|_| "fault lock poisoned".to_string())?
        .clone();
    if faults.inject_llm_stream_abort {
        return Err("DSH fault injected: LLM stream aborted".to_string());
    }

    let _guard_reset = ResetBoolGuard(Arc::clone(&state.hitl_active));
    let hitl_flag = Arc::clone(&state.hitl_active);
    let guard = Arc::clone(&state.policy_guard);
    let broker = Arc::clone(&state.hitl_broker);
    let sessions = Arc::clone(&state.sessions);
    let _ = build_tool_executor(Arc::clone(&sessions));
    let executor = Arc::new(move |tool_name: String, args: Value| {
        let sessions = Arc::clone(&sessions);
        Box::pin(async move { execute_host_tool(&sessions, &tool_name, args) })
            as aitools_ai::planner::ToolExecFuture
    });

    execute_dag(
        &plan,
        guard,
        broker,
        move |ev| {
            if matches!(ev, DagEvent::HitlSuspended { .. }) {
                hitl_flag.store(true, Ordering::Relaxed);
            }
            let _ = on_event.send(ev);
        },
        executor,
    )
    .await
    .map(HashMapStringValue)
    .map_err(|e| e.to_string())
}

/// Serializable map of executed DAG node outputs keyed by node ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HashMapStringValue(std::collections::HashMap<String, Value>);

#[tauri::command]
fn resolve_hitl_step<R: Runtime>(
    node_id: String,
    resolution: HitlResolution,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<bool, String> {
    ensure_host_webview(webview.label())?;
    let resolved = state.hitl_broker.resolve(&node_id, resolution);
    if resolved {
        state.hitl_active.store(false, Ordering::Relaxed);
    }
    Ok(resolved)
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri IPC command parameters map directly to flat JSON invoke keys"
)]
fn invoke_legacy_list_feature<R: Runtime>(
    plugin_id: String,
    trace_id: String,
    preload_code: String,
    feature_code: String,
    action_payload: Value,
    search_query: Option<String>,
    channel: Channel<ListPushEvent>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<u64, String> {
    ensure_host_webview(webview.label())?;
    let sink = Arc::new(ChannelListSink { channel });
    let handle = invoke_preload_list_feature(
        &plugin_id,
        &trace_id,
        &preload_code,
        &feature_code,
        &action_payload,
        search_query.as_deref(),
        &state.callback_table,
        sink,
    )
    .map_err(|e| e.to_string())?;
    Ok(handle.as_u64())
}

#[tauri::command]
fn dsh_recent_logs<R: Runtime>(
    limit: usize,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<LogRecord>, String> {
    ensure_host_webview(webview.label())?;
    let Some(ref telemetry) = state.telemetry else {
        return Ok(Vec::new());
    };
    Ok(telemetry.recent_records(limit.min(500)))
}

#[tauri::command]
fn dsh_ingest_frontend_log<R: Runtime>(
    payload: FrontendLogPayload,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<(), String> {
    ensure_host_webview(webview.label())?;
    if let Some(ref telemetry) = state.telemetry {
        telemetry.ingest_external(
            &payload.level,
            &payload.source,
            payload.plugin_id,
            payload.trace_id,
            &payload.msg,
            payload.fields,
        );
    }
    Ok(())
}

#[tauri::command]
fn dsh_set_fault_injection<R: Runtime>(
    config: FaultInjectionConfig,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<FaultInjectionConfig, String> {
    ensure_host_webview(webview.label())?;
    let mut guard = state
        .faults
        .lock()
        .map_err(|_| "fault lock poisoned".to_string())?;
    *guard = config.clone();
    Ok(config)
}

/// Ensures a privileged host command cannot be invoked from any `plugin-*` guest webview (`SPEC-016-R4`).
///
/// # Errors
///
/// Returns a `"forbidden (403): ..."` string if `label` begins with `"plugin-"`.
pub fn ensure_host_webview(label: &str) -> std::result::Result<(), String> {
    if label.starts_with("plugin-") {
        return Err(format!(
            "forbidden (403): guest webview '{label}' is not permitted to invoke host commands"
        ));
    }
    Ok(())
}

fn plugin_id_from_label(label: &str) -> std::result::Result<&str, String> {
    label
        .strip_prefix("plugin-")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| format!("forbidden (403): webview '{label}' is not a sandboxed plugin"))
}

fn sdk_db_get(
    storage: &StorageEngine,
    label: &str,
    id: &str,
) -> std::result::Result<Option<Document>, String> {
    let plugin_id = plugin_id_from_label(label)?;
    storage.get_doc(plugin_id, id).map_err(|e| e.to_string())
}

fn sdk_db_put(
    storage: &StorageEngine,
    label: &str,
    doc: Document,
) -> std::result::Result<PluginDataWriteResult, String> {
    let plugin_id = plugin_id_from_label(label)?;
    storage.put_doc(plugin_id, doc).map_err(|e| e.to_string())
}

fn sdk_db_remove(
    storage: &StorageEngine,
    label: &str,
    id: String,
    rev: Option<String>,
) -> std::result::Result<PluginDataWriteResult, String> {
    let plugin_id = plugin_id_from_label(label)?;
    let target = match rev {
        Some(r) => PluginDataRemove::Document(Document {
            id,
            rev: Some(r),
            data: json!({}),
        }),
        None => PluginDataRemove::Id(id),
    };
    storage
        .remove_doc(plugin_id, target)
        .map_err(|e| e.to_string())
}

fn sdk_db_all_docs(
    storage: &StorageEngine,
    label: &str,
    prefix: Option<String>,
) -> std::result::Result<Vec<Document>, String> {
    let plugin_id = plugin_id_from_label(label)?;
    let query = match prefix {
        Some(p) if !p.is_empty() => PluginDataQuery::Prefix(p),
        _ => PluginDataQuery::All,
    };
    storage
        .all_docs(plugin_id, query)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SdkChatRequest {
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    system: Option<String>,
    prompt: String,
}

#[tauri::command]
fn db_get<R: Runtime>(
    id: String,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Option<Document>, String> {
    sdk_db_get(&state.storage, webview.label(), &id)
}

#[tauri::command]
fn db_put<R: Runtime>(
    doc: Document,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<PluginDataWriteResult, String> {
    sdk_db_put(&state.storage, webview.label(), doc)
}

#[tauri::command]
fn db_remove<R: Runtime>(
    id: String,
    rev: Option<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<PluginDataWriteResult, String> {
    sdk_db_remove(&state.storage, webview.label(), id, rev)
}

#[tauri::command]
fn db_all_docs<R: Runtime>(
    prefix: Option<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<Document>, String> {
    sdk_db_all_docs(&state.storage, webview.label(), prefix)
}

#[tauri::command]
async fn ai_chat<R: Runtime>(
    request: SdkChatRequest,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<String, String> {
    let plugin_id = plugin_id_from_label(webview.label())?;
    let broker_snapshot = state
        .model_broker
        .read()
        .map_err(|_| "model broker lock poisoned".to_string())?
        .clone();
    let profile = ModelProfileKind::parse_lenient(request.profile.as_deref().unwrap_or("fast"));
    let keys = KeychainApiKeyProvider;
    let outcome = broker_snapshot
        .chat_text(
            &state.http_client,
            &keys,
            plugin_id,
            profile,
            request.system.as_deref(),
            &request.prompt,
            None,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(outcome.content)
}

#[tauri::command]
async fn ai_embed<R: Runtime>(
    texts: Vec<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<Vec<f32>>, String> {
    let plugin_id = plugin_id_from_label(webview.label())?;
    let broker_snapshot = state
        .model_broker
        .read()
        .map_err(|_| "model broker lock poisoned".to_string())?
        .clone();
    let keys = KeychainApiKeyProvider;
    let outcome = broker_snapshot
        .embed_texts(&state.http_client, &keys, plugin_id, &texts)
        .await
        .map_err(|e| e.to_string())?;
    Ok(outcome.vectors)
}

#[tauri::command]
fn telemetry_log<R: Runtime>(
    level: String,
    msg: String,
    fields: Map<String, Value>,
    webview: tauri::Webview<R>,
    request: tauri::ipc::Request<'_>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<(), String> {
    let label = webview.label().to_string();
    let plugin_id = label.strip_prefix("plugin-").map(str::to_string);
    let trace_id = request
        .headers()
        .get("x-aitools-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(ref telemetry) = state.telemetry {
        telemetry.ingest_external(&level, &label, plugin_id, trace_id, &msg, fields);
    }
    Ok(())
}

fn to_http_response(reply: ProtocolReply) -> Response<Vec<u8>> {
    Response::builder()
        .status(reply.status)
        .header(CONTENT_TYPE, reply.content_type)
        .body(reply.body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Constructs the `aitools` Tauri v2 plugin with `aitools-plugin://`, `aitools-sync://`, and IPC commands.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::new("aitools")
        .setup(|app, _api| {
            if let Some(state) = app.try_state::<AitoolsState>() {
                state.attach_app_handle(app.clone());
            }
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("aitools-plugin", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            let uri = request.uri().to_string();
            let mounts = ctx.app_handle().state::<AitoolsState>().mounts.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let reply = handle_plugin_asset_request(&label, &uri, &mounts);
                responder.respond(to_http_response(reply));
            });
        })
        .register_asynchronous_uri_scheme_protocol("aitools-sync", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            let uri = request.uri().to_string();
            let body = request.body().clone();
            let state = ctx.app_handle().state::<AitoolsState>();
            let reply = handle_sync_request_with_session(&label, &uri, &body, &state.sessions);
            responder.respond(to_http_response(reply));
        })
        .invoke_handler(tauri::generate_handler![
            search_features,
            featured_home_items,
            zero_query_suggestions,
            launch_item,
            search_list_mode,
            select_list_item,
            sync_ztools_dir,
            submit_agent_prompt,
            run_agent_dag,
            resolve_hitl_step,
            invoke_legacy_list_feature,
            dsh_recent_logs,
            dsh_ingest_frontend_log,
            dsh_set_fault_injection,
            dsh_preload_status,
            db_get,
            db_put,
            db_remove,
            db_all_docs,
            ai_chat,
            ai_embed,
            telemetry_log,
        ])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocols::handle_sync_request;
    use base64::Engine as _;
    use tempfile::TempDir;

    #[test]
    fn inv_sec_01_blocks_cross_plugin_and_path_traversal() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("demo");
        let outside_file = dir.path().join("secret.txt");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("index.html"), "<h1>ok</h1>").unwrap();
        std::fs::write(&outside_file, "top-secret").unwrap();

        let mounts = PluginMountRegistry::new();
        mounts.register_root("demo", &plugin_dir).unwrap();

        let ok =
            handle_plugin_asset_request("plugin-demo", "aitools-plugin://demo/index.html", &mounts);
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body, b"<h1>ok</h1>");

        let cross =
            handle_plugin_asset_request("plugin-evil", "aitools-plugin://demo/index.html", &mounts);
        assert_eq!(cross.status, 403);

        let traversal = handle_plugin_asset_request(
            "plugin-demo",
            "aitools-plugin://demo/../secret.txt",
            &mounts,
        );
        assert_eq!(traversal.status, 403);
    }

    #[test]
    fn spec_002_r4_aitools_sync_enforces_caller_and_returns_real_rev() {
        let dir = TempDir::new().unwrap();
        let storage = StorageEngine::open(dir.path()).unwrap();
        let faults = Mutex::new(FaultInjectionConfig::default());

        let forbidden = handle_sync_request(
            "main",
            "aitools-sync://demo/db/put",
            br#"{"_id":"k1","v":1}"#,
            &storage,
            &faults,
        );
        assert_eq!(forbidden.status, 403);

        let put = handle_sync_request(
            "plugin-demo",
            "aitools-sync://demo/db/put",
            br#"{"_id":"k1","v":1}"#,
            &storage,
            &faults,
        );
        assert_eq!(put.status, 200);
        let put_json: Value = serde_json::from_slice(&put.body).unwrap();
        assert_eq!(put_json["ok"], true);
        let rev = put_json["rev"].as_str().unwrap().to_string();

        let get = handle_sync_request(
            "plugin-demo",
            "aitools-sync://demo/db/get",
            br#"{"id":"k1"}"#,
            &storage,
            &faults,
        );
        assert_eq!(get.status, 200);
        let get_json: Value = serde_json::from_slice(&get.body).unwrap();
        assert_eq!(get_json["_rev"], rev);

        // Fault injection triggers simulated LMDB conflict.
        faults.lock().unwrap().inject_lmdb_rev_conflict = true;
        let injected = handle_sync_request(
            "plugin-demo",
            "aitools-sync://demo/db/put",
            br#"{"_id":"k1","v":2}"#,
            &storage,
            &faults,
        );
        let inj_json: Value = serde_json::from_slice(&injected.body).unwrap();
        assert_eq!(inj_json["error"], true);
        assert_eq!(inj_json["name"], "conflict");
    }

    #[test]
    fn parse_plugin_url_rejects_invalid_plugin_ids() {
        let valid = parse_plugin_url("demo-1.0_a", "/index.html").unwrap();
        assert_eq!(valid.as_str(), "aitools-plugin://demo-1.0_a/index.html");

        assert!(matches!(
            parse_plugin_url("", "index.html"),
            Err(PluginError::InvalidPluginUrl { .. })
        ));
        assert!(matches!(
            parse_plugin_url("evil/path", "index.html"),
            Err(PluginError::InvalidPluginUrl { .. })
        ));
        assert!(matches!(
            parse_plugin_url("evil@host", "index.html"),
            Err(PluginError::InvalidPluginUrl { .. })
        ));
    }

    #[test]
    fn aitools_state_open_initializes_with_temp_dir() {
        let dir = TempDir::new().unwrap();
        let state = AitoolsState::open(dir.path(), None, &[]).unwrap();
        assert!(state.telemetry.is_none());
    }

    #[test]
    fn spec_012_r2_sdk_commands_enforce_plugin_isolation_and_persist_docs() {
        let dir = TempDir::new().unwrap();
        let storage = StorageEngine::open(dir.path()).unwrap();

        // Non-plugin caller webview labels are rejected.
        assert!(sdk_db_get(&storage, "main", "k1").is_err());

        let doc = Document {
            id: "clip-1".to_string(),
            rev: None,
            data: serde_json::json!({"text": "hello sdk"}),
        };
        let put = sdk_db_put(&storage, "plugin-smart-clip", doc).unwrap();
        assert_eq!(put.ok, Some(true));
        let rev = put.rev.expect("real rev must be returned");

        // Another plugin cannot read plugin-smart-clip's document.
        assert!(sdk_db_get(&storage, "plugin-other", "clip-1")
            .unwrap()
            .is_none());

        let fetched = sdk_db_get(&storage, "plugin-smart-clip", "clip-1")
            .unwrap()
            .expect("doc should exist for owner plugin");
        assert_eq!(fetched.rev.as_deref(), Some(rev.as_str()));
        assert_eq!(fetched.data["text"], "hello sdk");

        let all =
            sdk_db_all_docs(&storage, "plugin-smart-clip", Some("clip-".to_string())).unwrap();
        assert_eq!(all.len(), 1);

        let removed = sdk_db_remove(
            &storage,
            "plugin-smart-clip",
            "clip-1".to_string(),
            Some(rev),
        )
        .unwrap();
        assert_eq!(removed.ok, Some(true));
        assert!(sdk_db_get(&storage, "plugin-smart-clip", "clip-1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn spec_015_r4_webview_shim_sync_call_and_preload_proxy() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("shim-demo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"Shim Demo","version":"1.0.0","preload":"preload.js","features":[{"code":"run","cmds":["run"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("preload.js"),
            r#"window.demoApi = { greet: (name) => "hi:" + name };"#,
        )
        .unwrap();

        let state = AitoolsState::open(dir.path().join("db"), None, &[]).unwrap();
        let sync_res = state.sessions.sync_plugins_directory(dir.path());
        assert_eq!(sync_res.indexed_plugins, 1);

        let (_rt, info) = state
            .sessions
            .get_or_load_preload_session("plugin-ab4e")
            .or_else(|_| {
                let plugin_id = state.sessions.preload_status_summary().plugins[0]
                    .plugin_id
                    .clone();
                state.sessions.get_or_load_preload_session(&plugin_id)
            })
            .unwrap();
        let plugin_id = state.sessions.preload_status_summary().plugins[0]
            .plugin_id
            .clone();

        let script =
            build_initialization_script_with_preload_props(&plugin_id, &info.exported_props);
        assert!(
            script.contains("'ztools'")
                && script.contains("'utools'")
                && script.contains("'rubick'")
        );
        assert!(script.contains("__aitools_sync_call__"));
        assert!(script.contains("demoApi"));

        let label = format!("plugin-{plugin_id}");
        let uri = format!("aitools-sync://{plugin_id}/preload/invoke");
        let reply = handle_sync_request_with_session(
            &label,
            &uri,
            br#"{"propPath":"demoApi.greet","args":["world"]}"#,
            &state.sessions,
        );
        assert_eq!(reply.status, 200);
        let parsed: Value = serde_json::from_slice(&reply.body).unwrap();
        assert_eq!(parsed, Value::String("hi:world".to_string()));
    }

    #[test]
    fn spec_015_r5_sync_protocol_db_all_ops_attachments_and_promises() {
        let dir = TempDir::new().unwrap();
        let state = AitoolsState::open(dir.path(), None, &[]).unwrap();

        // Bulk insert mixed flat ZTools & envelope documents.
        let bulk = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/bulkDocs",
            br#"{"docs":[{"_id":"k1","note":"flat-note"},{"_id":"k2","data":{"nested":42}}]}"#,
            &state.sessions,
        );
        assert_eq!(bulk.status, 200);
        let bulk_json: Value = serde_json::from_slice(&bulk.body).unwrap();
        assert_eq!(bulk_json.as_array().unwrap().len(), 2);

        let all_by_keys = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/allDocs",
            br#"{"keys":["k2","missing"]}"#,
            &state.sessions,
        );
        assert_eq!(all_by_keys.status, 200);
        let docs: Value = serde_json::from_slice(&all_by_keys.body).unwrap();
        assert_eq!(docs.as_array().unwrap().len(), 1);
        assert_eq!(docs[0]["_id"], "k2");

        // Attachments put/get/type/remove roundtrip.
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"png-bytes-test");
        let put_att = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/putAttachment",
            format!(r#"{{"id":"img-1","data":"{b64}","type":"image/png"}}"#).as_bytes(),
            &state.sessions,
        );
        assert_eq!(put_att.status, 200);

        let get_att = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/getAttachment",
            br#"{"id":"img-1"}"#,
            &state.sessions,
        );
        assert_eq!(get_att.status, 200);
        let att_val: Value = serde_json::from_slice(&get_att.body).unwrap();
        assert_eq!(att_val["data"], b64);
        assert_eq!(att_val["contentType"], "image/png");

        let att_type = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/getAttachmentType",
            br#"{"id":"img-1"}"#,
            &state.sessions,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&att_type.body).unwrap(),
            Value::String("image/png".to_string())
        );

        let rm_att = handle_sync_request_with_session(
            "plugin-p1",
            "aitools-sync://p1/db/removeAttachment",
            br#"{"id":"img-1"}"#,
            &state.sessions,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&rm_att.body).unwrap()["ok"],
            json!(true)
        );
    }

    #[test]
    fn spec_015_r6_ztools_lifecycle_os_clipboard_shell_window_and_ai_apis() {
        let dir = TempDir::new().unwrap();
        let state = AitoolsState::open(dir.path(), None, &[]).unwrap();
        let mem_clip = Arc::new(aitools_native::context::MemoryClipboard::with_text("init"));
        state.sessions.set_clipboard_provider(mem_clip);
        let noop_ctl = Arc::new(NoopWindowController::default());
        state.sessions.set_window_controller(noop_ctl.clone());

        let cp = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/clipboard/copyText",
            br#"{"text":"copied-value"}"#,
            &state.sessions,
        );
        assert_eq!(cp.status, 200);
        let read = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/clipboard/readText",
            b"{}",
            &state.sessions,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&read.body).unwrap(),
            Value::String("copied-value".to_string())
        );

        let notif = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/notification/show",
            br#"{"body":"Hello ZTools"}"#,
            &state.sessions,
        );
        assert_eq!(notif.status, 200);
        assert_eq!(noop_ctl.notifications.lock().unwrap().len(), 1);

        let open_ext = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/shell/openExternal",
            br#"{"url":"https://example.com"}"#,
            &state.sessions,
        );
        assert_eq!(open_ext.status, 200);
        assert_eq!(
            noop_ctl.shell_actions.lock().unwrap()[0].1,
            "https://example.com"
        );

        let ai_models = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/ai/allModels",
            b"{}",
            &state.sessions,
        );
        assert_eq!(ai_models.status, 200);
        let models: Value = serde_json::from_slice(&ai_models.body).unwrap();
        assert!(models.as_array().unwrap().len() >= 3);

        let ai_resp = handle_sync_request_with_session(
            "plugin-z1",
            "aitools-sync://z1/ai/chat",
            br#"{"prompt":"ping","model":"fast"}"#,
            &state.sessions,
        );
        assert_eq!(ai_resp.status, 200);
        let chat_json: Value = serde_json::from_slice(&ai_resp.body).unwrap();
        assert_eq!(chat_json["backend"], "offline_mock");
    }

    #[test]
    fn spec_016_r2_launch_item_and_list_mode_dispatch_all_five_kinds() {
        let dir = TempDir::new().unwrap();
        let workspace_builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("plugins")
            .join("builtin");

        let ztools_list_dir = dir.path().join("list-demo");
        std::fs::create_dir_all(&ztools_list_dir).unwrap();
        std::fs::write(
            ztools_list_dir.join("plugin.json"),
            r#"{"name":"List Demo","version":"1.0.0","preload":"preload.js","features":[{"code":"items","cmds":["items"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            ztools_list_dir.join("preload.js"),
            r#"window.exports = {
                items: {
                    mode: "list",
                    args: {
                        enter: (action, callbackSetList) => {
                            callbackSetList([{ title: "Alpha", description: "first" }, { title: "Beta", description: "second" }]);
                        },
                        search: (action, word, callbackSetList) => {
                            const list = [{ title: "Alpha", description: "first" }, { title: "Beta", description: "second" }];
                            callbackSetList(list.filter(i => i.title.toLowerCase().includes(word.toLowerCase())));
                        },
                        select: (action, item, callbackSetList) => {
                            window.utools.outPlugin(false);
                            return "selected:" + item.title;
                        }
                    }
                }
            };"#,
        )
        .unwrap();

        let state = AitoolsState::open(dir.path().join("db"), None, &[]).unwrap();
        state
            .sessions
            .set_execution_mode(aitools_js_bridge::PreloadExecutionMode::PureRustOnly);
        let noop_ctl = Arc::new(NoopWindowController::default());
        state.sessions.set_window_controller(noop_ctl.clone());

        let builtin_sync = state.sessions.sync_plugins_directory(&workspace_builtin);
        assert_eq!(builtin_sync.indexed_plugins, 8);
        let custom_sync = state.sessions.sync_plugins_directory(dir.path());
        assert_eq!(custom_sync.indexed_plugins, 1);

        // 1. WebView plugin launch -> OpenedWindow
        let launched_ui = state
            .sessions
            .launch_item(
                "clipboard-history",
                "clipboard-history",
                "",
                None,
                None,
                None,
            )
            .unwrap();
        assert!(matches!(launched_ui, LaunchResult::OpenedWindow { .. }));

        // 2. Wasm plugin launch -> WasmResult (with manifest permission rejection check too!)
        let launched_wasm = state
            .sessions
            .launch_item("format_json", "json-codec", r#"{"ok":1}"#, None, None, None)
            .unwrap();
        match launched_wasm {
            LaunchResult::WasmResult { output, .. } => {
                assert_eq!(output["result"]["valid"], json!(true));
            }
            other => panic!("expected WasmResult, got {other:?}"),
        }

        // Manifest permission isolation on v2 plugin: `json-codec` has NO `db.write` or `network`.
        let denied = handle_sync_request_with_session(
            "plugin-json-codec",
            "aitools-sync://json-codec/db/put",
            br#"{"_id":"x","v":1}"#,
            &state.sessions,
        );
        assert_eq!(denied.status, 403);

        // 3. ZTools headless list mode (QuickJsNodeCompat PureRustOnly) -> ListMode -> search -> select
        let list_plugin_id = state
            .sessions
            .preload_status_summary()
            .plugins
            .into_iter()
            .find(|p| p.plugin_id.starts_with("list-demo"))
            .unwrap()
            .plugin_id;
        let launched_list = state
            .sessions
            .launch_item("items", &list_plugin_id, "", Some("list"), None, None)
            .unwrap();
        let session_id = match launched_list {
            LaunchResult::ListMode {
                session_id, items, ..
            } => {
                assert_eq!(items.len(), 2);
                session_id
            }
            other => panic!("expected ListMode, got {other:?}"),
        };
        let filtered = state
            .sessions
            .search_list_mode(&session_id, &list_plugin_id, "items", "beta", None)
            .unwrap();
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].title, "Beta");
        let selected = state
            .sessions
            .select_list_item(
                &session_id,
                &list_plugin_id,
                "items",
                json!({ "title": "Beta" }),
                0,
            )
            .unwrap();
        assert!(selected.ok);
        assert_eq!(selected.out_plugin, Some(true));

        // 4. Macro launch -> MacroStarted
        let mac = state
            .sessions
            .launch_item(
                "macro:daily",
                "builtin",
                "",
                Some("macro"),
                Some("tr-1"),
                None,
            )
            .unwrap();
        assert!(matches!(mac, LaunchResult::MacroStarted { .. }));

        // 5. DSH preload summary + PureRustOnly gap enforcement
        let gap_dir = TempDir::new().unwrap();
        let gap_plugin = gap_dir.path().join("gap-demo");
        std::fs::create_dir_all(&gap_plugin).unwrap();
        std::fs::write(
            gap_plugin.join("plugin.json"),
            r#"{"name":"Gap Demo","version":"1.0.0","preload":"preload.js","features":[{"code":"gap","cmds":["gap"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            gap_plugin.join("preload.js"),
            r#"const { Worker } = require('worker_threads'); window.gap = Worker;"#,
        )
        .unwrap();
        let gap_sync = state.sessions.sync_plugins_directory(gap_dir.path());
        assert_eq!(gap_sync.indexed_plugins, 1);
        let gap_err = state
            .sessions
            .get_or_load_preload_session("gap-demo")
            .unwrap_err();
        assert!(gap_err.message.contains("gap"));

        let dsh_summary = state.sessions.preload_status_summary();
        assert_eq!(dsh_summary.execution_mode, "PureRustOnly");
        assert!(dsh_summary.node_compat_gap_count >= 1);
        assert!(dsh_summary
            .plugins
            .iter()
            .any(|p| p.plugin_id == list_plugin_id && p.active_tier == "QuickJsNodeCompat"));
    }

    #[test]
    fn inv_sec_01_app_icon_protocol_enforces_host_allowlist_and_caches_png() {
        let dir = TempDir::new().unwrap();
        let cache_dir = dir.path().join("icon-cache");
        let plugin_dir = dir.path().join("demo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("index.html"), "<h1>demo</h1>").unwrap();

        let src_png = dir.path().join("sample.png");
        let img = aitools_native::app_catalog::render_app_icon_png_128;
        let _ = img;
        std::fs::write(
            &src_png,
            base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
                .unwrap(),
        )
        .unwrap();

        let mounts = PluginMountRegistry::new();
        mounts.set_icon_cache_dir(&cache_dir);
        mounts.register_root("demo", &plugin_dir).unwrap();
        let icon_uri = mounts.register_icon(&src_png).expect("icon uri");
        assert!(icon_uri.starts_with("aitools-plugin://__app_icon__/"));
        assert!(icon_uri.ends_with(".png"));

        // Host main & dsh can read registered 16-hex.png icons.
        let main_ok = handle_plugin_asset_request("main", &icon_uri, &mounts);
        assert_eq!(main_ok.status, 200);
        assert_eq!(main_ok.content_type, "image/png");
        assert!(main_ok.body.starts_with(b"\x89PNG\r\n\x1a\n"));

        let dsh_ok = handle_plugin_asset_request("dsh", &icon_uri, &mounts);
        assert_eq!(dsh_ok.status, 200);
        assert_eq!(dsh_ok.body, main_ok.body);

        let icon_id = icon_uri
            .strip_prefix("aitools-plugin://__app_icon__/")
            .unwrap()
            .strip_suffix(".png")
            .unwrap();
        assert!(cache_dir.join(format!("{icon_id}.png")).is_file());

        // Sandboxed plugin-* webviews MUST get 403 for __app_icon__!
        let plugin_blocked = handle_plugin_asset_request("plugin-demo", &icon_uri, &mounts);
        assert_eq!(plugin_blocked.status, 403);

        // main/dsh MUST get 403 for non-__app_icon__ hosts, unregistered hex IDs, or invalid paths.
        assert_eq!(
            handle_plugin_asset_request("main", "aitools-plugin://demo/index.html", &mounts).status,
            403
        );
        assert_eq!(
            handle_plugin_asset_request(
                "main",
                "aitools-plugin://__app_icon__/0000000000000000.png",
                &mounts
            )
            .status,
            403
        );
        assert_eq!(
            handle_plugin_asset_request(
                "main",
                "aitools-plugin://__app_icon__/../secret.png",
                &mounts
            )
            .status,
            403
        );
        assert_eq!(
            handle_plugin_asset_request(
                "main",
                "aitools-plugin://__app_icon__/nothex.png",
                &mounts
            )
            .status,
            403
        );
        // Existing plugin isolation stays intact.
        assert_eq!(
            handle_plugin_asset_request("plugin-demo", "aitools-plugin://demo/index.html", &mounts)
                .status,
            200
        );
        assert_eq!(
            handle_plugin_asset_request("plugin-evil", "aitools-plugin://demo/index.html", &mounts)
                .status,
            403
        );
    }

    #[test]
    fn spec_016_r1_featured_home_items_combines_plugins_and_apps_with_icon_uris() {
        let dir = TempDir::new().unwrap();
        let state = AitoolsState::open(dir.path().join("lmdb"), None, &[]).unwrap();

        let workspace_builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("plugins")
            .join("builtin");
        let _ = state.sessions.sync_plugins_directory(&workspace_builtin);

        // Add a local ZTools-style plugin with a PNG logo file.
        let z_dir = dir.path().join("ztools");
        let z_plugin = z_dir.join("z-sample");
        std::fs::create_dir_all(&z_plugin).unwrap();
        std::fs::write(
            z_plugin.join("plugin.json"),
            r#"{"name":"ZSample","title":"ZSample Tool","version":"1.0.0","logo":"logo.png","main":"index.html","features":[{"code":"z-open","explain":"Sample tool","cmds":["zsample"]}]}"#,
        )
        .unwrap();
        std::fs::write(z_plugin.join("index.html"), "<h1>z</h1>").unwrap();
        std::fs::write(
            z_plugin.join("logo.png"),
            base64::engine::general_purpose::STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
                .unwrap(),
        )
        .unwrap();
        let _ = state.sessions.sync_plugins_directory(&z_dir);

        // Register a mock app with icon_path.
        let app_icon = dir.path().join("Safari.png");
        std::fs::copy(z_plugin.join("logo.png"), &app_icon).unwrap();
        state
            .sessions
            .register_apps(vec![aitools_native::app_catalog::LauncherApp {
                name: "Safari".to_string(),
                path: "/System/Cryptexes/App/System/Applications/Safari.app".to_string(),
                aliases: vec!["com.apple.Safari".to_string()],
                bundle_id: Some("com.apple.Safari".to_string()),
                icon_path: Some(app_icon.to_string_lossy().into_owned()),
            }])
            .unwrap();

        let items = state.sessions.featured_home_items(27);
        assert!(items.len() >= 10);
        assert!(items.iter().any(|h| h.plugin_id == "clipboard-history"));
        let z_hit = items.iter().find(|h| h.plugin_id == "zsample").unwrap();
        assert!(z_hit
            .icon
            .as_deref()
            .is_some_and(|u| u.starts_with("aitools-plugin://__app_icon__/")));
        let safari_hit = items
            .iter()
            .find(|h| h.feature_code == "app:Safari")
            .unwrap();
        assert_eq!(safari_hit.kind.as_deref(), Some("app"));
        assert!(safari_hit
            .icon
            .as_deref()
            .is_some_and(|u| u.starts_with("aitools-plugin://__app_icon__/")));

        let capped = state.sessions.featured_home_items(3);
        assert_eq!(capped.len(), 3);

        // Host-only check: featured-home-items MUST be in default.toml and NOT in guest-sandbox.toml.
        let default_toml = include_str!("../permissions/default.toml");
        let guest_toml = include_str!("../permissions/guest-sandbox.toml");
        assert!(default_toml.contains("allow-featured-home-items"));
        assert!(!guest_toml.contains("featured-home-items"));
    }
}
