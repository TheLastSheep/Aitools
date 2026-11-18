//! CommonJS Node.js preload sidecar (`NodePreloadRuntime`) & bidirectional host RPC (`SPEC-015-R2`, `SPEC-015-R3`).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use tracing::{debug, info_span, warn};

use crate::callback_proxy::ListItem;
use crate::shim::TRIPLE_GLOBAL_SHIM_JS;
use crate::{JsBridgeError, Result};

const NODE_PRELOAD_RUNNER_CJS: &str = include_str!("node_preload_runner.cjs");

/// Default maximum wait for Node.js process spawn and readiness handshake.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Default maximum wait for each preload lifecycle or `window.*` invocation call.
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Brief grace window before `kill()` during `shutdown` / `Drop` teardown.
const SHUTDOWN_GRACE_TIMEOUT: Duration = Duration::from_millis(300);

/// Configuration for [`NodePreloadRuntime`] binary discovery and execution timeouts (`SPEC-015-R2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRuntimeConfig {
    /// Explicit path to a `node` executable; when `None`, [`resolve_node_binary`] is used.
    pub node_binary: Option<PathBuf>,
    /// Maximum wait time for Node process startup and handshake.
    pub startup_timeout: Duration,
    /// Maximum wait time for each lifecycle or `window.*` invocation call.
    pub call_timeout: Duration,
}

impl Default for NodeRuntimeConfig {
    fn default() -> Self {
        Self {
            node_binary: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }
}

impl NodeRuntimeConfig {
    /// Creates a [`NodeRuntimeConfig`] with default timeouts and automatic `node` discovery.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Descriptor of a property attached to `window` by a plugin's `preload.js` (`SPEC-015-R3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportedWindowProp {
    /// A callable function attached to `window` or a nested namespace.
    Function {
        /// Dot-separated property path relative to `window` (for example, `"charTransformApi.transform"`).
        path: String,
    },
    /// A plain JSON-serializable value attached to `window`.
    Value {
        /// Dot-separated property path relative to `window` (for example, `"charTransformApi.modes"`).
        path: String,
        /// JSON-serialized snapshot of the value.
        value: Value,
    },
    /// A namespace object containing nested functions and/or data values.
    Object {
        /// Dot-separated property path relative to `window` (for example, `"charTransformApi"`).
        path: String,
        /// Child property descriptors.
        entries: Vec<ExportedWindowProp>,
    },
}

/// Metadata for a feature handler registered on `window.exports` or `module.exports`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedFeatureInfo {
    /// Feature code key inside `window.exports`.
    pub code: String,
    /// Declared mode string (for example, `"list"`, `"none"`, or `"doc"`).
    pub mode: String,
}

/// Summary returned by [`NodePreloadRuntime::load`] after evaluating `preload.js` (`SPEC-015-R3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreloadLoadInfo {
    /// Properties attached to `window` during `require(preloadPath)`.
    pub exported_props: Vec<ExportedWindowProp>,
    /// Feature handlers exported via `window.exports` or `module.exports`.
    pub export_features: Vec<ExportedFeatureInfo>,
    /// Whether `onPluginEnter` was registered during preload initialization.
    pub has_on_plugin_enter: bool,
    /// Whether `onMainPush` was registered during preload initialization.
    pub has_on_main_push: bool,
    /// Whether `onPluginOut` was registered during preload initialization.
    pub has_on_plugin_out: bool,
}

