//! Plugin runtime session orchestration, host bridge handlers, and desktop dispatch (`SPEC-015` v2.0, `SPEC-016`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aitools_ai::broker::{ApiKeyProvider, ModelBroker};
use aitools_core::{
    current_ztools_platform, load_plugin_dir, Cmd, Feature, FeatureEntry, FeatureSearchIndex,
    LoadedPlugin, PluginRuntimeTier, StorageEngine,
};
use aitools_js_bridge::{
    ExportedWindowProp, HostBridgeHandler, JsBridgeError, ListItem, ListPushSink,
    PreloadCapabilities, PreloadExecutionMode, PreloadLoadInfo, PreloadRuntime, PreloadTier,
};
use aitools_native::app_catalog::LauncherApp;
use aitools_native::context::{ClipboardProvider, SystemClipboard};
use aitools_native::secrets::{KeyringSecretStore, SecretStore};
use aitools_telemetry::TelemetryHandle;
use aitools_wasm_host::WasmPluginHost;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::protocols::{FaultInjectionConfig, PluginMountRegistry};

pub(crate) const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
pub(crate) const MAX_CLIPBOARD_HISTORY: usize = 200;

/// Shared single-runtime Tokio handle for bridging synchronous host traits (`http_op`, `ai_op`) to async `reqwest`.
static SYNC_TOKIO_RT: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> =
    OnceLock::new();

pub(crate) fn block_on_async<F, T>(fut: F) -> std::result::Result<T, String>
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    let rt_res = SYNC_TOKIO_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("failed to build sync async bridge runtime: {e}"))
    });
    let rt = rt_res.as_ref().map_err(Clone::clone)?;

    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(|| rt.block_on(fut))
                .join()
                .map_err(|_| "async bridge thread panicked".to_string())
        })
    } else {
        Ok(rt.block_on(fut))
    }
}

/// Errors mapped to HTTP status codes in `aitools-sync://` or Tauri command strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOpError {
    /// HTTP status code (`400`, `403`, `404`, `409`, `413`, `500`).
    pub status: u16,
    /// Optional structured machine-readable error code (e.g. `"conflict"`).
    pub name: Option<String>,
    /// Human-readable error explanation.
    pub message: String,
}

impl SyncOpError {
    pub(crate) fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            name: None,
            message: message.into(),
        }
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self::new(403, message)
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, message)
    }
}

impl std::fmt::Display for SyncOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// [`ApiKeyProvider`] reading model API credentials from OS Keychain via `aitools-native` (`SPEC-016-R6`, `INV-SEC-03`).
#[derive(Debug, Default, Clone, Copy)]
pub struct KeychainApiKeyProvider;

impl ApiKeyProvider for KeychainApiKeyProvider {
    fn get_api_key(&self, account: &str) -> aitools_ai::Result<Option<String>> {
        let store = KeyringSecretStore::default();
        if let Ok(Some(val)) = store.get_secret(account) {
            if !val.trim().is_empty() {
                return Ok(Some(val));
            }
        }
        if let Some(stripped) = account.strip_prefix("aitools.ai.") {
            if let Ok(Some(val)) = store.get_secret(&format!("ai:{stripped}")) {
                if !val.trim().is_empty() {
                    return Ok(Some(val));
                }
            }
        }
        for env_key in [
            "AITOOLS_AI_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "DEEPSEEK_API_KEY",
        ] {
            if let Ok(val) = std::env::var(env_key) {
                if !val.trim().is_empty() {
                    return Ok(Some(val));
                }
            }
        }
        Ok(None)
    }
}

/// Tagged launch result returned by `plugin:aitools|launch_item` (`SPEC-016-R2`, `#18`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchResult {
    /// Sandbox WebView plugin window opened or focused.
    OpenedWindow {
        /// Plugin identifier.
        plugin_id: String,
        /// WebView window label (`plugin-<plugin_id>`).
        window_label: String,
        /// Whether the launcher main window should auto-hide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hide_main: Option<bool>,
    },
    /// Native desktop application launched via OS launcher.
    LaunchedApp {
        /// Display name of the launched application.
        app_name: String,
        /// Whether the launcher main window should auto-hide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hide_main: Option<bool>,
    },
    /// Headless ZTools list-mode plugin entered inside the main launcher window.
    ListMode {
        /// Active list session identifier.
        session_id: String,
        /// Owning plugin identifier.
        plugin_id: String,
        /// Human-readable plugin display name.
        plugin_name: String,
        /// Active feature code.
        feature_code: String,
        /// Optional placeholder text for the sub-input filter box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        /// Initial items pushed by `onPluginEnter` / `callbackSetList`.
        items: Vec<ListItem>,
    },
    /// Headless preload feature completed execution without opening a window.
    ExecutedHeadless {
        /// Owning plugin identifier.
        plugin_id: String,
        /// Executed feature code.
        feature_code: String,
        /// Optional human-readable summary message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Optional structured output payload.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        /// Whether the launcher main window should auto-hide.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hide_main: Option<bool>,
    },
    /// Zero-Webview Wasmtime plugin executed deterministically.
    WasmResult {
        /// Owning plugin identifier.
        plugin_id: String,
        /// Executed feature code or tool name.
        feature_code: String,
        /// Structured JSON execution output.
        output: Value,
    },
    /// Multi-step workflow macro started asynchronously.
    MacroStarted {
        /// Macro identifier.
        macro_id: String,
        /// Correlation trace identifier.
        trace_id: String,
    },
}

