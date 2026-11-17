//! `tauri-plugin-aitools` — Integrates core, native, AI, JS bridge, and Wasm host into Tauri v2 (`SPEC-000`, `SPEC-004`, `SPEC-006`).

pub mod protocols;

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use aitools_ai::broker::ModelBroker;
use aitools_ai::context_fusion::{suggest_zero_query_actions, ContextEnvelope, QuickAgentAction};
use aitools_ai::hitl::PolicyGuard;
use aitools_ai::launcher::{evaluate_launcher_input, LauncherDecision};
use aitools_ai::planner::{execute_dag, DagEvent, DagPlan, HitlBroker, HitlResolution};
use aitools_ai::registry::ToolRegistry;
use aitools_core::{
    Document, FeatureEntry, FeatureSearchIndex, PluginDataQuery, PluginDataRemove,
    PluginDataWriteResult, StorageEngine,
};
use aitools_js_bridge::{
    build_initialization_script, invoke_preload_list_feature, CallbackTable, JsBridgeHost,
    ListPushEvent, ListPushSink,
};
use aitools_telemetry::{LogRecord, TelemetryHandle};
use aitools_wasm_host::WasmPluginHost;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::http::{header::CONTENT_TYPE, Response};
use tauri::ipc::Channel;
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use thiserror::Error;