/// Bidirectional host capability handler invoked by [`NodePreloadRuntime`] over NDJSON stdio (`SPEC-015-R3`).
pub trait HostBridgeHandler: Send + Sync + 'static {
    /// Streams list-mode items pushed via `callbackSetList(items)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the host fails to forward or record the list items.
    fn callback_set_list(&self, plugin_id: &str, items: Vec<ListItem>) -> Result<()> {
        let _ = (plugin_id, items);
        Ok(())
    }

    /// Handles `db/*` operations (`db/get`, `db/put`, `db/remove`, `db/allDocs`, `db/bulkDocs`,
    /// `db/putAttachment`, `db/getAttachment`, `db/getAttachmentType`, `db/removeAttachment`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the operation is unsupported or fails on the host.
    fn db_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!("unsupported db op: {op}")))
    }

    /// Handles `clipboard/*` operations (`clipboard/copyText`, `clipboard/copyImage`,
    /// `clipboard/copyFile`, `clipboard/getCopyedFiles`, `clipboard/readText`, `clipboard/readImage`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the operation is unsupported or fails on the host.
    fn clipboard_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported clipboard op: {op}"
        )))
    }

    /// Handles `notification/*` requests (`notification/show` with `{ body, clickFeatureCode }`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when delivering the notification fails.
    fn notification_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Ok(Value::Bool(true))
    }

    /// Handles `window/*` operations (`window/setSubInput`, `window/setSubInputValue`,
    /// `window/removeSubInput`, `window/redirect`, `window/outPlugin`, `window/hideMainWindow`,
    /// `window/showMainWindow`, `window/setExpendHeight`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the window operation fails.
    fn window_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Ok(Value::Bool(true))
    }

    /// Handles `shell/*` operations (`shell/openExternal`, `shell/openPath`,
    /// `shell/showItemInFolder`, `shell/beep`, `shell/trashItem`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the shell operation fails or is unsupported.
    fn shell_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported shell op: {op}"
        )))
    }

    /// Handles `system/*` and `display/*` operations (`system/getPath`, `system/getAppName`,
    /// `system/getAppVersion`, `system/getNativeId`, `system/isDarkColors`, `system/screenCapture`,
    /// `system/readCurrentBrowserUrl`, `system/readCurrentFolderPath`, `display/*`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the system or display query fails or is unsupported.
    fn system_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported system op: {op}"
        )))
    }

    /// Handles `features/*` operations (`features/getFeatures`, `features/setFeature`,
    /// `features/removeFeature`, `features/registerMainPush`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the feature operation fails or is unsupported.
    fn features_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported features op: {op}"
        )))
    }

    /// Handles `dialog/*` operations (`dialog/showOpenDialog`, `dialog/showSaveDialog`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the dialog operation fails or is unsupported.
    fn dialog_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported dialog op: {op}"
        )))
    }

    /// Handles `ai/*` operations (`ai/chat`, `ai/allModels`, `ai/abort`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the AI operation fails or is unsupported.
    fn ai_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!("unsupported ai op: {op}")))
    }

    /// Handles input simulation operations (`system/simulateKeyboardTap`,
    /// `system/simulateMouseMove`, `system/simulateMouseClick`, `input/*`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when input simulation fails or is unsupported.
    fn input_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!(
            "unsupported input op: {op}"
        )))
    }

    /// Handles `http/*` network operations (`http/request`, `http/fetch`) gated by [`crate::PreloadCapabilities::allow_net`].
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::HostRpc`] when the network request fails or is unsupported.
    fn http_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        let _ = (plugin_id, op, payload);
        Err(JsBridgeError::HostRpc(format!("unsupported http op: {op}")))
    }
}

/// Safe zero-side-effect [`HostBridgeHandler`] returning deterministic system defaults for smoke runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHostBridgeHandler;

impl HostBridgeHandler for NoopHostBridgeHandler {
    fn db_op(&self, _plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        match op {
            "db/get" | "db/getAttachment" | "db/getAttachmentType" => Ok(Value::Null),
            "db/allDocs" | "db/bulkDocs" => Ok(Value::Array(Vec::new())),
            "db/put" | "db/remove" | "db/putAttachment" | "db/removeAttachment" => {
                let id = payload
                    .get("_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Ok(json!({ "id": id, "ok": true }))
            }
            _ => Ok(Value::Null),
        }
    }

    fn clipboard_op(&self, _plugin_id: &str, op: &str, _payload: Value) -> Result<Value> {
        match op {
            "clipboard/getCopyedFiles" => Ok(Value::Array(Vec::new())),
            "clipboard/readText" | "clipboard/readHTML" => Ok(Value::String(String::new())),
            "clipboard/readImage" => Ok(Value::Null),
            _ => Ok(Value::Bool(true)),
        }
    }

    fn shell_op(&self, _plugin_id: &str, _op: &str, _payload: Value) -> Result<Value> {
        Ok(Value::Bool(true))
    }

    fn system_op(&self, plugin_id: &str, op: &str, payload: Value) -> Result<Value> {
        match op {
            "system/getAppName" => Ok(Value::String("aitools".to_string())),
            "system/getAppVersion" => Ok(Value::String(env!("CARGO_PKG_VERSION").to_string())),
            "system/getNativeId" => Ok(Value::String(format!("aitools-{plugin_id}"))),
            "system/isDev" => Ok(Value::Bool(cfg!(debug_assertions))),
            "system/isMacOs" => Ok(Value::Bool(cfg!(target_os = "macos"))),
            "system/isWindows" => Ok(Value::Bool(cfg!(target_os = "windows"))),
            "system/isLinux" => Ok(Value::Bool(cfg!(target_os = "linux"))),
            "system/isDarkColors" => Ok(Value::Bool(false)),
            "system/getThemeInfo" => Ok(json!({ "isDark": false, "primaryColor": "#7c3aed" })),
            "system/getPath" => {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("temp");
                Ok(Value::String(
                    resolve_default_path(name).to_string_lossy().into_owned(),
                ))
            }
            "system/getPathForFile" | "system/getFileIcon" | "system/screenCapture" => {
                Ok(Value::String(String::new()))
            }
            "system/readCurrentBrowserUrl" | "system/readCurrentFolderPath" => {
                Ok(Value::String(String::new()))
            }
            "display/getPrimaryDisplay" | "display/getDisplayNearestPoint" => Ok(json!({
                "id": 1,
                "bounds": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "workArea": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "scaleFactor": 2.0
            })),
            "display/getAllDisplays" => Ok(json!([{
                "id": 1,
                "bounds": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "workArea": { "x": 0, "y": 0, "width": 1920, "height": 1080 },
                "scaleFactor": 2.0
            }])),
            "display/getCursorScreenPoint" => Ok(json!({ "x": 0, "y": 0 })),
            "display/dipToScreenPoint" | "display/screenToDipPoint" => Ok(payload),
            _ => Ok(Value::Null),
        }
    }

    fn features_op(&self, _plugin_id: &str, op: &str, _payload: Value) -> Result<Value> {
        match op {
            "features/getFeatures" => Ok(Value::Array(Vec::new())),
            _ => Ok(Value::Bool(true)),
        }
    }

    fn dialog_op(&self, _plugin_id: &str, _op: &str, _payload: Value) -> Result<Value> {
        Ok(Value::Null)
    }

    fn ai_op(&self, _plugin_id: &str, op: &str, _payload: Value) -> Result<Value> {
        match op {
            "ai/allModels" => Ok(Value::Array(Vec::new())),
            _ => Ok(Value::Null),
        }
    }

    fn input_op(&self, _plugin_id: &str, _op: &str, _payload: Value) -> Result<Value> {
        Ok(Value::Bool(true))
    }
}

fn resolve_default_path(name: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    match name {
        "home" => home,
        "appData" => dirs::data_dir().unwrap_or_else(|| home.clone()),
        "userData" => dirs::data_local_dir().unwrap_or_else(|| home.clone()),
        "desktop" => dirs::desktop_dir().unwrap_or_else(|| home.join("Desktop")),
        "documents" => dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
        "downloads" => dirs::download_dir().unwrap_or_else(|| home.join("Downloads")),
        "music" => dirs::audio_dir().unwrap_or_else(|| home.join("Music")),
        "pictures" => dirs::picture_dir().unwrap_or_else(|| home.join("Pictures")),
        "videos" => dirs::video_dir().unwrap_or_else(|| home.join("Videos")),
        _ => std::env::temp_dir(),
    }
}

type PendingReplies = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value>>>>>;

struct ActiveChild {
    plugin_id: String,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Mutex<Child>,
    pending: PendingReplies,
    next_req_id: AtomicU64,
    call_lock: Mutex<()>,
    alive: Arc<AtomicBool>,
    stdout_thread: Mutex<Option<JoinHandle<()>>>,
    stderr_thread: Mutex<Option<JoinHandle<()>>>,
    _runner_script: NamedTempFile,
}

/// Manages an isolated CommonJS Node.js preload sidecar process with bidirectional NDJSON stdio RPC (`SPEC-015-R2`, `SPEC-015-R3`).
pub struct NodePreloadRuntime {
    config: NodeRuntimeConfig,
    handler: Arc<dyn HostBridgeHandler>,
    active: Mutex<Option<Arc<ActiveChild>>>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NodePreloadRuntime>();
};

impl std::fmt::Debug for NodePreloadRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodePreloadRuntime")
            .field("config", &self.config)
            .field("is_alive", &self.is_alive())
            .finish_non_exhaustive()
    }
}

impl Drop for NodePreloadRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl NodePreloadRuntime {
    /// Creates an idle [`NodePreloadRuntime`] bound to `handler` with default timeouts.
    #[must_use]
    pub fn new(handler: Arc<dyn HostBridgeHandler>) -> Self {
        Self::with_config(NodeRuntimeConfig::default(), handler)
    }

    /// Creates an idle [`NodePreloadRuntime`] with custom [`NodeRuntimeConfig`] and `handler`.
    #[must_use]
    pub fn with_config(config: NodeRuntimeConfig, handler: Arc<dyn HostBridgeHandler>) -> Self {
        Self {
            config,
            handler,
            active: Mutex::new(None),
        }
    }

    /// Spawns a Node.js CommonJS sidecar and immediately loads `preload_path` inside `plugin_dir`.
    ///
    /// # Errors
    ///
    /// Returns an error if no compatible `node` binary is found, `preload_path` escapes `plugin_dir`,
    /// the child process fails startup, or `require(preload_path)` throws an exception.
    pub fn spawn_and_load(
        preload_path: impl AsRef<Path>,
        plugin_id: &str,
        plugin_dir: impl AsRef<Path>,
        handler: Arc<dyn HostBridgeHandler>,
    ) -> Result<Self> {
        let runtime = Self::new(handler);
        runtime.load(preload_path, plugin_id, plugin_dir)?;
        Ok(runtime)
    }

    /// Loads `preload_path` inside a freshly spawned CommonJS Node.js process with `cwd = plugin_dir`
    /// and `NODE_PATH` containing `<plugin_dir>/node_modules` (`SPEC-015-R2`, `SPEC-015-R3`).
    ///
    /// Guarantees zero writes or file mutations inside `plugin_dir` (`INV-ZTOOLS-IMMUTABLE`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::Io`] on path or process I/O failure, [`JsBridgeError::NodeRuntime`]
    /// if Node.js cannot be resolved or `require(preload_path)` fails, or [`JsBridgeError::Timeout`]
    /// if startup or loading exceeds configured timeouts.
    pub fn load(
        &self,
        preload_path: impl AsRef<Path>,
        plugin_id: &str,
        plugin_dir: impl AsRef<Path>,
    ) -> Result<PreloadLoadInfo> {
        if plugin_id.trim().is_empty() {
            return Err(JsBridgeError::NodeRuntime(
                "plugin_id must not be empty".into(),
            ));
        }

        let canonical_dir = std::fs::canonicalize(plugin_dir.as_ref())?;
        let preload_candidate = if preload_path.as_ref().is_relative() {
            canonical_dir.join(preload_path.as_ref())
        } else {
            preload_path.as_ref().to_path_buf()
        };
        let canonical_preload = std::fs::canonicalize(&preload_candidate)?;
        if !canonical_preload.starts_with(&canonical_dir) {
            return Err(JsBridgeError::NodeRuntime(format!(
                "preload path '{}' escapes plugin directory '{}'",
                canonical_preload.display(),
                canonical_dir.display()
            )));
        }

        let _ = self.shutdown();

        let _span = info_span!(
            "node_preload_load",
            plugin_id = %plugin_id,
            preload = %canonical_preload.display()
        )
        .entered();

        let node_bin = match self.config.node_binary {
            Some(ref explicit) => resolve_usable_executable(explicit).ok_or_else(|| {
                JsBridgeError::NodeRuntime(format!(
                    "configured node binary '{}' is missing or not executable",
                    explicit.display()
                ))
            })?,
            None => resolve_node_binary()?,
        };

        let mut runner_script = TempFileBuilder::new()
            .prefix("aitools-node-runner-")
            .suffix(".cjs")
            .tempfile()?;
        runner_script.write_all(NODE_PRELOAD_RUNNER_CJS.as_bytes())?;
        runner_script.flush()?;

        let mut command = Command::new(&node_bin);
        command
            .arg(runner_script.path())
            .current_dir(&canonical_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(joined) = build_node_path(&canonical_dir, &canonical_preload) {
            command.env("NODE_PATH", joined);
        }

        let mut child = command.spawn().map_err(|e| {
            JsBridgeError::NodeRuntime(format!(
                "failed to spawn node '{}': {e}",
                node_bin.display()
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            JsBridgeError::NodeRuntime("node child stdin pipe is unavailable".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            JsBridgeError::NodeRuntime("node child stdout pipe is unavailable".into())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            JsBridgeError::NodeRuntime("node child stderr pipe is unavailable".into())
        })?;

        let pending: PendingReplies = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let stdin_arc = Arc::new(Mutex::new(Some(stdin)));
        let (startup_tx, startup_rx) = mpsc::channel::<Result<()>>();

        let stderr_plugin = plugin_id.to_string();
        let stderr_thread = thread::Builder::new()
            .name(format!("aitools-node-stderr-{plugin_id}"))
            .spawn(move || drain_stderr(stderr, &stderr_plugin))?;

        let stdout_thread = {
            let plugin_owned = plugin_id.to_string();
            let handler = Arc::clone(&self.handler);
            let pending_ref = Arc::clone(&pending);
            let alive_ref = Arc::clone(&alive);
            let stdin_ref = Arc::clone(&stdin_arc);
            thread::Builder::new()
                .name(format!("aitools-node-stdout-{plugin_id}"))
                .spawn(move || {
                    read_stdout_loop(
                        stdout,
                        &plugin_owned,
                        handler,
                        pending_ref,
                        alive_ref,
                        stdin_ref,
                        startup_tx,
                    );
                })?
        };

        let active_child = Arc::new(ActiveChild {
            plugin_id: plugin_id.to_string(),
            stdin: stdin_arc,
            child: Mutex::new(child),
            pending,
            next_req_id: AtomicU64::new(1),
            call_lock: Mutex::new(()),
            alive,
            stdout_thread: Mutex::new(Some(stdout_thread)),
            stderr_thread: Mutex::new(Some(stderr_thread)),
            _runner_script: runner_script,
        });

        {
            let mut slot = self
                .active
                .lock()
                .map_err(|_| JsBridgeError::NodeRuntime("runtime active lock poisoned".into()))?;
            *slot = Some(Arc::clone(&active_child));
        }

        match startup_rx.recv_timeout(self.config.startup_timeout) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = self.shutdown();
                return Err(err);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = self.shutdown();
                return Err(JsBridgeError::Timeout(format!(
                    "node preload startup timed out after {:?}",
                    self.config.startup_timeout
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = self.shutdown();
                return Err(JsBridgeError::NodeRuntime(
                    "node preload exited during startup".into(),
                ));
            }
        }

        let load_res = self.call_method(
            "load",
            json!({
                "pluginId": plugin_id,
                "pluginDir": canonical_dir,
                "preloadPath": canonical_preload,
                "shimSource": TRIPLE_GLOBAL_SHIM_JS,
            }),
        );

        match load_res {
            Ok(val) => Ok(serde_json::from_value(val)?),
            Err(err) => {
                let _ = self.shutdown();
                Err(err)
            }
        }
    }

    /// Dispatches feature entry (`window.exports[feature_code].args.enter` or `onPluginEnter`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, the JS call throws, or the call times out.
    pub fn enter(&self, feature_code: &str, action: &Value) -> Result<Value> {
        self.call_method(
            "enter",
            json!({
                "featureCode": feature_code,
                "action": action,
            }),
        )
    }

    /// Dispatches list-mode search (`window.exports[feature_code].args.search` or `setSubInput`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, the JS call throws, or the call times out.
    pub fn search(&self, feature_code: &str, action: &Value, query: &str) -> Result<Value> {
        self.call_method(
            "search",
            json!({
                "featureCode": feature_code,
                "action": action,
                "query": query,
            }),
        )
    }

    /// Dispatches list-mode item selection (`window.exports[feature_code].args.select` or `onMainPush` select).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, the JS call throws, or the call times out.
    pub fn select(&self, feature_code: &str, action: &Value, item: &Value) -> Result<Value> {
        self.call_method(
            "select",
            json!({
                "featureCode": feature_code,
                "action": action,
                "item": item,
            }),
        )
    }

    /// Dispatches `onMainPush` callbacks for the loaded plugin.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, the JS callback throws, or the call times out.
    pub fn main_push(&self, feature_code: &str, action: &Value) -> Result<Value> {
        self.call_method(
            "main_push",
            json!({
                "featureCode": feature_code,
                "action": action,
            }),
        )
    }

    /// Invokes a function or reads a property exposed on `window.<prop_path>` by `preload.js` (`SPEC-015-R3`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, `prop_path` is invalid/missing,
    /// invocation throws, or the call times out.
    pub fn invoke_window_prop(&self, prop_path: &str, args: &[Value]) -> Result<Value> {
        self.call_method(
            "invoke_window_prop",
            json!({
                "propPath": prop_path,
                "args": args,
            }),
        )
    }

    /// Queries the current set of `window.*` properties exported by `preload.js`.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active or JSON decoding fails.
    pub fn get_exported_props(&self) -> Result<Vec<ExportedWindowProp>> {
        let val = self.call_method("get_exported_props", json!({}))?;
        Ok(serde_json::from_value(val)?)
    }

    /// Dispatches `onPluginOut(is_kill)` listeners registered by `preload.js`.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload child is active, the callback throws, or the call times out.
    pub fn plugin_out(&self, is_kill: bool) -> Result<Value> {
        self.call_method("plugin_out", json!({ "isKill": is_kill }))
    }

    /// Returns `true` if a Node.js child process is currently loaded and marked alive.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|c| c.alive.load(Ordering::Acquire)))
            .unwrap_or(false)
    }

    /// Returns the plugin ID currently loaded in this runtime, if any.
    #[must_use]
    pub fn plugin_id(&self) -> Option<String> {
        self.active
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|c| c.plugin_id.clone()))
    }

    /// Gracefully shuts down the active Node.js child process, falling back to `kill()` after a brief grace period.
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::NodeRuntime`] if internal state locks are poisoned.
    pub fn shutdown(&self) -> Result<()> {
        let taken = {
            let mut slot = self
                .active
                .lock()
                .map_err(|_| JsBridgeError::NodeRuntime("runtime active lock poisoned".into()))?;
            slot.take()
        };
        let Some(child_arc) = taken else {
            return Ok(());
        };
        terminate_active_child(&child_arc);
        Ok(())
    }

    fn current_child(&self) -> Result<Arc<ActiveChild>> {
        let slot = self
            .active
            .lock()
            .map_err(|_| JsBridgeError::NodeRuntime("runtime active lock poisoned".into()))?;
        slot.as_ref()
            .cloned()
            .ok_or_else(|| JsBridgeError::NodeRuntime("no active Node.js preload loaded".into()))
    }

    fn call_method(&self, method: &str, params: Value) -> Result<Value> {
        let child_arc = self.current_child()?;
        let res = call_on_child(&child_arc, method, params, self.config.call_timeout);
        if matches!(res, Err(JsBridgeError::Timeout(_))) {
            let _ = self.shutdown();
        }
        res
    }
}

fn call_on_child(
    child_arc: &ActiveChild,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let _call_guard = child_arc
        .call_lock
        .lock()
        .map_err(|_| JsBridgeError::NodeRuntime("node preload call lock poisoned".into()))?;

    if !child_arc.alive.load(Ordering::Acquire) {
        return Err(JsBridgeError::NodeRuntime(format!(
            "node preload for '{}' is no longer alive",
            child_arc.plugin_id
        )));
    }

    let id = child_arc.next_req_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    {
        let mut pending = child_arc
            .pending
            .lock()
            .map_err(|_| JsBridgeError::NodeRuntime("pending reply lock poisoned".into()))?;
        pending.insert(id, tx);
    }

    let frame = json!({
        "type": "request",
        "id": id,
        "method": method,
        "params": params,
    });
    if let Err(err) = write_stdin_frame(&child_arc.stdin, &frame) {
        if let Ok(mut pending) = child_arc.pending.lock() {
            pending.remove(&id);
        }
        return Err(err);
    }

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if let Ok(mut pending) = child_arc.pending.lock() {
                pending.remove(&id);
            }
            Err(JsBridgeError::Timeout(format!(
                "node preload '{method}' for '{}' timed out after {timeout:?}",
                child_arc.plugin_id
            )))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Ok(mut pending) = child_arc.pending.lock() {
                pending.remove(&id);
            }
            Err(JsBridgeError::NodeRuntime(format!(
                "node preload channel for '{}' disconnected during '{method}'",
                child_arc.plugin_id
            )))
        }
    }
}

fn write_stdin_frame(stdin: &Mutex<Option<ChildStdin>>, frame: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    let mut guard = stdin
        .lock()
        .map_err(|_| JsBridgeError::NodeRuntime("node stdin mutex poisoned".into()))?;
    let Some(ref mut pipe) = *guard else {
        return Err(JsBridgeError::NodeRuntime("node stdin is closed".into()));
    };
    pipe.write_all(&bytes)?;
    pipe.flush()?;
    Ok(())
}

fn terminate_active_child(child_arc: &ActiveChild) {
    if child_arc.alive.load(Ordering::Acquire) {
        let _ = call_on_child(child_arc, "shutdown", json!({}), SHUTDOWN_GRACE_TIMEOUT);
    }
    child_arc.alive.store(false, Ordering::Release);

    if let Ok(mut stdin_guard) = child_arc.stdin.lock() {
        drop(stdin_guard.take());
    }

    if let Ok(mut proc_guard) = child_arc.child.lock() {
        let deadline = Instant::now() + SHUTDOWN_GRACE_TIMEOUT;
        let mut exited = false;
        while Instant::now() < deadline {
            match proc_guard.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        if !exited {
            let _ = proc_guard.kill();
            let _ = proc_guard.wait();
        }
    }

    if let Ok(mut guard) = child_arc.stdout_thread.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
    }
    if let Ok(mut guard) = child_arc.stderr_thread.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
    }
}

fn drain_stderr(stderr: ChildStderr, plugin_id: &str) {
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        match line {
            Ok(msg) => {
                if !msg.trim().is_empty() {
                    debug!(plugin_id = %plugin_id, "[node-preload stderr] {msg}");
                }
            }
            Err(_) => break,
        }
    }
}

fn read_stdout_loop(
    stdout: ChildStdout,
    plugin_id: &str,
    handler: Arc<dyn HostBridgeHandler>,
    pending: PendingReplies,
    alive: Arc<AtomicBool>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    startup_tx: mpsc::Sender<Result<()>>,
) {
    let mut startup_opt = Some(startup_tx);
    let reader = BufReader::new(stdout);
    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(err) => {
                warn!(plugin_id = %plugin_id, "ignoring non-JSON line from node stdout: {err}");
                continue;
            }
        };

        let Some(msg_type) = msg.get("type").and_then(Value::as_str) else {
            continue;
        };

        match msg_type {
            "ready" => {
                if let Some(tx) = startup_opt.take() {
                    let _ = tx.send(Ok(()));
                }
            }
            "startup_error" => {
                let err_msg = msg
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown node startup error")
                    .to_string();
                if let Some(tx) = startup_opt.take() {
                    let _ = tx.send(Err(JsBridgeError::NodeRuntime(err_msg)));
                }
            }
            "response" => {
                let Some(id) = msg.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let ok = msg.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let reply = if ok {
                    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    let err_str = msg
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown node preload error")
                        .to_string();
                    Err(JsBridgeError::NodeRuntime(err_str))
                };
                if let Ok(mut map) = pending.lock() {
                    if let Some(tx) = map.remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
            }
            "rpc_request" => {
                let Some(rpc_id) = msg.get("rpc_id").and_then(Value::as_u64) else {
                    continue;
                };
                let op = msg.get("op").and_then(Value::as_str).unwrap_or("");
                let payload = msg.get("payload").cloned().unwrap_or(Value::Null);
                let reply_frame = match dispatch_host_rpc(handler.as_ref(), plugin_id, op, payload)
                {
                    Ok(data) => json!({
                        "type": "rpc_reply",
                        "rpc_reply_id": rpc_id,
                        "ok": true,
                        "data": data,
                    }),
                    Err(err) => json!({
                        "type": "rpc_reply",
                        "rpc_reply_id": rpc_id,
                        "ok": false,
                        "error": err.to_string(),
                    }),
                };
                if let Err(err) = write_stdin_frame(&stdin, &reply_frame) {
                    warn!(plugin_id = %plugin_id, op = %op, "failed sending rpc_reply: {err}");
                }
            }
            _ => {}
        }
    }

    alive.store(false, Ordering::Release);
    if let Some(tx) = startup_opt.take() {
        let _ = tx.send(Err(JsBridgeError::NodeRuntime(format!(
            "node preload for '{plugin_id}' exited before ready handshake"
        ))));
    }
    if let Ok(mut map) = pending.lock() {
        for (_, tx) in map.drain() {
            let _ = tx.send(Err(JsBridgeError::NodeRuntime(format!(
                "node preload for '{plugin_id}' exited unexpectedly"
            ))));
        }
    }
}

pub(crate) fn dispatch_host_rpc(
    handler: &dyn HostBridgeHandler,
    plugin_id: &str,
    op: &str,
    payload: Value,
) -> Result<Value> {
    if op == "callbackSetList" {
        let raw_items = payload
            .get("items")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let items: Vec<ListItem> = serde_json::from_value(raw_items)?;
        handler.callback_set_list(plugin_id, items)?;
        return Ok(Value::Bool(true));
    }
    if op.starts_with("notification/") {
        return handler.notification_op(plugin_id, op, payload);
    }
    if op.starts_with("db/") {
        return handler.db_op(plugin_id, op, payload);
    }
    if op.starts_with("clipboard/") {
        return handler.clipboard_op(plugin_id, op, payload);
    }
    if op.starts_with("window/") {
        return handler.window_op(plugin_id, op, payload);
    }
    if op.starts_with("shell/") {
        return handler.shell_op(plugin_id, op, payload);
    }
    if matches!(
        op,
        "system/simulateKeyboardTap" | "system/simulateMouseMove" | "system/simulateMouseClick"
    ) || op.starts_with("input/")
    {
        return handler.input_op(plugin_id, op, payload);
    }
    if op.starts_with("system/") || op.starts_with("display/") {
        return handler.system_op(plugin_id, op, payload);
    }
    if op.starts_with("features/") {
        return handler.features_op(plugin_id, op, payload);
    }
    if op.starts_with("dialog/") {
        return handler.dialog_op(plugin_id, op, payload);
    }
    if op.starts_with("ai/") {
        return handler.ai_op(plugin_id, op, payload);
    }
    if op.starts_with("http/") {
        return handler.http_op(plugin_id, op, payload);
    }
    Err(JsBridgeError::HostRpc(format!("unknown RPC op '{op}'")))
}

fn build_node_path(plugin_dir: &Path, preload_path: &Path) -> Option<OsString> {
    let mut entries: Vec<PathBuf> = Vec::new();
    entries.push(plugin_dir.join("node_modules"));
    if let Some(preload_dir) = preload_path.parent() {
        if preload_dir != plugin_dir {
            entries.push(preload_dir.join("node_modules"));
        }
    }
    entries.push(plugin_dir.to_path_buf());
    if let Some(existing) = std::env::var_os("NODE_PATH") {
        entries.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(entries).ok()
}

struct NodeSearchEnv {
    explicit: Option<OsString>,
    path: Option<OsString>,
    executable: Option<PathBuf>,
    home: Option<PathBuf>,
    system: Vec<PathBuf>,
}

/// Locates a usable host `node` executable without spawning a login shell (`SPEC-015-R2`).
///
/// Checks `ATOOLS_NODE_BINARY`, executable-adjacent packaged runtimes, absolute `PATH` entries,
/// `~/.nvm`, `~/.volta`, `~/.fnm`, `~/.local/bin`, and standard system installation directories.
///
/// # Errors
///
/// Returns [`JsBridgeError::NodeRuntime`] if `ATOOLS_NODE_BINARY` points to an invalid path
/// or no executable `node` binary is found on the machine.
pub fn resolve_node_binary() -> Result<PathBuf> {
    let mut system = Vec::new();
    #[cfg(target_os = "macos")]
    system.extend(["/opt/homebrew/bin/node", "/usr/local/bin/node"].map(PathBuf::from));
    #[cfg(all(unix, not(target_os = "macos")))]
    system.extend(["/usr/local/bin/node", "/usr/bin/node"].map(PathBuf::from));
    #[cfg(windows)]
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            system.push(PathBuf::from(root).join("nodejs/node.exe"));
        }
    }

    resolve_node_with(&NodeSearchEnv {
        explicit: std::env::var_os("ATOOLS_NODE_BINARY"),
        path: std::env::var_os("PATH"),
        executable: std::env::current_exe().ok(),
        home: dirs::home_dir(),
        system,
    })
}

fn resolve_node_with(search: &NodeSearchEnv) -> Result<PathBuf> {
    if let Some(ref value) = search.explicit {
        let requested = Path::new(value);
        let found = if requested.is_absolute() {
            resolve_usable_executable(requested)
        } else if requested.components().count() == 1 {
            from_absolute_path(search.path.as_deref(), value)
        } else {
            None
        };
        return found.ok_or_else(|| {
            JsBridgeError::NodeRuntime(
                "ATOOLS_NODE_BINARY must point to an existing executable node binary".into(),
            )
        });
    }

    let bin_name = node_executable_name();
    if let Some(parent) = search.executable.as_deref().and_then(Path::parent) {
        if let Some(found) = resolve_usable_executable(&parent.join(bin_name)) {
            return Ok(found);
        }
        for root in [
            parent.join("runtime"),
            parent.join("../Resources/runtime"),
            parent.join("../Resources/node/bin"),
        ] {
            for candidate in [root.join(bin_name), root.join("node/bin").join(bin_name)] {
                if let Some(found) = resolve_usable_executable(&candidate) {
                    return Ok(found);
                }
            }
        }
    }

    if let Some(found) = from_absolute_path(search.path.as_deref(), OsStr::new(bin_name)) {
        return Ok(found);
    }

    if let Some(ref home) = search.home {
        if let Some(found) = resolve_nvm_default(&home.join(".nvm"), bin_name) {
            return Ok(found);
        }
        for rel in [
            ".volta/bin",
            ".fnm/aliases/default/installation/bin",
            ".local/bin",
        ] {
            if let Some(found) = resolve_usable_executable(&home.join(rel).join(bin_name)) {
                return Ok(found);
            }
        }
    }

    for candidate in &search.system {
        if let Some(found) = resolve_usable_executable(candidate) {
            return Ok(found);
        }
    }

    Err(JsBridgeError::NodeRuntime(
        "Node.js runtime not found; install Node.js >= 18 or set ATOOLS_NODE_BINARY".into(),
    ))
}

const fn node_executable_name() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

fn from_absolute_path(path_env: Option<&OsStr>, bin_name: &OsStr) -> Option<PathBuf> {
    let path_env = path_env?;
    std::env::split_paths(path_env)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| resolve_usable_executable(&dir.join(bin_name)))
}

fn resolve_usable_executable(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(canonical)
}

fn resolve_nvm_default(nvm_root: &Path, bin_name: &str) -> Option<PathBuf> {
    let mut alias = "default".to_string();
    let mut prefix: Option<Vec<u64>> = None;

    for _ in 0..4 {
        if alias
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return None;
        }
        let value = read_small_text(&nvm_root.join("alias").join(&alias))?;
        let trimmed = value.trim();
        let numeric = trimmed.trim_start_matches('v');
        let parts: Vec<&str> = numeric.split('.').collect();
        if (1..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        {
            prefix = parts
                .iter()
                .map(|p| p.parse::<u64>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok();
            break;
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_/.*".contains(c))
        {
            return None;
        }
        alias = trimmed.to_string();
    }

    let prefix = prefix?;
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    let entries = std::fs::read_dir(nvm_root.join("versions/node")).ok()?;
    for entry in entries.take(128).flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Some(ver_str) = name_str.strip_prefix('v') else {
            continue;
        };
        let Ok(nums) = ver_str
            .split('.')
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>()
        else {
            continue;
        };
        if !nums.starts_with(&prefix) {
            continue;
        }
        let candidate = entry.path().join("bin").join(bin_name);
        if let Some(usable) = resolve_usable_executable(&candidate) {
            if best.as_ref().is_none_or(|(b, _)| nums > *b) {
                best = Some((nums, usable));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn read_small_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = String::new();
    file.take(512).read_to_string(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_node_ignores_relative_path_entries_and_finds_nvm_default() {
        let dir = TempDir::new().unwrap();
        let nvm_bin = dir.path().join(".nvm/versions/node/v24.15.0/bin");
        std::fs::create_dir_all(dir.path().join(".nvm/alias")).unwrap();
        std::fs::create_dir_all(&nvm_bin).unwrap();
        std::fs::write(dir.path().join(".nvm/alias/default"), "24\n").unwrap();

        let bin_path = nvm_bin.join(node_executable_name());
        std::fs::write(&bin_path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin_path, perms).unwrap();
        }

        let found = resolve_node_with(&NodeSearchEnv {
            explicit: None,
            path: Some(OsString::from("./relative-dir")),
            executable: None,
            home: Some(dir.path().to_path_buf()),
            system: Vec::new(),
        })
        .unwrap();
        assert_eq!(found, std::fs::canonicalize(&bin_path).unwrap());
    }
}