/// Response returned by `plugin:aitools|search_list_mode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListSearchResponse {
    /// Active list session identifier.
    pub session_id: String,
    /// Filtered list items returned for the query.
    pub items: Vec<ListItem>,
}

/// Response returned by `plugin:aitools|select_list_item`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectListResult {
    /// Whether selection handling succeeded.
    pub ok: bool,
    /// Whether the plugin requested exiting list/plugin mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_plugin: Option<bool>,
    /// Whether the main launcher window should hide after selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_main: Option<bool>,
    /// Optional status feedback message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Updated list items if the plugin navigated to a nested list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ListItem>>,
}

/// Summary returned by `plugin:aitools|sync_ztools_dir` (`SPEC-016-R1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncZtoolsResult {
    /// Directory path scanned for plugins.
    pub dir: String,
    /// Count of valid plugins indexed from `dir`.
    pub indexed_plugins: usize,
    /// Total features extracted across indexed plugins.
    pub indexed_features: usize,
    /// Non-fatal per-directory manifest load diagnostics.
    pub errors: Vec<String>,
}

/// Active preload runtime entry telemetry displayed in DSH (`SPEC-015-R2b`, `SPEC-016-R2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreloadPluginStatus {
    /// Plugin identifier.
    pub plugin_id: String,
    /// Active execution tier (`"WebViewOnly"`, `"QuickJsNodeCompat"`, or `"SystemNodeFallback"`).
    pub active_tier: String,
    /// Whether this plugin declares a CommonJS preload entry.
    pub has_preload: bool,
    /// Timestamp (Unix ms) of the most recent preload invocation, if loaded.
    pub last_used_ms: Option<u64>,
}

/// Snapshot returned by `plugin:aitools|dsh_preload_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPreloadSummary {
    /// Configured execution mode (`"PureRustOnly"` or `"RustFirstWithFallback"`).
    pub execution_mode: String,
    /// Cumulative number of Tier-3 `SystemNodeFallback` compatibility gap triggers.
    pub node_compat_gap_count: u64,
    /// Per-plugin preload runtime tier breakdown.
    pub plugins: Vec<PreloadPluginStatus>,
}

/// Specification passed to [`HostWindowController::open_plugin_window`].
pub struct PluginWindowSpec<'a> {
    /// Plugin identifier used for `plugin-<id>` window labeling.
    pub plugin_id: &'a str,
    /// Relative asset path or external `http(s)://` entry URL.
    pub entry_path: &'a str,
    /// Window title shown in native chrome.
    pub title: &'a str,
    /// Initial logical window width.
    pub width: f64,
    /// Initial logical window height.
    pub height: f64,
    /// Preload `window.<prop>` exports injected as RPC stubs (`INV-SEC-02`).
    pub exported_props: &'a [ExportedWindowProp],
    /// Optional `pluginEnter` action payload delivered after window launch.
    pub init_action: Option<Value>,
}

/// Core shared engine handles passed into [`PluginSessionManager::new`].
pub struct SessionDependencies<'a> {
    /// Persistent per-plugin LMDB storage engine.
    pub storage: Arc<StorageEngine>,
    /// Optional structured telemetry subscriber handle.
    pub telemetry: Option<Arc<TelemetryHandle>>,
    /// Plugin mount directory registry.
    pub mounts: PluginMountRegistry,
    /// Hybrid feature search index.
    pub search_index: Arc<RwLock<FeatureSearchIndex>>,
    /// Wasmtime plugin execution host.
    pub wasm_host: Arc<WasmPluginHost>,
    /// AI model & quota broker.
    pub model_broker: Arc<RwLock<ModelBroker>>,
    /// Runtime DSH fault injection switches.
    pub faults: Arc<Mutex<FaultInjectionConfig>>,
    /// Seed features retained across index rebuilds.
    pub initial_features: &'a [FeatureEntry],
}

