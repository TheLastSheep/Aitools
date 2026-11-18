//! Tier 2 primary in-process Rust `rquickjs` Node/Electron compatibility runtime & unified
//! 3-tier [`PreloadRuntime`] orchestrator (`SPEC-015` v2.0).

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::UdpSocket;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64_STD;
use base64::Engine as _;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::Compression;
use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256, Sha384, Sha512};
use tracing::{debug, info_span, warn};
use url::Url;
use uuid::Uuid;

use crate::node_runtime::{
    dispatch_host_rpc, ExportedWindowProp, HostBridgeHandler, NodePreloadRuntime,
    NodeRuntimeConfig, PreloadLoadInfo,
};
use crate::shim::TRIPLE_GLOBAL_SHIM_JS;
use crate::{JsBridgeError, Result};

const RUST_NODE_COMPAT_BOOTSTRAP_JS: &str = include_str!("rust_node_compat_bootstrap.js");
const QUICKJS_HEAP_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RANDOM_BYTES: usize = 16 * 1024 * 1024;

/// Execution tier selected for a plugin preload script (`SPEC-015` v2.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreloadTier {
    /// Plugin has no `"preload"` entry and runs solely inside the WebView.
    WebViewOnly,
    /// Primary in-process pure-Rust `rquickjs` runtime with CommonJS & Node built-in compat.
    QuickJsNodeCompat,
    /// Tier 3 external `node` fallback used only on explicit [`JsBridgeError::NodeCompatGap`].
    SystemNodeFallback,
}

/// Policy controlling whether Tier 3 `SystemNodeFallback` is permitted (`SPEC-015-R2b`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreloadExecutionMode {
    /// Strictly forbids spawning external `node`; returns [`JsBridgeError::NodeCompatGap`] on gap.
    PureRustOnly,
    /// Always tries [`PreloadTier::QuickJsNodeCompat`] first and falls back to [`PreloadTier::SystemNodeFallback`]
    /// only on explicit [`JsBridgeError::NodeCompatGap`].
    #[default]
    RustFirstWithFallback,
}

/// Capability gates enforced by in-process `QuickJsNodeCompat` built-in modules (`SPEC-015-R2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PreloadCapabilities {
    /// Permits `fs`, `fs/promises`, and `original-fs` filesystem access.
    pub allow_fs: bool,
    /// Permits `http`, `https`, and global `fetch` routed through [`HostBridgeHandler::http_op`].
    pub allow_net: bool,
    /// Permits `child_process` command execution (`execSync`, `spawn`, `exec`, etc.).
    pub allow_child_process: bool,
}

impl PreloadCapabilities {
    /// Enables filesystem, network, and child-process capabilities for trusted legacy ZTools plugins.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            allow_fs: true,
            allow_net: true,
            allow_child_process: true,
        }
    }

    /// Disables filesystem, network, and child-process capabilities (least-privilege sandbox).
    #[must_use]
    pub const fn restricted() -> Self {
        Self {
            allow_fs: false,
            allow_net: false,
            allow_child_process: false,
        }
    }
}

#[derive(Debug, Clone)]
struct TimerEntry {
    id: u64,
    fire_at: Instant,
    delay: Duration,
    repeat: bool,
}

#[derive(Default)]
struct TimerState {
    next_id: u64,
    entries: Vec<TimerEntry>,
}

impl TimerState {
    fn schedule(&mut self, delay_ms: u64, repeat: bool) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let delay = Duration::from_millis(delay_ms);
        self.entries.push(TimerEntry {
            id,
            fire_at: Instant::now() + delay,
            delay,
            repeat,
        });
        id
    }

    fn clear(&mut self, id: u64) {
        self.entries.retain(|t| t.id != id);
    }

    fn pop_due(&mut self, now: Instant) -> Vec<u64> {
        let mut due = Vec::new();
        let mut idx = 0;
        while idx < self.entries.len() {
            if self.entries[idx].fire_at <= now {
                let id = self.entries[idx].id;
                due.push(id);
                if self.entries[idx].repeat {
                    let step = self.entries[idx].delay.max(Duration::from_millis(1));
                    self.entries[idx].fire_at = now + step;
                    idx += 1;
                } else {
                    self.entries.swap_remove(idx);
                }
            } else {
                idx += 1;
            }
        }
        due
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().map(|t| t.fire_at).min()
    }
}

enum WorkerCmd {
    Call {
        method: String,
        params: Value,
        timeout: Duration,
        reply: mpsc::Sender<Result<Value>>,
    },
    Shutdown,
}