use crate::protocols::{
    handle_plugin_asset_request, handle_sync_request, FaultInjectionConfig, PluginMountRegistry,
    ProtocolReply,
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
    pub search_index: RwLock<FeatureSearchIndex>,
    /// Table of active streaming list callbacks for legacy preload features.
    pub callback_table: CallbackTable,
    /// QuickJS-backed host for legacy Tier 1 preload plugins.
    pub js_bridge: Mutex<JsBridgeHost>,
    /// Sandboxed Wasmtime host for Tier 3 plugins.
    pub wasm_host: WasmPluginHost,
    /// Registry of built-in and plugin-provided agent tools.
    pub tool_registry: RwLock<ToolRegistry>,
    /// Centralized multi-provider AI model & quota broker.
    pub model_broker: RwLock<ModelBroker>,
    /// Risk-tier policy guard enforcing HITL approvals.
    pub policy_guard: Arc<PolicyGuard>,
    /// Broker coordinating pending HITL confirmation steps.
    pub hitl_broker: Arc<HitlBroker>,
    /// Runtime DSH fault injection toggles.
    pub faults: Mutex<FaultInjectionConfig>,
    /// Most recent pre-focus context captured on global wake.
    pub latest_context: Mutex<Option<ContextEnvelope>>,
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
        let storage = Arc::new(StorageEngine::open(db_dir)?);
        let search_index = FeatureSearchIndex::try_build(initial_features)?;
        let wasm_host = WasmPluginHost::with_defaults()?;

        let mut registry = ToolRegistry::new();
        registry.register_builtin_os_defaults();

        Ok(Self {
            storage,
            telemetry,
            mounts: PluginMountRegistry::new(),
            search_index: RwLock::new(search_index),
            callback_table: CallbackTable::new(),
            js_bridge: Mutex::new(JsBridgeHost::new()),
            wasm_host,
            tool_registry: RwLock::new(registry),
            model_broker: RwLock::new(ModelBroker::new()),
            policy_guard: Arc::new(PolicyGuard::new()),
            hitl_broker: Arc::new(HitlBroker::new()),
            faults: Mutex::new(FaultInjectionConfig::default()),
            latest_context: Mutex::new(None),
        })
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
    let label = format!("plugin-{plugin_id}");
    let url = WebviewUrl::CustomProtocol(parse_plugin_url(plugin_id, entry_path)?);
    let init_script = build_initialization_script(plugin_id);

    Ok(WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(width, height)
        .initialization_script(&init_script)
        .build()?)
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
fn search_features(
    query: String,
    tab_pressed: bool,
    submitted: bool,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<LauncherDecision, String> {
    let index = state
        .search_index
        .read()
        .map_err(|_| "search index lock poisoned".to_string())?;
    evaluate_launcher_input(&index, &query, tab_pressed, submitted).map_err(|e| e.to_string())
}

#[tauri::command]
fn zero_query_suggestions(
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<QuickAgentAction>, String> {
    let envelope = state
        .latest_context
        .lock()
        .map_err(|_| "context lock poisoned".to_string())?
        .clone()
        .unwrap_or_default();
    Ok(suggest_zero_query_actions(&envelope))
}

#[tauri::command]
async fn run_agent_dag(
    plan: DagPlan,
    on_event: Channel<DagEvent>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<HashMapStringValue, String> {
    let faults = state
        .faults
        .lock()
        .map_err(|_| "fault lock poisoned".to_string())?
        .clone();
    if faults.inject_llm_stream_abort {
        return Err("DSH fault injected: LLM stream aborted".to_string());
    }

    let storage = Arc::clone(&state.storage);
    let guard = Arc::clone(&state.policy_guard);
    let broker = Arc::clone(&state.hitl_broker);

    let executor = Arc::new(move |tool_name: String, args: Value| {
        let storage = Arc::clone(&storage);
        Box::pin(async move {
            match tool_name.as_str() {
                "os.clipboard.read" => Ok(json!({ "text": "" })),
                "os.files.find" => Ok(json!({ "matches": [] })),
                _ => {
                    let _ = &storage;
                    Ok(json!({ "executed": tool_name, "args": args }))
                }
            }
        }) as aitools_ai::planner::ToolExecFuture
    });

    execute_dag(
        &plan,
        guard,
        broker,
        move |ev| {
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
fn resolve_hitl_step(
    node_id: String,
    resolution: HitlResolution,
    state: tauri::State<'_, AitoolsState>,
) -> bool {
    state.hitl_broker.resolve(&node_id, resolution)
}

#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri IPC command parameters map directly to flat JSON invoke keys"
)]
fn invoke_legacy_list_feature(
    plugin_id: String,
    trace_id: String,
    preload_code: String,
    feature_code: String,
    action_payload: Value,
    search_query: Option<String>,
    channel: Channel<ListPushEvent>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<u64, String> {
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
fn dsh_recent_logs(
    limit: usize,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<LogRecord>, String> {
    let Some(ref telemetry) = state.telemetry else {
        return Ok(Vec::new());
    };
    Ok(telemetry.recent_records(limit.min(500)))
}

#[tauri::command]
fn dsh_ingest_frontend_log(
    payload: FrontendLogPayload,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<(), String> {
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
fn dsh_set_fault_injection(
    config: FaultInjectionConfig,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<FaultInjectionConfig, String> {
    let mut guard = state
        .faults
        .lock()
        .map_err(|_| "fault lock poisoned".to_string())?;
    *guard = config.clone();
    Ok(config)
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
fn ai_chat<R: Runtime>(
    request: SdkChatRequest,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<String, String> {
    let plugin_id = plugin_id_from_label(webview.label())?;
    let est_tokens =
        ((request.prompt.len() + request.system.as_deref().unwrap_or("").len()) / 4).max(1) as u64;
    let broker = state
        .model_broker
        .read()
        .map_err(|_| "model broker lock poisoned".to_string())?;
    broker
        .check_and_consume_quota(plugin_id, est_tokens, Instant::now())
        .map_err(|e| e.to_string())?;
    let profile = request.profile.as_deref().unwrap_or("fast");
    Ok(format!(
        "[{profile}] processed {} chars for plugin '{plugin_id}'",
        request.prompt.len()
    ))
}

#[tauri::command]
fn ai_embed<R: Runtime>(
    texts: Vec<String>,
    webview: tauri::Webview<R>,
    state: tauri::State<'_, AitoolsState>,
) -> std::result::Result<Vec<Vec<f32>>, String> {
    let plugin_id = plugin_id_from_label(webview.label())?;
    let total_chars: usize = texts.iter().map(String::len).sum();
    let est_tokens = (total_chars / 4).max(1) as u64;
    let broker = state
        .model_broker
        .read()
        .map_err(|_| "model broker lock poisoned".to_string())?;
    broker
        .check_and_consume_quota(plugin_id, est_tokens, Instant::now())
        .map_err(|e| e.to_string())?;
    Ok(texts
        .into_iter()
        .map(|t| vec![t.len() as f32, 1.0])
        .collect())
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
        .register_asynchronous_uri_scheme_protocol("aitools-plugin", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            let uri = request.uri().to_string();
            let state = ctx.app_handle().state::<AitoolsState>();
            let reply = handle_plugin_asset_request(&label, &uri, &state.mounts);
            responder.respond(to_http_response(reply));
        })
        .register_asynchronous_uri_scheme_protocol("aitools-sync", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            let uri = request.uri().to_string();
            let body = request.body().clone();
            let state = ctx.app_handle().state::<AitoolsState>();
            let reply = handle_sync_request(&label, &uri, &body, &state.storage, &state.faults);
            responder.respond(to_http_response(reply));
        })
        .invoke_handler(tauri::generate_handler![
            search_features,
            zero_query_suggestions,
            run_agent_dag,
            resolve_hitl_step,
            invoke_legacy_list_feature,
            dsh_recent_logs,
            dsh_ingest_frontend_log,
            dsh_set_fault_injection,
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
}