/// Abstraction over desktop window, notification, dialog, and shell integrations for headless tests and Tauri live runs.
pub trait HostWindowController: Send + Sync {
    /// Opens or focuses a sandboxed plugin WebView window described by `spec`.
    ///
    /// # Errors
    ///
    /// Returns a descriptive error string if constructing or focusing the WebView fails.
    fn open_plugin_window(&self, spec: PluginWindowSpec<'_>)
        -> std::result::Result<String, String>;

    /// Closes the sandboxed WebView window for `plugin_id` if open.
    ///
    /// # Errors
    ///
    /// Returns an error string if the window close operation fails.
    fn close_plugin_window(&self, plugin_id: &str) -> std::result::Result<(), String>;

    /// Hides the main launcher window.
    ///
    /// # Errors
    ///
    /// Returns an error string if hiding fails.
    fn hide_main_window(&self) -> std::result::Result<(), String>;

    /// Shows and focuses the main launcher window.
    ///
    /// # Errors
    ///
    /// Returns an error string if showing fails.
    fn show_main_window(&self) -> std::result::Result<(), String>;

    /// Updates main launcher expand height.
    ///
    /// # Errors
    ///
    /// Returns an error string if resizing fails.
    fn set_expand_height(&self, height: f64) -> std::result::Result<(), String>;

    /// Emits an event payload to the main/dsh shell windows.
    fn emit_event(&self, event: &str, payload: Value);

    /// Evaluates a ZTools lifecycle hook on an open `plugin-<plugin_id>` WebView.
    fn eval_plugin_lifecycle(&self, plugin_id: &str, event: &str, arg1: Value, arg2: Value);

    /// Displays an OS notification.
    ///
    /// # Errors
    ///
    /// Returns an error string if the notification service rejects the request.
    fn show_notification(
        &self,
        title: &str,
        body: &str,
        click_feature_code: Option<&str>,
    ) -> std::result::Result<(), String>;

    /// Opens an external HTTP(S)/URI link via the OS default handler.
    ///
    /// # Errors
    ///
    /// Returns an error string if opening fails.
    fn open_external(&self, url: &str) -> std::result::Result<(), String>;

    /// Opens a filesystem path with the OS default application.
    ///
    /// # Errors
    ///
    /// Returns an error string if opening fails.
    fn open_path(&self, path: &str) -> std::result::Result<(), String>;

    /// Highlights a file or folder in Finder / Explorer / file manager.
    ///
    /// # Errors
    ///
    /// Returns an error string if revealing fails.
    fn show_item_in_folder(&self, path: &str) -> std::result::Result<(), String>;

    /// Plays the system alert beep.
    fn beep(&self);

    /// Shows a native open-file dialog if available.
    ///
    /// # Errors
    ///
    /// Returns an error string if the dialog backend fails.
    fn show_open_dialog(&self, options: &Value)
        -> std::result::Result<Option<Vec<String>>, String>;

    /// Shows a native save-file dialog if available.
    ///
    /// # Errors
    ///
    /// Returns an error string if the dialog backend fails.
    fn show_save_dialog(&self, options: &Value) -> std::result::Result<Option<String>, String>;
}

/// Headless fallback [`HostWindowController`] used in unit tests and CLI contexts.
#[derive(Debug, Default)]
pub struct NoopWindowController {
    /// Recorded notifications for deterministic verification.
    pub notifications: Mutex<Vec<(String, String)>>,
    /// Recorded opened plugin windows `(plugin_id, entry_path)`.
    pub opened_windows: Mutex<Vec<(String, String)>>,
    /// Recorded shell operations `(op, target)`.
    pub shell_actions: Mutex<Vec<(String, String)>>,
}

impl HostWindowController for NoopWindowController {
    fn open_plugin_window(
        &self,
        spec: PluginWindowSpec<'_>,
    ) -> std::result::Result<String, String> {
        if let Ok(mut guard) = self.opened_windows.lock() {
            guard.push((spec.plugin_id.to_string(), spec.entry_path.to_string()));
        }
        Ok(format!("plugin-{}", spec.plugin_id))
    }

    fn close_plugin_window(&self, _plugin_id: &str) -> std::result::Result<(), String> {
        Ok(())
    }