/// Primary in-process pure-Rust `rquickjs` preload worker with CommonJS & Node built-in compat (`SPEC-015-R2`).
struct QuickJsNodeCompat {
    plugin_id: String,
    tx: Mutex<mpsc::Sender<WorkerCmd>>,
    alive: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for QuickJsNodeCompat {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl QuickJsNodeCompat {
    fn spawn_and_load(
        preload_path: &Path,
        plugin_id: &str,
        plugin_dir: &Path,
        capabilities: PreloadCapabilities,
        handler: Arc<dyn HostBridgeHandler>,
        timeout: Duration,
    ) -> Result<(Self, PreloadLoadInfo)> {
        let _span = info_span!("quickjs_cold_start", plugin_id = %plugin_id).entered();
        let canonical_dir = std::fs::canonicalize(plugin_dir)?;
        let canonical_preload = std::fs::canonicalize(preload_path)?;
        if !canonical_preload.starts_with(&canonical_dir) {
            return Err(JsBridgeError::NodeRuntime(format!(
                "preload path '{}' escapes plugin directory '{}'",
                canonical_preload.display(),
                canonical_dir.display()
            )));
        }

        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        let (init_tx, init_rx) = mpsc::channel::<Result<()>>();
        let alive = Arc::new(AtomicBool::new(true));

        let pid_owned = plugin_id.to_string();
        let dir_owned = canonical_dir.clone();
        let alive_ref = Arc::clone(&alive);

        let handle = thread::Builder::new()
            .name(format!("aitools-qjs-compat-{plugin_id}"))
            .spawn(move || {
                run_quickjs_worker(
                    rx,
                    init_tx,
                    &pid_owned,
                    &dir_owned,
                    capabilities,
                    handler,
                    alive_ref,
                );
            })?;

        match init_rx.recv_timeout(timeout) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = handle.join();
                return Err(err);
            }
            Err(_) => {
                alive.store(false, Ordering::Release);
                return Err(JsBridgeError::Timeout(format!(
                    "QuickJsNodeCompat initialization timed out for '{plugin_id}'"
                )));
            }
        }

        let worker = Self {
            plugin_id: plugin_id.to_string(),
            tx: Mutex::new(tx),
            alive,
            handle: Mutex::new(Some(handle)),
        };

        let load_val = worker.call(
            "load",
            json!({
                "pluginId": plugin_id,
                "pluginDir": canonical_dir,
                "preloadPath": canonical_preload,
            }),
            timeout,
        )?;
        let load_info: PreloadLoadInfo = serde_json::from_value(load_val)?;
        Ok((worker, load_info))
    }

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(JsBridgeError::QuickJs(format!(
                "QuickJsNodeCompat worker for '{}' is stopped",
                self.plugin_id
            )));
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        {
            let guard = self
                .tx
                .lock()
                .map_err(|_| JsBridgeError::QuickJs("worker tx mutex poisoned".into()))?;
            guard
                .send(WorkerCmd::Call {
                    method: method.to_string(),
                    params,
                    timeout,
                    reply: reply_tx,
                })
                .map_err(|_| JsBridgeError::QuickJs("QuickJsNodeCompat worker exited".into()))?;
        }
        match reply_rx.recv_timeout(timeout + Duration::from_millis(50)) {
            Ok(res) => res,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(JsBridgeError::Timeout(format!(
                "QuickJsNodeCompat '{method}' for '{}' timed out",
                self.plugin_id
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(JsBridgeError::QuickJs(format!(
                "QuickJsNodeCompat channel for '{}' disconnected",
                self.plugin_id
            ))),
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        if let Ok(guard) = self.tx.lock() {
            let _ = guard.send(WorkerCmd::Shutdown);
        }
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

fn run_quickjs_worker(
    rx: mpsc::Receiver<WorkerCmd>,
    init_tx: mpsc::Sender<Result<()>>,
    plugin_id: &str,
    plugin_dir: &Path,
    capabilities: PreloadCapabilities,
    handler: Arc<dyn HostBridgeHandler>,
    alive: Arc<AtomicBool>,
) {
    let timers = Arc::new(Mutex::new(TimerState::default()));
    let init = (|| -> Result<(Runtime, Context)> {
        let runtime = Runtime::new().map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
        runtime.set_memory_limit(QUICKJS_HEAP_BYTES);
        let context = Context::full(&runtime).map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

        let plugin_dir_owned = plugin_dir.to_path_buf();
        let pid_native = plugin_id.to_string();
        let timers_ref = Arc::clone(&timers);
        let pid_bridge = plugin_id.to_string();
        let handler_ref = Arc::clone(&handler);

        context.with(|ctx| -> Result<()> {
            // Non-reentrant native primitive boundary: returns plain JSON string, never invokes ctx.eval.
            let native_fn = Function::new(ctx.clone(), move |op: String, args_json: String| {
                handle_native_node_op(
                    &op,
                    &args_json,
                    &pid_native,
                    &plugin_dir_owned,
                    capabilities,
                    &timers_ref,
                )
            })
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

            let bridge_fn = Function::new(ctx.clone(), move |op: String, payload_json: String| {
                handle_host_bridge_op(&op, &payload_json, &pid_bridge, capabilities, &handler_ref)
            })
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

            ctx.globals()
                .set("__aitools_native_node_op", native_fn)
                .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
            ctx.globals()
                .set("__aitools_host_bridge_op", bridge_fn)
                .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

            ctx.eval::<(), _>(RUST_NODE_COMPAT_BOOTSTRAP_JS)
                .map_err(|e| JsBridgeError::QuickJs(format!("bootstrap eval error: {e}")))?;

            let pid_lit = serde_json::to_string(plugin_id)?;
            ctx.eval::<(), _>(format!("globalThis.__AITOOLS_PLUGIN_ID__ = {pid_lit};"))
                .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
            ctx.eval::<(), _>(TRIPLE_GLOBAL_SHIM_JS)
                .map_err(|e| JsBridgeError::QuickJs(format!("shim eval error: {e}")))?;
            Ok(())
        })?;

        Ok((runtime, context))
    })();

    let (runtime, context) = match init {
        Ok(pair) => {
            let _ = init_tx.send(Ok(()));
            pair
        }
        Err(err) => {
            alive.store(false, Ordering::Release);
            let _ = init_tx.send(Err(err));
            return;
        }
    };

    loop {
        let wait_duration = timers
            .lock()
            .ok()
            .and_then(|t| t.next_deadline())
            .map(|dl| dl.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| Duration::from_millis(250));

        match rx.recv_timeout(wait_duration.min(Duration::from_millis(250))) {
            Ok(WorkerCmd::Call {
                method,
                params,
                timeout,
                reply,
            }) => {
                let res = execute_quickjs_call(
                    &runtime, &context, &timers, plugin_id, &method, &params, timeout,
                );
                let _ = reply.send(res);
            }
            Ok(WorkerCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = pump_jobs_and_timers(&runtime, &context, &timers);
            }
        }
    }

    alive.store(false, Ordering::Release);
}

fn pump_jobs_and_timers(
    runtime: &Runtime,
    context: &Context,
    timers: &Arc<Mutex<TimerState>>,
) -> Result<()> {
    for _ in 0..256 {
        let mut progressed = false;
        while runtime.is_job_pending() {
            progressed = true;
            if let Err(err) = runtime.execute_pending_job() {
                debug!("pending QuickJS job threw: {err:?}");
                break;
            }
        }
        let due_ids = timers
            .lock()
            .map(|mut t| t.pop_due(Instant::now()))
            .unwrap_or_default();
        if !due_ids.is_empty() {
            progressed = true;
            context.with(|ctx| {
                for id in due_ids {
                    let _ = ctx.eval::<(), _>(format!("globalThis.__aitools_fire_timer__({id});"));
                }
            });
            while runtime.is_job_pending() {
                if let Err(err) = runtime.execute_pending_job() {
                    debug!("pending QuickJS job after timer threw: {err:?}");
                    break;
                }
            }
        }
        if !progressed {
            break;
        }
    }
    Ok(())
}

fn execute_quickjs_call(
    runtime: &Runtime,
    context: &Context,
    timers: &Arc<Mutex<TimerState>>,
    plugin_id: &str,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let method_lit = serde_json::to_string(method)?;
    let params_json = serde_json::to_string(params)?;
    let params_lit = serde_json::to_string(&params_json)?;

    let slot_id: u64 = context.with(|ctx| {
        ctx.eval::<u64, _>(format!(
            "globalThis.__aitools_runtime_api__.start({method_lit}, {params_lit})"
        ))
        .map_err(|e| JsBridgeError::QuickJs(e.to_string()))
    })?;

    loop {
        pump_jobs_and_timers(runtime, context, timers)?;
        let polled: String = context.with(|ctx| {
            ctx.eval::<String, _>(format!(
                "globalThis.__aitools_runtime_api__.poll({slot_id})"
            ))
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))
        })?;

        if !polled.is_empty() {
            let envelope: Value = serde_json::from_str(&polled)?;
            if envelope.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(envelope.get("data").cloned().unwrap_or(Value::Null));
            }
            if let Some(gap) = envelope.get("compatGap").and_then(Value::as_str) {
                return Err(JsBridgeError::NodeCompatGap {
                    plugin_id: plugin_id.to_string(),
                    capability: gap.to_string(),
                });
            }
            let err_msg = envelope
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("QuickJsNodeCompat error")
                .to_string();
            if let Some(idx) = err_msg.find("__AITOOLS_NODE_COMPAT_GAP__:") {
                let capability = err_msg[idx + "__AITOOLS_NODE_COMPAT_GAP__:".len()..]
                    .lines()
                    .next()
                    .unwrap_or("unknown")
                    .trim()
                    .to_string();
                return Err(JsBridgeError::NodeCompatGap {
                    plugin_id: plugin_id.to_string(),
                    capability,
                });
            }
            return Err(JsBridgeError::QuickJs(err_msg));
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(JsBridgeError::Timeout(format!(
                "QuickJsNodeCompat '{method}' timed out after {timeout:?}"
            )));
        }
        let sleep_step = timers
            .lock()
            .ok()
            .and_then(|t| t.next_deadline())
            .map(|dl| dl.saturating_duration_since(now))
            .unwrap_or_else(|| Duration::from_millis(5))
            .min(Duration::from_millis(5))
            .min(deadline.saturating_duration_since(now));
        if !sleep_step.is_zero() {
            thread::sleep(sleep_step);
        }
    }
}

fn handle_host_bridge_op(
    op: &str,
    payload_json: &str,
    plugin_id: &str,
    capabilities: PreloadCapabilities,
    handler: &Arc<dyn HostBridgeHandler>,
) -> String {
    if op.starts_with("http/") && !capabilities.allow_net {
        return json_err_with_code("EACCES: network capability disabled", "EACCES");
    }
    let payload: Value = match serde_json::from_str(payload_json) {
        Ok(v) => v,
        Err(err) => return json_err(&format!("invalid host bridge JSON: {err}")),
    };
    match dispatch_host_rpc(handler.as_ref(), plugin_id, op, payload) {
        Ok(data) => json_ok(data),
        Err(err) => json_err(&err.to_string()),
    }
}

fn handle_native_node_op(
    op: &str,
    args_json: &str,
    plugin_id: &str,
    plugin_dir: &Path,
    capabilities: PreloadCapabilities,
    timers: &Arc<Mutex<TimerState>>,
) -> String {
    let args: Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(err) => return json_err(&format!("invalid native node op JSON: {err}")),
    };

    match op {
        "resolve_module" => native_resolve_module(&args, plugin_dir),
        "read_module_source" => native_read_module_source(&args, plugin_dir),
        "fs_op" => native_fs_op(&args, capabilities),
        "os_info" => native_os_info(plugin_dir),
        "os_network_interfaces" => json_ok(probe_network_interfaces()),
        "crypto_hash" => native_crypto_hash(&args),
        "crypto_hmac" => native_crypto_hmac(&args),
        "crypto_random_bytes" => native_crypto_random_bytes(&args),
        "crypto_random_uuid" => json_ok(Value::String(Uuid::new_v4().to_string())),
        "url_op" => native_url_op(&args),
        "zlib_op" => native_zlib_op(&args),
        "child_process_op" => native_child_process_op(&args, capabilities),
        "timer_schedule" => {
            let delay_ms = args.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
            let repeat = args.get("repeat").and_then(Value::as_bool).unwrap_or(false);
            match timers.lock() {
                Ok(mut guard) => json_ok(json!(guard.schedule(delay_ms, repeat))),
                Err(_) => json_err("timer queue lock poisoned"),
            }
        }
        "timer_clear" => {
            let id = args.get("id").and_then(Value::as_u64).unwrap_or(0);
            if let Ok(mut guard) = timers.lock() {
                guard.clear(id);
            }
            json_ok(Value::Bool(true))
        }
        "console_log" => {
            let level = args.get("level").and_then(Value::as_str).unwrap_or("info");
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            match level {
                "warn" | "error" => {
                    warn!(plugin_id = %plugin_id, "[quickjs console.{level}] {text}")
                }
                _ => debug!(plugin_id = %plugin_id, "[quickjs console.{level}] {text}"),
            }
            json_ok(Value::Bool(true))
        }
        _ => json_err(&format!("unknown native node op: {op}")),
    }
}

fn json_ok(data: Value) -> String {
    serde_json::to_string(&json!({ "ok": true, "data": data }))
        .unwrap_or_else(|_| r#"{"ok":true,"data":null}"#.to_string())
}

fn json_err(message: &str) -> String {
    serde_json::to_string(&json!({ "ok": false, "error": message }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization error"}"#.to_string())
}

fn json_err_with_code(message: &str, code: &str) -> String {
    serde_json::to_string(&json!({ "ok": false, "error": message, "code": code }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"serialization error"}"#.to_string())
}

fn json_compat_gap(specifier: &str) -> String {
    serde_json::to_string(&json!({
        "ok": false,
        "compatGap": specifier,
        "error": format!("__AITOOLS_NODE_COMPAT_GAP__:{specifier}"),
    }))
    .unwrap_or_else(|_| r#"{"ok":false,"compatGap":"unknown"}"#.to_string())
}

fn is_supported_builtin(name: &str) -> bool {
    matches!(
        name,
        "electron"
            | "fs"
            | "fs/promises"
            | "original-fs"
            | "path"
            | "path/posix"
            | "path/win32"
            | "os"
            | "crypto"
            | "buffer"
            | "url"
            | "querystring"
            | "util"
            | "events"
            | "stream"
            | "timers"
            | "timers/promises"
            | "readline"
            | "zlib"
            | "assert"
            | "assert/strict"
            | "string_decoder"
            | "process"
            | "console"
            | "module"
            | "child_process"
            | "http"
            | "https"
    )
}

fn is_gap_builtin(name: &str) -> bool {
    matches!(
        name,
        "worker_threads"
            | "vm"
            | "dgram"
            | "tls"
            | "net"
            | "http2"
            | "inspector"
            | "inspector/promises"
            | "v8"
            | "perf_hooks"
            | "async_hooks"
            | "cluster"
            | "dns"
            | "repl"
            | "tty"
            | "diagnostics_channel"
            | "trace_events"
            | "wasi"
            | "sqlite"
            | "@electron/remote"
    )
}

fn native_resolve_module(args: &Value, plugin_dir: &Path) -> String {
    let request = args.get("request").and_then(Value::as_str).unwrap_or("");
    let parent_filename = args
        .get("parentFilename")
        .and_then(Value::as_str)
        .unwrap_or("");
    if request.is_empty() {
        return json_err_with_code("Cannot find empty module", "MODULE_NOT_FOUND");
    }

    let bare = request.strip_prefix("node:").unwrap_or(request);
    if is_supported_builtin(bare) {
        return json_ok(json!({ "kind": "builtin", "name": bare }));
    }
    if is_gap_builtin(bare) || request.starts_with("node:") {
        return json_compat_gap(request);
    }
    if request.ends_with(".node") {
        return json_compat_gap(request);
    }

    let parent_dir = Path::new(parent_filename)
        .parent()
        .filter(|p| p.starts_with(plugin_dir))
        .unwrap_or(plugin_dir);

    let is_path_req = request.starts_with("./")
        || request.starts_with("../")
        || request == "."
        || request == ".."
        || request.starts_with('/')
        || request.starts_with('\\');

    if is_path_req {
        let candidate = if Path::new(request).is_absolute() {
            PathBuf::from(request)
        } else {
            parent_dir.join(request)
        };
        if has_parent_escape(plugin_dir, &candidate) {
            return json_err_with_code(
                &format!("Cannot require module outside plugin root: {request}"),
                "MODULE_NOT_FOUND",
            );
        }
        return resolve_file_or_dir_candidate(request, &candidate, plugin_dir);
    }

    // Walk node_modules up to canonical plugin_dir (never escaping plugin_dir).
    let mut cursor = Some(parent_dir);
    while let Some(dir) = cursor {
        if !dir.starts_with(plugin_dir) {
            break;
        }
        let nm_candidate = dir.join("node_modules").join(request);
        if let Some(resolved) = try_resolve_file_or_dir(request, &nm_candidate, plugin_dir) {
            return resolved;
        }
        if dir == plugin_dir {
            break;
        }
        cursor = dir.parent();
    }

    json_err_with_code(
        &format!("Cannot find module '{request}'"),
        "MODULE_NOT_FOUND",
    )
}

fn has_parent_escape(plugin_dir: &Path, candidate: &Path) -> bool {
    let mut normalized = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                if !normalized.pop() {
                    return true;
                }
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    !normalized.starts_with(plugin_dir)
}

fn resolve_file_or_dir_candidate(request: &str, candidate: &Path, plugin_dir: &Path) -> String {
    try_resolve_file_or_dir(request, candidate, plugin_dir).unwrap_or_else(|| {
        json_err_with_code(
            &format!("Cannot find module '{request}'"),
            "MODULE_NOT_FOUND",
        )
    })
}

fn try_resolve_file_or_dir(request: &str, base: &Path, plugin_dir: &Path) -> Option<String> {
    // 1. Exact file or common JS/JSON/.node extensions
    let mut probes = vec![base.to_path_buf()];
    if let Some(s) = base.to_str() {
        probes.push(PathBuf::from(format!("{s}.js")));
        probes.push(PathBuf::from(format!("{s}.cjs")));
        probes.push(PathBuf::from(format!("{s}.json")));
        probes.push(PathBuf::from(format!("{s}.node")));
        probes.push(PathBuf::from(format!("{s}.mjs")));
    }
    for probe in probes {
        if let Ok(canonical) = std::fs::canonicalize(&probe) {
            if canonical.is_file() && canonical.starts_with(plugin_dir) {
                return Some(classify_resolved_file(request, &canonical));
            }
        }
    }

    // 2. Directory with package.json or index.{js,cjs,json}
    if let Ok(canon_dir) = std::fs::canonicalize(base) {
        if canon_dir.is_dir() && canon_dir.starts_with(plugin_dir) {
            let pkg_path = canon_dir.join("package.json");
            if pkg_path.is_file() {
                if let Ok(raw) = std::fs::read_to_string(&pkg_path) {
                    if let Ok(pkg_json) = serde_json::from_str::<Value>(&raw) {
                        if let Some(entry) = extract_package_entry(&pkg_json) {
                            let target = canon_dir.join(entry);
                            if let Some(found) =
                                try_resolve_file_or_dir(request, &target, plugin_dir)
                            {
                                return Some(found);
                            }
                        }
                    }
                }
            }
            for idx_name in ["index.js", "index.cjs", "index.json", "index.node"] {
                let idx = canon_dir.join(idx_name);
                if let Ok(canonical) = std::fs::canonicalize(&idx) {
                    if canonical.is_file() && canonical.starts_with(plugin_dir) {
                        return Some(classify_resolved_file(request, &canonical));
                    }
                }
            }
        }
    }
    None
}

fn extract_package_entry(pkg: &Value) -> Option<String> {
    if let Some(exports) = pkg.get("exports") {
        if let Some(s) = exports.as_str() {
            return Some(s.to_string());
        }
        if let Some(root) = exports.get(".") {
            if let Some(s) = root.as_str() {
                return Some(s.to_string());
            }
            for cond in ["require", "node", "default"] {
                if let Some(s) = root.get(cond).and_then(Value::as_str) {
                    return Some(s.to_string());
                }
            }
        }
        for cond in ["require", "node", "default"] {
            if let Some(s) = exports.get(cond).and_then(Value::as_str) {
                return Some(s.to_string());
            }
        }
    }
    pkg.get("main").and_then(Value::as_str).map(str::to_string)
}

fn classify_resolved_file(request: &str, canonical: &Path) -> String {
    match canonical.extension().and_then(|e| e.to_str()) {
        Some("node") => json_compat_gap(request),
        Some("mjs") => json_compat_gap(&format!("esm:{request}")),
        Some("json") => json_ok(json!({ "kind": "json", "filename": canonical })),
        _ => json_ok(json!({ "kind": "cjs", "filename": canonical })),
    }
}

fn native_read_module_source(args: &Value, plugin_dir: &Path) -> String {
    let filename = args.get("filename").and_then(Value::as_str).unwrap_or("");
    let Ok(canonical) = std::fs::canonicalize(filename) else {
        return json_err_with_code(&format!("ENOENT: no such file '{filename}'"), "ENOENT");
    };
    if !canonical.starts_with(plugin_dir) {
        return json_err_with_code("EACCES: module path outside plugin directory", "EACCES");
    }
    match std::fs::read_to_string(&canonical) {
        Ok(src) => json_ok(Value::String(src)),
        Err(err) => json_err(&err.to_string()),
    }
}

fn native_fs_op(args: &Value, capabilities: PreloadCapabilities) -> String {
    if !capabilities.allow_fs {
        return json_err_with_code("EACCES: fs capability disabled", "EACCES");
    }
    let sub_op = args.get("subOp").and_then(Value::as_str).unwrap_or("");
    let path_str = args.get("path").and_then(Value::as_str).unwrap_or("");
    let path = Path::new(path_str);

    match sub_op {
        "existsSync" => json_ok(Value::Bool(path.exists())),
        "readFileSync" => match std::fs::read(path) {
            Ok(bytes) => json_ok(Value::String(BASE64_STD.encode(bytes))),
            Err(e) => map_io_err(e),
        },
        "writeFileSync" => {
            let b64 = args.get("dataBase64").and_then(Value::as_str).unwrap_or("");
            let Ok(bytes) = BASE64_STD.decode(b64) else {
                return json_err("invalid base64 payload for writeFileSync");
            };
            match std::fs::write(path, bytes) {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "appendFileSync" => {
            let b64 = args.get("dataBase64").and_then(Value::as_str).unwrap_or("");
            let Ok(bytes) = BASE64_STD.decode(b64) else {
                return json_err("invalid base64 payload for appendFileSync");
            };
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| f.write_all(&bytes))
            {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "mkdirSync" => {
            let recursive = args
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let res = if recursive {
                std::fs::create_dir_all(path)
            } else {
                std::fs::create_dir(path)
            };
            match res {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "readdirSync" => {
            let with_types = args
                .get("withFileTypes")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let entries = match std::fs::read_dir(path) {
                Ok(rd) => rd,
                Err(e) => return map_io_err(e),
            };
            let mut out = Vec::new();
            for item in entries.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                if with_types {
                    let ft = item.file_type().ok();
                    out.push(json!({
                        "name": name,
                        "isFile": ft.is_some_and(|t| t.is_file()),
                        "isDirectory": ft.is_some_and(|t| t.is_dir()),
                        "isSymbolicLink": ft.is_some_and(|t| t.is_symlink()),
                    }));
                } else {
                    out.push(Value::String(name));
                }
            }
            json_ok(Value::Array(out))
        }
        "statSync" | "lstatSync" => {
            let meta = if sub_op == "lstatSync" {
                std::fs::symlink_metadata(path)
            } else {
                std::fs::metadata(path)
            };
            match meta {
                Ok(m) => {
                    let mtime_ms = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0_u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    let atime_ms = m
                        .accessed()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0_u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    let ctime_ms = m
                        .created()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0_u64, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode()
                    };
                    #[cfg(not(unix))]
                    let mode = 0o644_u32;
                    json_ok(json!({
                        "size": m.len(),
                        "mode": mode,
                        "mtimeMs": mtime_ms,
                        "atimeMs": atime_ms,
                        "ctimeMs": ctime_ms,
                        "birthtimeMs": ctime_ms,
                        "isFile": m.is_file(),
                        "isDirectory": m.is_dir(),
                        "isSymbolicLink": m.file_type().is_symlink(),
                    }))
                }
                Err(e) => map_io_err(e),
            }
        }
        "unlinkSync" => match std::fs::remove_file(path) {
            Ok(()) => json_ok(Value::Null),
            Err(e) => map_io_err(e),
        },
        "rmSync" => {
            let recursive = args
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let res = if path.is_dir() && recursive {
                std::fs::remove_dir_all(path)
            } else if path.exists() {
                if path.is_dir() {
                    std::fs::remove_dir(path)
                } else {
                    std::fs::remove_file(path)
                }
            } else if force {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such file or directory",
                ))
            };
            match res {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "rmdirSync" => {
            let recursive = args
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let res = if recursive {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_dir(path)
            };
            match res {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "renameSync" => {
            let src = args.get("src").and_then(Value::as_str).unwrap_or("");
            let dst = args.get("dst").and_then(Value::as_str).unwrap_or("");
            match std::fs::rename(src, dst) {
                Ok(()) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "copyFileSync" => {
            let src = args.get("src").and_then(Value::as_str).unwrap_or("");
            let dst = args.get("dst").and_then(Value::as_str).unwrap_or("");
            match std::fs::copy(src, dst) {
                Ok(_) => json_ok(Value::Null),
                Err(e) => map_io_err(e),
            }
        }
        "realpathSync" => match std::fs::canonicalize(path) {
            Ok(canon) => json_ok(Value::String(canon.to_string_lossy().into_owned())),
            Err(e) => map_io_err(e),
        },
        "accessSync" => match std::fs::metadata(path) {
            Ok(_) => json_ok(Value::Null),
            Err(e) => map_io_err(e),
        },
        "chmodSync" => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = args.get("mode").and_then(Value::as_u64).unwrap_or(0o644);
                let mut perms = match std::fs::metadata(path) {
                    Ok(m) => m.permissions(),
                    Err(e) => return map_io_err(e),
                };
                perms.set_mode(u32::try_from(mode).unwrap_or(0o644));
                match std::fs::set_permissions(path, perms) {
                    Ok(()) => json_ok(Value::Null),
                    Err(e) => map_io_err(e),
                }
            }
            #[cfg(not(unix))]
            json_ok(Value::Null)
        }
        _ => json_err(&format!("unsupported fs_op: {sub_op}")),
    }
}

fn map_io_err(err: std::io::Error) -> String {
    let code = match err.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        _ => "EIO",
    };
    json_err_with_code(&format!("{code}: {err}"), code)
}

fn native_os_info(plugin_dir: &Path) -> String {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let os_type = match std::env::consts::OS {
        "macos" => "Darwin",
        "windows" => "Windows_NT",
        "linux" => "Linux",
        other => other,
    };
    let homedir = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let tmpdir = std::env::temp_dir();
    let eol = if cfg!(windows) { "\r\n" } else { "\n" };
    let env_map: HashMap<String, String> = std::env::vars().collect();
    let cpu_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let cpus: Vec<Value> = (0..cpu_count)
        .map(|_| json!({ "model": "Host CPU", "speed": 2400 }))
        .collect();

    json_ok(json!({
        "platform": platform,
        "arch": arch,
        "osType": os_type,
        "release": "1.0.0",
        "hostname": env_map.get("HOSTNAME").cloned().unwrap_or_else(|| "localhost".to_string()),
        "homedir": homedir.to_string_lossy(),
        "tmpdir": tmpdir.to_string_lossy(),
        "cwd": plugin_dir.to_string_lossy(),
        "eol": eol,
        "endianness": if cfg!(target_endian = "little") { "LE" } else { "BE" },
        "pid": std::process::id(),
        "ppid": 0,
        "env": env_map,
        "cpus": cpus,
        "totalmem": 8_u64 * 1024 * 1024 * 1024,
        "freemem": 4_u64 * 1024 * 1024 * 1024,
    }))
}

fn probe_network_interfaces() -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "lo0".to_string(),
        json!([{
            "address": "127.0.0.1",
            "netmask": "255.0.0.0",
            "family": "IPv4",
            "mac": "00:00:00:00:00:00",
            "internal": true,
            "cidr": "127.0.0.1/8"
        }]),
    );

    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.0.2.1:80").is_ok() {
            if let Ok(local) = sock.local_addr() {
                let ip = local.ip();
                if ip.is_ipv4() && !ip.is_loopback() {
                    let ip_str = ip.to_string();
                    map.insert(
                        "en0".to_string(),
                        json!([{
                            "address": ip_str,
                            "netmask": "255.255.255.0",
                            "family": "IPv4",
                            "mac": "00:00:00:00:00:00",
                            "internal": false,
                            "cidr": format!("{ip}/24")
                        }]),
                    );
                }
            }
        }
    }
    Value::Object(map)
}

fn native_crypto_hash(args: &Value) -> String {
    let algo = args
        .get("algorithm")
        .and_then(Value::as_str)
        .unwrap_or("sha256");
    let data_b64 = args.get("dataBase64").and_then(Value::as_str).unwrap_or("");
    let Ok(bytes) = BASE64_STD.decode(data_b64) else {
        return json_err("invalid base64 data for crypto_hash");
    };
    match compute_hash_bytes(algo, &bytes) {
        Ok(digest) => json_ok(Value::String(BASE64_STD.encode(digest))),
        Err(msg) => json_err(&msg),
    }
}

fn native_crypto_hmac(args: &Value) -> String {
    let algo = args
        .get("algorithm")
        .and_then(Value::as_str)
        .unwrap_or("sha256");
    let key_b64 = args.get("keyBase64").and_then(Value::as_str).unwrap_or("");
    let data_b64 = args.get("dataBase64").and_then(Value::as_str).unwrap_or("");
    let Ok(key) = BASE64_STD.decode(key_b64) else {
        return json_err("invalid base64 key for crypto_hmac");
    };
    let Ok(data) = BASE64_STD.decode(data_b64) else {
        return json_err("invalid base64 data for crypto_hmac");
    };

    let block_size = match algo.to_ascii_lowercase().as_str() {
        "sha384" | "sha512" | "sha-384" | "sha-512" => 128,
        _ => 64,
    };

    let key_block = if key.len() > block_size {
        match compute_hash_bytes(algo, &key) {
            Ok(h) => h,
            Err(msg) => return json_err(&msg),
        }
    } else {
        key
    };

    let mut ipad = vec![0x36_u8; block_size];
    let mut opad = vec![0x5c_u8; block_size];
    for (idx, b) in key_block.iter().enumerate() {
        ipad[idx] ^= *b;
        opad[idx] ^= *b;
    }

    ipad.extend_from_slice(&data);
    let inner = match compute_hash_bytes(algo, &ipad) {
        Ok(h) => h,
        Err(msg) => return json_err(&msg),
    };
    opad.extend_from_slice(&inner);
    match compute_hash_bytes(algo, &opad) {
        Ok(mac) => json_ok(Value::String(BASE64_STD.encode(mac))),
        Err(msg) => json_err(&msg),
    }
}

fn compute_hash_bytes(algo: &str, data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    match algo.to_ascii_lowercase().as_str() {
        "md5" => Ok(md5_digest(data).to_vec()),
        "sha1" | "sha-1" => Ok(sha1_digest(data).to_vec()),
        "sha256" | "sha-256" => Ok(Sha256::digest(data).to_vec()),
        "sha384" | "sha-384" => Ok(Sha384::digest(data).to_vec()),
        "sha512" | "sha-512" => Ok(Sha512::digest(data).to_vec()),
        other => Err(format!("Digest method '{other}' not supported")),
    }
}

fn native_crypto_random_bytes(args: &Value) -> String {
    let size = args.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
    if size > MAX_RANDOM_BYTES {
        return json_err("randomBytes size exceeds 16 MiB bound");
    }
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        let block = Uuid::new_v4();
        let bytes = block.as_bytes();
        let take = (size - out.len()).min(bytes.len());
        out.extend_from_slice(&bytes[..take]);
    }
    json_ok(Value::String(BASE64_STD.encode(out)))
}

fn native_url_op(args: &Value) -> String {
    let sub_op = args.get("subOp").and_then(Value::as_str).unwrap_or("");
    let input = args.get("input").and_then(Value::as_str).unwrap_or("");
    match sub_op {
        "parse" => {
            let parsed = match args.get("base").and_then(Value::as_str) {
                Some(base_str) => Url::parse(base_str).and_then(|b| b.join(input)),
                None => Url::parse(input),
            };
            match parsed {
                Ok(u) => {
                    let protocol = format!("{}:", u.scheme());
                    let host = u.host_str().unwrap_or("").to_string();
                    let port = u.port().map(|p| p.to_string()).unwrap_or_default();
                    let host_with_port = if port.is_empty() {
                        host.clone()
                    } else {
                        format!("{host}:{port}")
                    };
                    let origin = if host.is_empty() {
                        "null".to_string()
                    } else {
                        format!("{protocol}//{host_with_port}")
                    };
                    let search = u.query().map(|q| format!("?{q}")).unwrap_or_default();
                    let hash = u.fragment().map(|f| format!("#{f}")).unwrap_or_default();
                    json_ok(json!({
                        "href": u.as_str(),
                        "origin": origin,
                        "protocol": protocol,
                        "username": u.username(),
                        "password": u.password().unwrap_or(""),
                        "host": host_with_port,
                        "hostname": host,
                        "port": port,
                        "pathname": u.path(),
                        "search": search,
                        "hash": hash,
                    }))
                }
                Err(err) => json_err_with_code(&format!("Invalid URL: {err}"), "ERR_INVALID_URL"),
            }
        }
        "fileURLToPath" => match Url::parse(input) {
            Ok(u) => match u.to_file_path() {
                Ok(path) => json_ok(Value::String(path.to_string_lossy().into_owned())),
                Err(()) => json_err_with_code("invalid file URL path", "ERR_INVALID_FILE_URL_PATH"),
            },
            Err(err) => json_err_with_code(&format!("Invalid URL: {err}"), "ERR_INVALID_URL"),
        },
        "pathToFileURL" => match Url::from_file_path(Path::new(input)) {
            Ok(u) => json_ok(Value::String(u.to_string())),
            Err(()) => json_err_with_code("path must be absolute", "ERR_INVALID_ARG_VALUE"),
        },
        _ => json_err(&format!("unsupported url_op: {sub_op}")),
    }
}

fn native_zlib_op(args: &Value) -> String {
    let sub_op = args.get("subOp").and_then(Value::as_str).unwrap_or("");
    let b64 = args.get("dataBase64").and_then(Value::as_str).unwrap_or("");
    let Ok(input) = BASE64_STD.decode(b64) else {
        return json_err("invalid base64 data for zlib_op");
    };

    let result: std::io::Result<Vec<u8>> = match sub_op {
        "gzip" => {
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(&input).and_then(|()| enc.finish())
        }
        "gunzip" => {
            let mut dec = GzDecoder::new(input.as_slice());
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map(|_| out)
        }
        "deflate" => {
            let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
            enc.write_all(&input).and_then(|()| enc.finish())
        }
        "inflate" => {
            let mut dec = ZlibDecoder::new(input.as_slice());
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map(|_| out)
        }
        "deflateRaw" => {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(&input).and_then(|()| enc.finish())
        }
        "inflateRaw" => {
            let mut dec = DeflateDecoder::new(input.as_slice());
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map(|_| out)
        }
        _ => return json_err(&format!("unsupported zlib_op: {sub_op}")),
    };

    match result {
        Ok(bytes) => json_ok(Value::String(BASE64_STD.encode(bytes))),
        Err(err) => json_err(&format!("zlib error: {err}")),
    }
}

fn native_child_process_op(args: &Value, capabilities: PreloadCapabilities) -> String {
    if !capabilities.allow_child_process {
        return json_err_with_code("EACCES: child_process capability disabled", "EACCES");
    }
    let cmd_str = args.get("command").and_then(Value::as_str).unwrap_or("");
    if cmd_str.trim().is_empty() {
        return json_err("child_process command must not be empty");
    }
    let shell = args.get("shell").and_then(Value::as_bool).unwrap_or(false);
    let cwd = args.get("cwd").and_then(Value::as_str);

    let mut command = if shell {
        #[cfg(windows)]
        let mut c = {
            let mut inner = Command::new("cmd");
            inner.args(["/C", cmd_str]);
            inner
        };
        #[cfg(not(windows))]
        let c = {
            let mut inner = Command::new("/bin/sh");
            inner.args(["-c", cmd_str]);
            inner
        };
        c
    } else {
        let mut c = Command::new(cmd_str);
        if let Some(arg_arr) = args.get("args").and_then(Value::as_array) {
            for item in arg_arr {
                if let Some(s) = item.as_str() {
                    c.arg(s);
                }
            }
        }
        c
    };

    if let Some(dir) = cwd {
        if !dir.is_empty() {
            command.current_dir(dir);
        }
    }
    if let Some(env_obj) = args.get("env").and_then(Value::as_object) {
        for (k, v) in env_obj {
            if let Some(vs) = v.as_str() {
                command.env(k, vs);
            }
        }
    }

    match command.output() {
        Ok(output) => json_ok(json!({
            "status": output.status.code().unwrap_or(-1),
            "stdoutBase64": BASE64_STD.encode(&output.stdout),
            "stderrBase64": BASE64_STD.encode(&output.stderr),
        })),
        Err(err) => map_io_err(err),
    }
}

/// Pure-Rust RFC 1321 MD5 digest for Node `crypto.createHash("md5")` compatibility.
fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301_u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476);
    for chunk in msg.chunks_exact(64) {
        let mut m = [0_u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let j = i * 4;
            *w = u32::from_le_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0_u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Pure-Rust FIPS 180-4 SHA-1 digest for Node `crypto.createHash("sha1")` compatibility.
fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let (mut h0, mut h1, mut h2, mut h3, mut h4) = (
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    );

    for chunk in msg.chunks_exact(64) {
        let mut w = [0_u32; 80];
        for (slot, bytes) in w[..16].iter_mut().zip(chunk.chunks_exact(4)) {
            *slot = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0_u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

enum RuntimeBackend {
    QuickJs(QuickJsNodeCompat),
    SystemNode(NodePreloadRuntime),
}

#[derive(Clone)]
struct LoadedSpec {
    preload_path: PathBuf,
    plugin_id: String,
    plugin_dir: PathBuf,
}

struct RuntimeSlot {
    backend: Option<RuntimeBackend>,
    loaded: Option<LoadedSpec>,
}

/// Unified 3-tier preload orchestrator (`WebViewOnly` -> `QuickJsNodeCompat` -> `SystemNodeFallback`) (`SPEC-015` v2.0).
pub struct PreloadRuntime {
    mode: PreloadExecutionMode,
    capabilities: PreloadCapabilities,
    node_config: NodeRuntimeConfig,
    handler: Arc<dyn HostBridgeHandler>,
    slot: Mutex<RuntimeSlot>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PreloadRuntime>();
};

impl std::fmt::Debug for PreloadRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreloadRuntime")
            .field("mode", &self.mode)
            .field("capabilities", &self.capabilities)
            .field("active_tier", &self.active_tier())
            .finish_non_exhaustive()
    }
}

impl Drop for PreloadRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl PreloadRuntime {
    /// Creates a [`PreloadRuntime`] with `mode`, `capabilities`, and host `handler`.
    #[must_use]
    pub fn new(
        mode: PreloadExecutionMode,
        capabilities: PreloadCapabilities,
        handler: Arc<dyn HostBridgeHandler>,
    ) -> Self {
        Self::with_node_config(mode, capabilities, NodeRuntimeConfig::default(), handler)
    }

    /// Creates a [`PreloadRuntime`] with explicit fallback [`NodeRuntimeConfig`] settings.
    #[must_use]
    pub fn with_node_config(
        mode: PreloadExecutionMode,
        capabilities: PreloadCapabilities,
        node_config: NodeRuntimeConfig,
        handler: Arc<dyn HostBridgeHandler>,
    ) -> Self {
        Self {
            mode,
            capabilities,
            node_config,
            handler,
            slot: Mutex::new(RuntimeSlot {
                backend: None,
                loaded: None,
            }),
        }
    }

    /// Returns the currently active [`PreloadTier`] (`WebViewOnly` when unloaded or shut down).
    #[must_use]
    pub fn active_tier(&self) -> PreloadTier {
        self.slot
            .lock()
            .ok()
            .and_then(|s| match s.backend {
                Some(RuntimeBackend::QuickJs(_)) => Some(PreloadTier::QuickJsNodeCompat),
                Some(RuntimeBackend::SystemNode(_)) => Some(PreloadTier::SystemNodeFallback),
                None => None,
            })
            .unwrap_or(PreloadTier::WebViewOnly)
    }

    /// Returns `true` if a preload runtime worker (Tier 2 or Tier 3) is currently active.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.slot
            .lock()
            .ok()
            .and_then(|s| match s.backend {
                Some(RuntimeBackend::QuickJs(ref q)) => Some(q.is_alive()),
                Some(RuntimeBackend::SystemNode(ref n)) => Some(n.is_alive()),
                None => None,
            })
            .unwrap_or(false)
    }

    /// Loads `preload_path` inside `plugin_dir`, always attempting Tier 2 [`PreloadTier::QuickJsNodeCompat`]
    /// first and falling back to Tier 3 [`PreloadTier::SystemNodeFallback`] only on explicit
    /// [`JsBridgeError::NodeCompatGap`] under [`PreloadExecutionMode::RustFirstWithFallback`] (`SPEC-015-R2`, `SPEC-015-R2b`).
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::NodeCompatGap`] under [`PreloadExecutionMode::PureRustOnly`] if the
    /// plugin requires unimplemented V8/native-addon APIs, or [`JsBridgeError::Io`] /
    /// [`JsBridgeError::QuickJs`] / [`JsBridgeError::NodeRuntime`] / [`JsBridgeError::Timeout`] on failure.
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

        match QuickJsNodeCompat::spawn_and_load(
            &canonical_preload,
            plugin_id,
            &canonical_dir,
            self.capabilities,
            Arc::clone(&self.handler),
            DEFAULT_CALL_TIMEOUT,
        ) {
            Ok((qjs, info)) => {
                let mut slot = self
                    .slot
                    .lock()
                    .map_err(|_| JsBridgeError::QuickJs("preload slot lock poisoned".into()))?;
                slot.backend = Some(RuntimeBackend::QuickJs(qjs));
                slot.loaded = Some(LoadedSpec {
                    preload_path: canonical_preload,
                    plugin_id: plugin_id.to_string(),
                    plugin_dir: canonical_dir,
                });
                Ok(info)
            }
            Err(JsBridgeError::NodeCompatGap {
                plugin_id: gap_pid,
                capability,
            }) => match self.mode {
                PreloadExecutionMode::PureRustOnly => Err(JsBridgeError::NodeCompatGap {
                    plugin_id: gap_pid,
                    capability,
                }),
                PreloadExecutionMode::RustFirstWithFallback => {
                    warn!(
                        target: "aitools::node_compat_gap",
                        plugin_id = %gap_pid,
                        capability = %capability,
                        "QuickJsNodeCompat hit NodeCompatGap; falling back to SystemNodeFallback"
                    );
                    let node_rt = NodePreloadRuntime::with_config(
                        self.node_config.clone(),
                        Arc::clone(&self.handler),
                    );
                    let info = node_rt.load(&canonical_preload, plugin_id, &canonical_dir)?;
                    let mut slot = self.slot.lock().map_err(|_| {
                        JsBridgeError::NodeRuntime("preload slot lock poisoned".into())
                    })?;
                    slot.backend = Some(RuntimeBackend::SystemNode(node_rt));
                    slot.loaded = Some(LoadedSpec {
                        preload_path: canonical_preload,
                        plugin_id: plugin_id.to_string(),
                        plugin_dir: canonical_dir,
                    });
                    Ok(info)
                }
            },
            Err(err) => Err(err),
        }
    }

    /// Dispatches feature entry (`window.exports[feature_code].args.enter` or `onPluginEnter`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, execution throws, or the call times out.
    pub fn enter(&self, feature_code: &str, action: &Value) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "enter",
                    json!({ "featureCode": feature_code, "action": action }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.enter(feature_code, action),
        )
    }

    /// Dispatches list-mode search (`window.exports[feature_code].args.search` or `setSubInput`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, execution throws, or the call times out.
    pub fn search(&self, feature_code: &str, action: &Value, query: &str) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "search",
                    json!({ "featureCode": feature_code, "action": action, "query": query }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.search(feature_code, action, query),
        )
    }

    /// Dispatches list-mode selection (`window.exports[feature_code].args.select`).
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, execution throws, or the call times out.
    pub fn select(&self, feature_code: &str, action: &Value, item: &Value) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "select",
                    json!({ "featureCode": feature_code, "action": action, "item": item }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.select(feature_code, action, item),
        )
    }

    /// Dispatches `onMainPush` callbacks registered by the plugin.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, execution throws, or the call times out.
    pub fn main_push(&self, feature_code: &str, action: &Value) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "main_push",
                    json!({ "featureCode": feature_code, "action": action }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.main_push(feature_code, action),
        )
    }

    /// Invokes a function or reads a property exposed on `window.<prop_path>` by `preload.js`.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, `prop_path` is missing/invalid,
    /// invocation throws, or the call times out.
    pub fn invoke_window_prop(&self, prop_path: &str, args: &[Value]) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "invoke_window_prop",
                    json!({ "propPath": prop_path, "args": args }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.invoke_window_prop(prop_path, args),
        )
    }

    /// Queries the current set of `window.*` properties exported by `preload.js`.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded or JSON decoding fails.
    pub fn get_exported_props(&self) -> Result<Vec<ExportedWindowProp>> {
        let slot = self
            .slot
            .lock()
            .map_err(|_| JsBridgeError::QuickJs("preload slot lock poisoned".into()))?;
        match slot.backend {
            Some(RuntimeBackend::QuickJs(ref qjs)) => {
                let val = qjs.call("get_exported_props", json!({}), DEFAULT_CALL_TIMEOUT)?;
                Ok(serde_json::from_value(val)?)
            }
            Some(RuntimeBackend::SystemNode(ref node)) => node.get_exported_props(),
            None => Err(JsBridgeError::QuickJs(
                "no active preload runtime loaded".into(),
            )),
        }
    }

    /// Dispatches `onPluginOut(is_kill)` listeners registered by `preload.js`.
    ///
    /// # Errors
    ///
    /// Returns an error if no preload runtime is loaded, execution throws, or the call times out.
    pub fn plugin_out(&self, is_kill: bool) -> Result<Value> {
        self.dispatch_with_fallback(
            |qjs| {
                qjs.call(
                    "plugin_out",
                    json!({ "isKill": is_kill }),
                    DEFAULT_CALL_TIMEOUT,
                )
            },
            |node| node.plugin_out(is_kill),
        )
    }

    /// Shuts down the active Tier 2 or Tier 3 runtime and resets [`Self::active_tier`] to [`PreloadTier::WebViewOnly`].
    ///
    /// # Errors
    ///
    /// Returns an error if internal state locks are poisoned or process teardown fails.
    pub fn shutdown(&self) -> Result<()> {
        let taken = {
            let mut slot = self
                .slot
                .lock()
                .map_err(|_| JsBridgeError::QuickJs("preload slot lock poisoned".into()))?;
            slot.loaded = None;
            slot.backend.take()
        };
        match taken {
            Some(RuntimeBackend::QuickJs(qjs)) => {
                qjs.shutdown();
                Ok(())
            }
            Some(RuntimeBackend::SystemNode(node)) => node.shutdown(),
            None => Ok(()),
        }
    }

    fn dispatch_with_fallback<FQ, FN>(&self, on_qjs: FQ, on_node: FN) -> Result<Value>
    where
        FQ: FnOnce(&QuickJsNodeCompat) -> Result<Value>,
        FN: FnOnce(&NodePreloadRuntime) -> Result<Value>,
    {
        let mut slot = self
            .slot
            .lock()
            .map_err(|_| JsBridgeError::QuickJs("preload slot lock poisoned".into()))?;

        match slot.backend {
            Some(RuntimeBackend::SystemNode(ref node)) => on_node(node),
            Some(RuntimeBackend::QuickJs(ref qjs)) => match on_qjs(qjs) {
                Ok(v) => Ok(v),
                Err(JsBridgeError::NodeCompatGap {
                    plugin_id,
                    capability,
                }) => {
                    if self.mode == PreloadExecutionMode::PureRustOnly {
                        return Err(JsBridgeError::NodeCompatGap {
                            plugin_id,
                            capability,
                        });
                    }
                    let Some(spec) = slot.loaded.clone() else {
                        return Err(JsBridgeError::NodeCompatGap {
                            plugin_id,
                            capability,
                        });
                    };
                    warn!(
                        target: "aitools::node_compat_gap",
                        plugin_id = %plugin_id,
                        capability = %capability,
                        "QuickJsNodeCompat hit lazy NodeCompatGap; promoting to SystemNodeFallback"
                    );
                    if let Some(RuntimeBackend::QuickJs(old_qjs)) = slot.backend.take() {
                        old_qjs.shutdown();
                    }
                    let node_rt = NodePreloadRuntime::with_config(
                        self.node_config.clone(),
                        Arc::clone(&self.handler),
                    );
                    node_rt.load(&spec.preload_path, &spec.plugin_id, &spec.plugin_dir)?;
                    let res = on_node(&node_rt);
                    slot.backend = Some(RuntimeBackend::SystemNode(node_rt));
                    res
                }
                Err(err) => Err(err),
            },
            None => Err(JsBridgeError::QuickJs(
                "no active preload runtime loaded".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_and_sha1_match_rfc_vectors() {
        assert_eq!(
            hex::encode(md5_digest(b"")),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            hex::encode(md5_digest(b"hello")),
            "5d41402abc4b2a76b9719d911017c592"
        );
        assert_eq!(
            hex::encode(sha1_digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }
}