    fn hide_main_window(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    fn show_main_window(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    fn set_expand_height(&self, _height: f64) -> std::result::Result<(), String> {
        Ok(())
    }

    fn emit_event(&self, _event: &str, _payload: Value) {}

    fn eval_plugin_lifecycle(&self, _plugin_id: &str, _event: &str, _arg1: Value, _arg2: Value) {}

    fn show_notification(
        &self,
        title: &str,
        body: &str,
        _click_feature_code: Option<&str>,
    ) -> std::result::Result<(), String> {
        if let Ok(mut guard) = self.notifications.lock() {
            guard.push((title.to_string(), body.to_string()));
        }
        Ok(())
    }

    fn open_external(&self, url: &str) -> std::result::Result<(), String> {
        if let Ok(mut guard) = self.shell_actions.lock() {
            guard.push(("openExternal".to_string(), url.to_string()));
        }
        Ok(())
    }

    fn open_path(&self, path: &str) -> std::result::Result<(), String> {
        if let Ok(mut guard) = self.shell_actions.lock() {
            guard.push(("openPath".to_string(), path.to_string()));
        }
        Ok(())
    }

    fn show_item_in_folder(&self, path: &str) -> std::result::Result<(), String> {
        if let Ok(mut guard) = self.shell_actions.lock() {
            guard.push(("showItemInFolder".to_string(), path.to_string()));
        }
        Ok(())
    }

    fn beep(&self) {
        if let Ok(mut guard) = self.shell_actions.lock() {
            guard.push(("beep".to_string(), String::new()));
        }
    }

    fn show_open_dialog(
        &self,
        _options: &Value,
    ) -> std::result::Result<Option<Vec<String>>, String> {
        Ok(None)
    }

    fn show_save_dialog(&self, _options: &Value) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClipboardHistoryItem {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) content: String,
    pub(crate) timestamp: u64,
}

pub(crate) struct ActivePreloadSession {
    pub(crate) runtime: Arc<PreloadRuntime>,
    pub(crate) load_info: PreloadLoadInfo,
    pub(crate) tier: PreloadTier,
    pub(crate) last_used: Mutex<(Instant, u64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct ListModeSession {
    pub(crate) feature_code: String,
    pub(crate) items: Vec<ListItem>,
    pub(crate) out_plugin: bool,
    pub(crate) hide_main: bool,
}

#[derive(Debug)]
struct SessionHostBridge {
    manager: Weak<PluginSessionManager>,
}

impl HostBridgeHandler for SessionHostBridge {
    fn callback_set_list(
        &self,
        plugin_id: &str,
        items: Vec<ListItem>,
    ) -> aitools_js_bridge::Result<()> {
        let Some(mgr) = self.manager.upgrade() else {
            return Ok(());
        };
        mgr.handle_callback_set_list(plugin_id, items);
        Ok(())
    }

    fn db_op(&self, plugin_id: &str, op: &str, payload: Value) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("db/") {
            op.to_string()
        } else {
            format!("db/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn clipboard_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("clipboard/") {
            op.to_string()
        } else {
            format!("clipboard/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn notification_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("notification/") {
            op.to_string()
        } else {
            format!("notification/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn window_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("window/") {
            op.to_string()
        } else {
            format!("window/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn shell_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("shell/") {
            op.to_string()
        } else {
            format!("shell/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn system_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("system/") || op.starts_with("display/") {
            op.to_string()
        } else {
            format!("system/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn features_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("features/") {
            op.to_string()
        } else {
            format!("features/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn dialog_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("dialog/") {
            op.to_string()
        } else {
            format!("dialog/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn ai_op(&self, plugin_id: &str, op: &str, payload: Value) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("ai/") {
            op.to_string()
        } else {
            format!("ai/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn input_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let full_op = if op.starts_with("system/") || op.starts_with("input/") {
            op.to_string()
        } else {
            format!("system/{op}")
        };
        mgr.exec_sync_op(plugin_id, &full_op, payload, false)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }

    fn http_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mgr = self
            .manager
            .upgrade()
            .ok_or_else(|| JsBridgeError::HostRpc("session manager dropped".into()))?;
        let caps = mgr.resolve_preload_capabilities(plugin_id);
        if !caps.allow_net {
            return Err(JsBridgeError::HostRpc(format!(
                "permission denied (403): plugin '{plugin_id}' lacks network permission for {op}"
            )));
        }
        mgr.execute_http_request(plugin_id, op, &payload)
            .map_err(|e| JsBridgeError::HostRpc(e.message))
    }
}

/// Coordinates loaded plugins, 3-tier `PreloadRuntime` sessions, `aitools-sync://` dispatch,
/// clipboard/OS/dialog/notification bridges, and launcher item execution (`SPEC-015` v2.0, `SPEC-016`).
pub struct PluginSessionManager {
    pub(crate) self_weak: Mutex<Weak<PluginSessionManager>>,
    pub(crate) storage: Arc<StorageEngine>,
    pub(crate) telemetry: Option<Arc<TelemetryHandle>>,
    pub(crate) mounts: PluginMountRegistry,
    pub(crate) search_index: Arc<RwLock<FeatureSearchIndex>>,
    pub(crate) wasm_host: Arc<WasmPluginHost>,
    pub(crate) model_broker: Arc<RwLock<ModelBroker>>,
    pub(crate) faults: Arc<Mutex<FaultInjectionConfig>>,
    pub(crate) clipboard: RwLock<Arc<dyn ClipboardProvider>>,
    pub(crate) window_controller: RwLock<Arc<dyn HostWindowController>>,
    pub(crate) http_client: reqwest::Client,
    pub(crate) execution_mode: RwLock<PreloadExecutionMode>,
    pub(crate) base_features: RwLock<Vec<FeatureEntry>>,
    pub(crate) loaded_plugins: RwLock<HashMap<String, LoadedPlugin>>,
    pub(crate) app_catalog: RwLock<HashMap<String, LauncherApp>>,
    pub(crate) preload_sessions: Mutex<HashMap<String, Arc<ActivePreloadSession>>>,
    pub(crate) list_sessions: Mutex<HashMap<String, ListModeSession>>,
    pub(crate) active_list_session_by_plugin: Mutex<HashMap<String, String>>,
    pub(crate) active_list_sinks: Mutex<HashMap<String, Arc<dyn ListPushSink>>>,
    pub(crate) dynamic_features: RwLock<HashMap<String, Vec<Feature>>>,
    pub(crate) sub_input_values: RwLock<HashMap<String, String>>,
    pub(crate) window_flag_hints: Mutex<HashMap<String, (bool, bool)>>,
    pub(crate) active_ai_aborts: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub(crate) clipboard_history: Mutex<Vec<ClipboardHistoryItem>>,
    pub(crate) node_compat_gap_count: AtomicU64,
    pub(crate) session_seq: AtomicU64,
}

impl std::fmt::Debug for PluginSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginSessionManager")
            .field("has_telemetry", &self.telemetry.is_some())
            .field(
                "node_compat_gap_count",
                &self.node_compat_gap_count.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl PluginSessionManager {
    /// Creates a new [`PluginSessionManager`] wired to shared core engines.
    #[must_use]
    pub fn new(deps: SessionDependencies<'_>) -> Arc<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        let execution_mode = resolve_env_execution_mode();
        let manager = Arc::new(Self {
            self_weak: Mutex::new(Weak::new()),
            storage: deps.storage,
            telemetry: deps.telemetry,
            mounts: deps.mounts,
            search_index: deps.search_index,
            wasm_host: deps.wasm_host,
            model_broker: deps.model_broker,
            faults: deps.faults,
            clipboard: RwLock::new(Arc::new(SystemClipboard)),
            window_controller: RwLock::new(Arc::new(NoopWindowController::default())),
            http_client,
            execution_mode: RwLock::new(execution_mode),
            base_features: RwLock::new(deps.initial_features.to_vec()),
            loaded_plugins: RwLock::new(HashMap::new()),
            app_catalog: RwLock::new(HashMap::new()),
            preload_sessions: Mutex::new(HashMap::new()),
            list_sessions: Mutex::new(HashMap::new()),
            active_list_session_by_plugin: Mutex::new(HashMap::new()),
            active_list_sinks: Mutex::new(HashMap::new()),
            dynamic_features: RwLock::new(HashMap::new()),
            sub_input_values: RwLock::new(HashMap::new()),
            window_flag_hints: Mutex::new(HashMap::new()),
            active_ai_aborts: Mutex::new(HashMap::new()),
            clipboard_history: Mutex::new(Vec::new()),
            node_compat_gap_count: AtomicU64::new(0),
            session_seq: AtomicU64::new(0),
        });
        if let Ok(mut guard) = manager.self_weak.lock() {
            *guard = Arc::downgrade(&manager);
        }
        manager
    }

    /// Replaces the host window/dialog/notification controller (used on live Tauri setup).
    pub fn set_window_controller(&self, controller: Arc<dyn HostWindowController>) {
        if let Ok(mut guard) = self.window_controller.write() {
            *guard = controller;
        }
    }

    /// Replaces the clipboard provider (used in deterministic headless tests).
    pub fn set_clipboard_provider(&self, provider: Arc<dyn ClipboardProvider>) {
        if let Ok(mut guard) = self.clipboard.write() {
            *guard = provider;
        }
    }

    /// Overrides the preload execution mode (`PureRustOnly` vs `RustFirstWithFallback`).
    pub fn set_execution_mode(&self, mode: PreloadExecutionMode) {
        if let Ok(mut guard) = self.execution_mode.write() {
            *guard = mode;
        }
    }

    /// Returns the active [`PreloadExecutionMode`].
    #[must_use]
    pub fn execution_mode(&self) -> PreloadExecutionMode {
        self.execution_mode
            .read()
            .map_or(PreloadExecutionMode::RustFirstWithFallback, |g| *g)
    }

    pub(crate) fn window_controller(&self) -> Arc<dyn HostWindowController> {
        self.window_controller.read().map_or_else(
            |_| Arc::new(NoopWindowController::default()) as Arc<dyn HostWindowController>,
            |g| Arc::clone(&*g),
        )
    }

    pub(crate) fn clipboard(&self) -> Arc<dyn ClipboardProvider> {
        self.clipboard.read().map_or_else(
            |_| Arc::new(SystemClipboard) as Arc<dyn ClipboardProvider>,
            |g| Arc::clone(&*g),
        )
    }

    /// Registers a single pre-loaded [`LoadedPlugin`] and rebuilds the search index.
    ///
    /// # Errors
    ///
    /// Returns an error string if mounting the plugin root or rebuilding the search index fails.
    pub fn register_loaded_plugin(&self, plugin: LoadedPlugin) -> std::result::Result<(), String> {
        self.mounts
            .register_root(&plugin.id, &plugin.root_dir)
            .map_err(|e| format!("failed to mount plugin '{}': {e}", plugin.id))?;
        if let Ok(mut guard) = self.loaded_plugins.write() {
            guard.insert(plugin.id.clone(), plugin);
        }
        self.rebuild_search_index()
    }

    /// Registers desktop applications discovered by `AppCatalog` and rebuilds the search index.
    ///
    /// # Errors
    ///
    /// Returns an error string if rebuilding the search index fails.
    pub fn register_apps(&self, apps: Vec<LauncherApp>) -> std::result::Result<(), String> {
        if let Ok(mut guard) = self.app_catalog.write() {
            for app in apps {
                let code = format!("app:{}", app.name);
                guard.insert(code, app);
            }
        }
        self.rebuild_search_index()
    }

    /// Scans `dir` read-only for `aitools.manifest.json` / `plugin.json` plugins without mutating files (`INV-ZTOOLS-IMMUTABLE`).
    pub fn sync_plugins_directory(&self, dir: &Path) -> SyncZtoolsResult {
        let dir_str = dir.to_string_lossy().to_string();
        let mut errors = Vec::new();
        let mut indexed_plugins = 0usize;
        let mut indexed_features = 0usize;

        if !dir.is_dir() {
            return SyncZtoolsResult {
                dir: dir_str,
                indexed_plugins: 0,
                indexed_features: 0,
                errors: vec![format!(
                    "plugin directory does not exist: {}",
                    dir.display()
                )],
            };
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(err) => {
                return SyncZtoolsResult {
                    dir: dir_str,
                    indexed_plugins: 0,
                    indexed_features: 0,
                    errors: vec![format!("failed to read {}: {err}", dir.display())],
                };
            }
        };

        let mut loaded_batch = Vec::new();
        let mut subdirs: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|entry| entry.path()))
            .filter(|p| {
                p.is_dir()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
            })
            .collect();
        subdirs.sort();

        // Also allow syncing a single plugin directory directly when passed to `sync_ztools_dir`.
        if dir.join("plugin.json").is_file() || dir.join("aitools.manifest.json").is_file() {
            subdirs.clear();
            subdirs.push(dir.to_path_buf());
        }

        let platform = current_ztools_platform();
        for plugin_dir in subdirs {
            if !plugin_dir.join("plugin.json").is_file()
                && !plugin_dir.join("aitools.manifest.json").is_file()
            {
                continue;
            }
            match load_plugin_dir(&plugin_dir) {
                Ok(plugin) => {
                    if let Err(err) = self.mounts.register_root(&plugin.id, &plugin.root_dir) {
                        errors.push(format!("{}: mount failed: {err}", plugin.id));
                        continue;
                    }
                    indexed_plugins += 1;
                    indexed_features +=
                        plugin.to_feature_entries_for_platform(Some(platform)).len();
                    loaded_batch.push(plugin);
                }
                Err(err) => {
                    errors.push(format!("{}: {err}", plugin_dir.display()));
                }
            }
        }

        if let Ok(mut guard) = self.loaded_plugins.write() {
            for plugin in loaded_batch {
                guard.insert(plugin.id.clone(), plugin);
            }
        }
        if let Err(err) = self.rebuild_search_index() {
            errors.push(format!("search index rebuild failed: {err}"));
        }

        SyncZtoolsResult {
            dir: dir_str,
            indexed_plugins,
            indexed_features,
            errors,
        }
    }

    /// Rebuilds the shared [`FeatureSearchIndex`] from base features, registered apps, loaded plugins, and dynamic features.
    ///
    /// # Errors
    ///
    /// Returns an error string if compiling any feature command regex fails or locks are poisoned.
    pub fn rebuild_search_index(&self) -> std::result::Result<(), String> {
        let mut all_features: Vec<FeatureEntry> = self
            .base_features
            .read()
            .map_err(|_| "base features lock poisoned".to_string())?
            .clone();

        if let Ok(apps) = self.app_catalog.read() {
            let mut app_list: Vec<(&String, &LauncherApp)> = apps.iter().collect();
            app_list.sort_by(|a, b| a.0.cmp(b.0));
            for (code, app) in app_list {
                let mut cmds = vec![Cmd::Text(app.name.clone())];
                for alias in &app.aliases {
                    if !alias.eq_ignore_ascii_case(&app.name) {
                        cmds.push(Cmd::Text(alias.clone()));
                    }
                }
                let explain = app
                    .bundle_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map_or_else(|| app.path.clone(), ToString::to_string);
                all_features.push(FeatureEntry {
                    code: code.clone(),
                    plugin_id: "system-apps".to_string(),
                    plugin_name: "Application".to_string(),
                    label: app.name.clone(),
                    icon: None,
                    explain,
                    cmds,
                    main_push: false,
                    priority: 75,
                });
            }
        }

        let platform = current_ztools_platform();
        let dyn_map = self
            .dynamic_features
            .read()
            .map_err(|_| "dynamic features lock poisoned".to_string())?;
        if let Ok(plugins) = self.loaded_plugins.read() {
            let mut sorted_ids: Vec<&String> = plugins.keys().collect();
            sorted_ids.sort();
            for id in sorted_ids {
                if let Some(plugin) = plugins.get(id) {
                    all_features.extend(plugin.to_feature_entries_for_platform(Some(platform)));
                    if let Some(extra) = dyn_map.get(id) {
                        for feat in extra {
                            all_features.push(FeatureEntry {
                                code: feat.code.clone(),
                                plugin_id: plugin.id.clone(),
                                plugin_name: plugin.name.clone(),
                                label: feat.label.clone().unwrap_or_else(|| plugin.name.clone()),
                                icon: feat.icon.clone().or_else(|| plugin.icon.clone()),
                                explain: feat.explain.clone(),
                                cmds: feat.cmds.clone(),
                                main_push: feat.main_push,
                                priority: 95,
                            });
                        }
                    }
                }
            }
        }

        let next_index = FeatureSearchIndex::try_build(&all_features).map_err(|e| e.to_string())?;
        let mut idx_guard = self
            .search_index
            .write()
            .map_err(|_| "search index lock poisoned".to_string())?;
        *idx_guard = next_index;
        Ok(())
    }

    /// Resolves accurate `LauncherHit.kind` hints from loaded plugin manifests (`"app"`, `"wasm"`, `"list"`, `"plugin"`).
    #[must_use]
    pub fn classify_hit_kind(&self, plugin_id: &str, feature_code: &str) -> Option<String> {
        if plugin_id == "system-apps" || feature_code.starts_with("app:") {
            return Some("app".to_string());
        }
        if feature_code.starts_with("macro:") {
            return Some("macro".to_string());
        }
        if let Ok(plugins) = self.loaded_plugins.read() {
            if let Some(plugin) = plugins.get(plugin_id) {
                if plugin.tier == PluginRuntimeTier::RustWasm
                    || (plugin.wasm_entry.is_some() && plugin.main.is_none())
                {
                    return Some("wasm".to_string());
                }
                if plugin.main.is_some() {
                    return Some("plugin".to_string());
                }
                if plugin.preload.is_some() {
                    return Some("list".to_string());
                }
            }
        }
        None
    }

    /// Returns a loaded plugin descriptor by ID, if registered.
    #[must_use]
    pub fn loaded_plugin(&self, plugin_id: &str) -> Option<LoadedPlugin> {
        self.loaded_plugins
            .read()
            .ok()
            .and_then(|g| g.get(plugin_id).cloned())
    }

    /// Derives [`PreloadCapabilities`] from `plugin_id`'s manifest trust tier (`SPEC-015-R2`, `SPEC-016-R2`).
    #[must_use]
    pub fn resolve_preload_capabilities(&self, plugin_id: &str) -> PreloadCapabilities {
        let Ok(plugins) = self.loaded_plugins.read() else {
            return PreloadCapabilities::all();
        };
        let Some(plugin) = plugins.get(plugin_id) else {
            return PreloadCapabilities::all();
        };
        if plugin.v2.is_some() {
            return PreloadCapabilities {
                allow_fs: has_any_perm(&plugin.permissions, &["fs", "fs.*", "fs.read", "fs.write"]),
                allow_net: has_any_perm(&plugin.permissions, &["network", "net", "net.*", "http"]),
                allow_child_process: has_any_perm(
                    &plugin.permissions,
                    &["shell.exec", "child_process", "process.exec"],
                ),
            };
        }
        PreloadCapabilities::all()
    }

    /// Checks manifest permission declarations for v2 `aitools.manifest.json` plugins (`SPEC-016-R4`).
    ///
    /// Legacy v1 ZTools `plugin.json` manifests (which do not carry an `aitools.manifest.json` `permissions`
    /// array) retain compatibility across standard ZTools ops while sandboxed capability boundaries enforce isolation.
    ///
    /// # Errors
    ///
    /// Returns [`SyncOpError`] with status `403` when a v2 manifest lacks the permission required by `op`.
    pub fn check_sync_permission(
        &self,
        plugin_id: &str,
        op: &str,
    ) -> std::result::Result<(), SyncOpError> {
        let Ok(plugins) = self.loaded_plugins.read() else {
            return Ok(());
        };
        let Some(plugin) = plugins.get(plugin_id) else {
            return Ok(());
        };
        if plugin.v2.is_none() {
            return Ok(());
        }
        let perms = &plugin.permissions;
        let required: &[&str] = match op {
            "db/get" | "db/allDocs" | "db/getAttachment" | "db/getAttachmentType" => {
                &["db.read", "db.*", "db"]
            }
            "db/put" | "db/remove" | "db/bulkDocs" | "db/putAttachment" | "db/removeAttachment" => {
                &["db.write", "db.*", "db"]
            }
            "clipboard/readText"
            | "clipboard/readImage"
            | "clipboard/getCopyedFiles"
            | "clipboard/getHistory"
            | "clipboard/search" => &["clipboard.read", "clipboard.*", "clipboard"],
            "clipboard/copyText"
            | "clipboard/copyImage"
            | "clipboard/copyFile"
            | "clipboard/paste"
            | "clipboard/deleteItem"
            | "clipboard/clearHistory" => &["clipboard.write", "clipboard.*", "clipboard"],
            "notification/show" => &["notification", "notifications"],
            "shell/openExternal" | "shell/openPath" | "shell/showItemInFolder" => {
                &["shell.open", "shell.exec", "shell.*", "opener"]
            }
            "shell/beep" => &[],
            s if s.starts_with("shell/") => &["shell.exec", "shell.*"],
            "ai/chat" => &["ai.fast", "ai.balanced", "ai.deep", "ai.*"],
            "ai/allModels" | "ai/abort" => &["ai.fast", "ai.balanced", "ai.deep", "ai.*"],
            s if s.starts_with("http/") => &["network", "net", "net.*", "http"],
            _ => &[],
        };
        if required.is_empty() || has_any_perm(perms, required) {
            return Ok(());
        }
        Err(SyncOpError::forbidden(format!(
            "permission denied (403): plugin '{plugin_id}' manifest does not declare any of {:?}",
            required
        )))
    }

    fn build_bridge_handler(&self) -> Arc<dyn HostBridgeHandler> {
        let weak = self
            .self_weak
            .lock()
            .map_or_else(|_| Weak::new(), |g| g.clone());
        Arc::new(SessionHostBridge { manager: weak })
    }

    /// Loads (or returns cached) 3-tier [`PreloadRuntime`] session for `plugin_id` (`SPEC-015-R2`).
    ///
    /// # Errors
    ///
    /// Returns [`SyncOpError`] if the plugin has no preload, cannot be loaded under active mode, or locks fail.
    pub fn get_or_load_preload_session(
        &self,
        plugin_id: &str,
    ) -> std::result::Result<(Arc<PreloadRuntime>, PreloadLoadInfo), SyncOpError> {
        let existing = {
            let guard = self
                .preload_sessions
                .lock()
                .map_err(|_| SyncOpError::new(500, "preload session map lock poisoned"))?;
            guard.get(plugin_id).cloned()
        };
        if let Some(entry) = existing {
            if entry.runtime.is_alive() {
                if let Ok(mut ts) = entry.last_used.lock() {
                    *ts = (Instant::now(), now_unix_ms());
                }
                return Ok((Arc::clone(&entry.runtime), entry.load_info.clone()));
            }
        }
        self.load_fresh_preload_session(plugin_id)
    }

    fn load_fresh_preload_session(
        &self,
        plugin_id: &str,
    ) -> std::result::Result<(Arc<PreloadRuntime>, PreloadLoadInfo), SyncOpError> {
        let plugin = self
            .loaded_plugin(plugin_id)
            .ok_or_else(|| SyncOpError::not_found(format!("plugin '{plugin_id}' is not loaded")))?;
        let preload_rel = plugin.preload.as_deref().ok_or_else(|| {
            SyncOpError::not_found(format!("plugin '{plugin_id}' declares no preload script"))
        })?;
        let preload_path = plugin.root_dir.join(preload_rel);
        if !preload_path.is_file() {
            return Err(SyncOpError::not_found(format!(
                "preload file missing: {}",
                preload_path.display()
            )));
        }

        let mode = self.execution_mode();
        let caps = self.resolve_preload_capabilities(plugin_id);
        let runtime = Arc::new(PreloadRuntime::new(mode, caps, self.build_bridge_handler()));
        let load_info = match runtime.load(&preload_path, plugin_id, &plugin.root_dir) {
            Ok(info) => info,
            Err(JsBridgeError::NodeCompatGap { capability, .. }) => {
                self.node_compat_gap_count.fetch_add(1, Ordering::Relaxed);
                warn!(
                    target: "aitools::node_compat_gap",
                    plugin_id = %plugin_id,
                    capability = %capability,
                    mode = ?mode,
                    "QuickJS node compatibility gap blocked or deferred preload"
                );
                return Err(SyncOpError::new(
                    500,
                    format!("preload node compat gap ({capability}) for plugin '{plugin_id}'"),
                ));
            }
            Err(err) => {
                return Err(SyncOpError::new(
                    500,
                    format!("failed to load preload for '{plugin_id}': {err}"),
                ));
            }
        };

        let tier = runtime.active_tier();
        if tier == PreloadTier::SystemNodeFallback {
            self.node_compat_gap_count.fetch_add(1, Ordering::Relaxed);
            warn!(
                target: "aitools::node_compat_gap",
                plugin_id = %plugin_id,
                "plugin fell back to Tier 3 SystemNodeFallback"
            );
        }

        let entry = Arc::new(ActivePreloadSession {
            runtime: Arc::clone(&runtime),
            load_info: load_info.clone(),
            tier,
            last_used: Mutex::new((Instant::now(), now_unix_ms())),
        });
        if let Ok(mut guard) = self.preload_sessions.lock() {
            guard.insert(plugin_id.to_string(), entry);
        }
        Ok((runtime, load_info))
    }

    /// Shuts down preload sessions that have been idle longer than `idle_ttl`.
    pub fn evict_idle_sessions(&self, idle_ttl: Duration) -> usize {
        let now = Instant::now();
        let expired: Vec<Arc<PreloadRuntime>> = match self.preload_sessions.lock() {
            Ok(mut guard) => {
                let mut to_remove = Vec::new();
                for (id, entry) in guard.iter() {
                    if let Ok(ts) = entry.last_used.lock() {
                        if now.duration_since(ts.0) >= idle_ttl {
                            to_remove.push(id.clone());
                        }
                    }
                }
                to_remove
                    .into_iter()
                    .filter_map(|id| guard.remove(&id).map(|e| Arc::clone(&e.runtime)))
                    .collect()
            }
            Err(_) => return 0,
        };
        let count = expired.len();
        for rt in expired {
            let _ = rt.plugin_out(true);
            let _ = rt.shutdown();
        }
        count
    }

    /// Shuts down all active preload sidecars cleanly (called on desktop exit).
    pub fn shutdown_all(&self) {
        let entries: Vec<Arc<PreloadRuntime>> = match self.preload_sessions.lock() {
            Ok(mut guard) => guard.drain().map(|(_, e)| Arc::clone(&e.runtime)).collect(),
            Err(_) => Vec::new(),
        };
        for rt in entries {
            let _ = rt.plugin_out(true);
            let _ = rt.shutdown();
        }
    }

    /// Returns a live snapshot of execution mode, Tier-3 gap counts, and active preload tiers for DSH.
    #[must_use]
    pub fn preload_status_summary(&self) -> DshPreloadSummary {
        let mode = match self.execution_mode() {
            PreloadExecutionMode::PureRustOnly => "PureRustOnly",
            PreloadExecutionMode::RustFirstWithFallback => "RustFirstWithFallback",
        };
        let mut plugins_out = Vec::new();
        let active_map: HashMap<String, (PreloadTier, u64)> = self
            .preload_sessions
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(k, v)| {
                        let ts = v.last_used.lock().map_or(0, |t| t.1);
                        (k.clone(), (v.tier, ts))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if let Ok(plugins) = self.loaded_plugins.read() {
            let mut ids: Vec<&String> = plugins.keys().collect();
            ids.sort();
            for id in ids {
                if let Some(p) = plugins.get(id) {
                    let (tier, last_used_ms) = if let Some((t, ts)) = active_map.get(id) {
                        (format_tier(*t), Some(*ts))
                    } else if p.preload.is_some() {
                        ("QuickJsNodeCompat (ready)".to_string(), None)
                    } else {
                        ("WebViewOnly".to_string(), None)
                    };
                    plugins_out.push(PreloadPluginStatus {
                        plugin_id: id.clone(),
                        active_tier: tier,
                        has_preload: p.preload.is_some(),
                        last_used_ms,
                    });
                }
            }
        }

        DshPreloadSummary {
            execution_mode: mode.to_string(),
            node_compat_gap_count: self.node_compat_gap_count.load(Ordering::Relaxed),
            plugins: plugins_out,
        }
    }
}

pub(crate) fn resolve_env_execution_mode() -> PreloadExecutionMode {
    match std::env::var("AITOOLS_PURE_RUST_PRELOAD") {
        Ok(val)
            if matches!(
                val.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            ) =>
        {
            PreloadExecutionMode::PureRustOnly
        }
        _ => PreloadExecutionMode::RustFirstWithFallback,
    }
}

fn format_tier(tier: PreloadTier) -> String {
    match tier {
        PreloadTier::WebViewOnly => "WebViewOnly".to_string(),
        PreloadTier::QuickJsNodeCompat => "QuickJsNodeCompat".to_string(),
        PreloadTier::SystemNodeFallback => "SystemNodeFallback".to_string(),
    }
}

fn has_any_perm(perms: &[String], accepted: &[&str]) -> bool {
    perms.iter().any(|p| {
        accepted
            .iter()
            .any(|candidate| p.eq_ignore_ascii_case(candidate))
    })
}

pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
