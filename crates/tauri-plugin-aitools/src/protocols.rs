//! Protocol handlers for `aitools-plugin://` and `aitools-sync://` (`INV-SEC-01`, `SPEC-002-R4`, `SPEC-006-R3`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use aitools_core::storage::{
    validate_sync_caller, PluginDataQuery, PluginDataRemove, StorageEngine,
};
use aitools_core::{AitoolsError, Document};
use aitools_native::app_catalog::render_app_icon_png_128;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::info_span;

use crate::session::PluginSessionManager;

/// Reserved `aitools-plugin://` authority serving host-cached `128×128` PNG app/plugin icons (`SPEC-016-R1`).
pub const APP_ICON_HOST: &str = "__app_icon__";

/// DSH runtime fault injection switches (`SPEC-013-R3`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultInjectionConfig {
    /// Simulates an LMDB revision conflict on `db/put` requests.
    pub inject_lmdb_rev_conflict: bool,
    /// Simulates a permission-denied failure for diagnostic testing.
    pub inject_permission_denied: bool,
    /// Simulates a Wasmtime fuel exhaustion failure.
    pub inject_wasm_fuel_exhaustion: bool,
    /// Simulates an LLM stream abort in `run_agent_dag`.
    pub inject_llm_stream_abort: bool,
}

/// Pure HTTP-shaped protocol response independent of Tauri window runtime for fast unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolReply {
    /// HTTP status code returned by the custom protocol handler.
    pub status: u16,
    /// MIME content type header value.
    pub content_type: &'static str,
    /// Raw response body bytes.
    pub body: Vec<u8>,
}

impl ProtocolReply {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    fn text(status: u16, msg: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: msg.as_bytes().to_vec(),
        }
    }
}

/// Registry of installed plugin filesystem roots and cached application icons for `aitools-plugin://` resolution.
#[derive(Debug, Clone, Default)]
pub struct PluginMountRegistry {
    roots: Arc<RwLock<HashMap<String, PathBuf>>>,
    icons: Arc<RwLock<HashMap<String, PathBuf>>>,
    icon_cache_dir: Arc<RwLock<Option<PathBuf>>>,
}

impl PluginMountRegistry {
    /// Creates an empty plugin mount and icon registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the directory where rendered `128×128` PNG icons are cached on disk.
    pub fn set_icon_cache_dir(&self, dir: impl AsRef<Path>) {
        if let Ok(mut guard) = self.icon_cache_dir.write() {
            *guard = Some(dir.as_ref().to_path_buf());
        }
    }

    /// Returns the configured icon cache directory, if set.
    #[must_use]
    pub fn icon_cache_dir(&self) -> Option<PathBuf> {
        self.icon_cache_dir.read().ok().and_then(|g| g.clone())
    }

    /// Canonicalizes and registers `root` as the asset directory for `plugin_id`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if `plugin_id` is reserved, `root` cannot be canonicalized,
    /// or the internal lock is poisoned.
    pub fn register_root(&self, plugin_id: &str, root: impl AsRef<Path>) -> std::io::Result<()> {
        if plugin_id == APP_ICON_HOST {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "__app_icon__ is a reserved protocol host",
            ));
        }
        let canonical = std::fs::canonicalize(root)?;
        let mut guard = self
            .roots
            .write()
            .map_err(|_| std::io::Error::other("plugin mount lock poisoned"))?;
        guard.insert(plugin_id.to_string(), canonical);
        Ok(())
    }

    /// Returns the canonicalized asset root registered for `plugin_id`, if any.
    #[must_use]
    pub fn get_root(&self, plugin_id: &str) -> Option<PathBuf> {
        self.roots
            .read()
            .ok()
            .and_then(|g| g.get(plugin_id).cloned())
    }

    /// Registers a source icon file path and returns its `aitools-plugin://__app_icon__/<id>.png` URI.
    ///
    /// Returns `None` if `source_path` does not point to an existing regular file.
    #[must_use]
    pub fn register_icon(&self, source_path: impl AsRef<Path>) -> Option<String> {
        let raw = source_path.as_ref();
        if !raw.is_file() {
            return None;
        }
        let canonical = std::fs::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf());
        let icon_id = icon_id_for_path(&canonical);
        if self
            .icons
            .read()
            .ok()
            .is_some_and(|g| g.contains_key(&icon_id))
        {
            return Some(format!("aitools-plugin://{APP_ICON_HOST}/{icon_id}.png"));
        }
        if let Ok(mut guard) = self.icons.write() {
            guard.insert(icon_id.clone(), canonical);
            return Some(format!("aitools-plugin://{APP_ICON_HOST}/{icon_id}.png"));
        }
        None
    }

    /// Looks up the source path registered for a 16-char hex `icon_id`.
    #[must_use]
    pub fn get_icon_source(&self, icon_id: &str) -> Option<PathBuf> {
        self.icons.read().ok().and_then(|g| g.get(icon_id).cloned())
    }
}

fn icon_id_for_path(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}

fn parse_valid_icon_id_filename(rel_path: &str) -> Option<&str> {
    let stem = rel_path.strip_suffix(".png")?;
    if stem.len() != 16 || !stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    Some(stem)
}

fn is_cached_icon_fresh(source_path: &Path, cache_path: &Path) -> bool {
    let Ok(cache_meta) = std::fs::metadata(cache_path) else {
        return false;
    };
    if !cache_meta.is_file() || cache_meta.len() == 0 {
        return false;
    }
    let Ok(src_meta) = std::fs::metadata(source_path) else {
        return false;
    };
    match (src_meta.modified(), cache_meta.modified()) {
        (Ok(src_mtime), Ok(cache_mtime)) => cache_mtime >= src_mtime,
        _ => true,
    }
}

fn handle_app_icon_request(
    webview_label: &str,
    rel_path: &str,
    mounts: &PluginMountRegistry,
) -> ProtocolReply {
    if webview_label != "main" && webview_label != "dsh" {
        return ProtocolReply::text(
            403,
            "forbidden (403): __app_icon__ is restricted to host webviews",
        );
    }

    let Some(icon_id) = parse_valid_icon_id_filename(rel_path) else {
        return ProtocolReply::text(403, "forbidden (403): invalid __app_icon__ path");
    };

    let Some(source_path) = mounts.get_icon_source(icon_id) else {
        return ProtocolReply::text(403, "forbidden (403): unregistered app icon id");
    };

    let _span = info_span!("app_icon_asset", icon_id = %icon_id).entered();
    let cache_dir = mounts
        .icon_cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("aitools-icon-cache"));
    let cache_path = cache_dir.join(format!("{icon_id}.png"));

    if !is_cached_icon_fresh(&source_path, &cache_path) {
        if let Err(err) = render_app_icon_png_128(&source_path, &cache_path) {
            return ProtocolReply::text(404, &format!("unable to render icon: {err}"));
        }
    }

    match std::fs::read(&cache_path) {
        Ok(bytes) => ProtocolReply {
            status: 200,
            content_type: "image/png",
            body: bytes,
        },
        Err(_) => ProtocolReply::text(404, "unable to read cached icon"),
    }
}

/// Serves `aitools-plugin://<plugin_id>/<asset_path>` (and host-only `aitools-plugin://__app_icon__/<id>.png`)
/// with `INV-SEC-01` & `SPEC-006-R3` canonicalization.
pub fn handle_plugin_asset_request(
    webview_label: &str,
    raw_uri: &str,
    mounts: &PluginMountRegistry,
) -> ProtocolReply {
    let Some((plugin_id, rel_path)) = parse_scheme_authority_and_path(raw_uri, "aitools-plugin")
    else {
        return ProtocolReply::text(400, "invalid aitools-plugin URI");
    };

    if plugin_id == APP_ICON_HOST {
        return handle_app_icon_request(webview_label, &rel_path, mounts);
    }

    if let Err(err) = validate_sync_caller(webview_label, &plugin_id) {
        return ProtocolReply::text(403, &err.to_string());
    }

    let _span = info_span!("plugin_asset", plugin_id = %plugin_id, path = %rel_path).entered();
    let Some(root) = mounts.get_root(&plugin_id) else {
        return ProtocolReply::text(404, "plugin root not registered");
    };

    let decoded = percent_decode_str(&rel_path).decode_utf8_lossy();
    let rel_clean = decoded.trim_start_matches('/');
    let rel_clean = if rel_clean.is_empty() {
        "index.html"
    } else {
        rel_clean
    };
    let candidate = root.join(rel_clean);
    let Ok(canonical_target) = std::fs::canonicalize(&candidate) else {
        return ProtocolReply::text(404, "asset not found");
    };

    // SPEC-006-R3: enforce canonicalized containment under plugin root to block `..` traversal.
    if !canonical_target.starts_with(&root) {
        return ProtocolReply::text(403, "path traversal rejected");
    }

    match std::fs::read(&canonical_target) {
        Ok(bytes) => ProtocolReply {
            status: 200,
            content_type: guess_mime(&canonical_target),
            body: bytes,
        },
        Err(_) => ProtocolReply::text(404, "unable to read asset"),
    }
}

#[derive(Deserialize)]
struct SyncPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    doc: Option<Document>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    keys: Option<Vec<String>>,
    #[serde(default)]
    trace_id: Option<String>,
}

/// Serves `aitools-sync://<plugin_id>/<op>` via [`PluginSessionManager`] with manifest permission checks
/// and full ZTools/DB/OS/clipboard/window/AI/preload RPC support (`INV-SEC-01`, `SPEC-015-R5`/`R6`, `SPEC-016-R4`).
pub fn handle_sync_request_with_session(
    webview_label: &str,
    raw_uri: &str,
    body: &[u8],
    sessions: &PluginSessionManager,
) -> ProtocolReply {
    let Some((plugin_id, op)) = parse_scheme_authority_and_path(raw_uri, "aitools-sync") else {
        return ProtocolReply::json(400, &json!({ "error": "invalid aitools-sync URI" }));
    };

    if let Err(err) = validate_sync_caller(webview_label, &plugin_id) {
        return ProtocolReply::json(403, &json!({ "error": err.to_string() }));
    }

    let _span = info_span!("aitools_sync", plugin_id = %plugin_id, op = %op).entered();
    let payload: Value = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(err) => {
                return ProtocolReply::json(
                    400,
                    &json!({ "error": format!("invalid JSON request body: {err}") }),
                );
            }
        }
    };

    match sessions.exec_sync_op(&plugin_id, &op, payload, true) {
        Ok(value) => ProtocolReply::json(200, &value),
        Err(err) => {
            let mut body_val = json!({
                "error": true,
                "message": err.message,
            });
            if let Some(name) = err.name {
                body_val["name"] = Value::String(name);
            }
            ProtocolReply::json(err.status, &body_val)
        }
    }
}

/// Serves `aitools-sync://<plugin_id>/<op>` directly from `heed` LMDB (`INV-SEC-01`, `SPEC-002-R4`).
pub fn handle_sync_request(
    webview_label: &str,
    raw_uri: &str,
    body: &[u8],
    storage: &StorageEngine,
    faults: &Mutex<FaultInjectionConfig>,
) -> ProtocolReply {
    let Some((plugin_id, op)) = parse_scheme_authority_and_path(raw_uri, "aitools-sync") else {
        return ProtocolReply::json(400, &json!({ "error": "invalid aitools-sync URI" }));
    };

    if let Err(err) = validate_sync_caller(webview_label, &plugin_id) {
        return ProtocolReply::json(403, &json!({ "error": err.to_string() }));
    }

    let payload: SyncPayload = if body.is_empty() {
        SyncPayload {
            id: None,
            doc: None,
            key: None,
            keys: None,
            trace_id: None,
        }
    } else {
        match serde_json::from_slice(body) {
            Ok(p) => p,
            Err(err) => {
                return ProtocolReply::json(
                    400,
                    &json!({ "error": format!("invalid sync JSON: {err}") }),
                )
            }
        }
    };

    let trace_id = payload.trace_id.as_deref().unwrap_or("sync");
    let _span =
        info_span!("db_sync", trace_id = %trace_id, plugin_id = %plugin_id, op = %op).entered();

    let inject_conflict = faults
        .lock()
        .is_ok_and(|guard| guard.inject_lmdb_rev_conflict);
    if inject_conflict && op == "db/put" {
        return ProtocolReply::json(
            200,
            &json!({
                "id": payload.doc.as_ref().map(|d| d.id.as_str()).unwrap_or("injected"),
                "error": true,
                "name": "conflict",
                "message": "DSH fault injected: Document update conflict"
            }),
        );
    }

    match op.as_str() {
        "db/get" => {
            let Some(id) = payload.id.as_deref() else {
                return ProtocolReply::json(400, &json!({ "error": "missing document id" }));
            };
            match storage.get_doc(&plugin_id, id) {
                Ok(Some(doc)) => {
                    ProtocolReply::json(200, &serde_json::to_value(doc).unwrap_or(Value::Null))
                }
                Ok(None) => ProtocolReply::json(200, &Value::Null),
                Err(err) => map_storage_error(err),
            }
        }
        "db/put" => {
            let doc = if let Some(doc) = payload.doc {
                doc
            } else {
                match serde_json::from_slice::<Document>(body) {
                    Ok(doc) => doc,
                    Err(_) => {
                        return ProtocolReply::json(
                            400,
                            &json!({ "error": "missing Document body" }),
                        )
                    }
                }
            };
            match storage.put_doc(&plugin_id, doc) {
                Ok(res) => {
                    ProtocolReply::json(200, &serde_json::to_value(res).unwrap_or(Value::Null))
                }
                Err(err) => map_storage_error(err),
            }
        }
        "db/remove" => {
            let target = if let Some(doc) = payload.doc {
                PluginDataRemove::Document(doc)
            } else if let Some(id) = payload.id {
                PluginDataRemove::Id(id)
            } else {
                return ProtocolReply::json(400, &json!({ "error": "missing id or doc" }));
            };
            match storage.remove_doc(&plugin_id, target) {
                Ok(res) => {
                    ProtocolReply::json(200, &serde_json::to_value(res).unwrap_or(Value::Null))
                }
                Err(err) => map_storage_error(err),
            }
        }
        "db/allDocs" => {
            let query = if let Some(keys) = payload.keys {
                PluginDataQuery::Keys(keys)
            } else if let Some(prefix) = payload.key {
                PluginDataQuery::Prefix(prefix)
            } else {
                PluginDataQuery::All
            };
            match storage.all_docs(&plugin_id, query) {
                Ok(docs) => ProtocolReply::json(
                    200,
                    &serde_json::to_value(docs).unwrap_or_else(|_| Value::Array(vec![])),
                ),
                Err(err) => map_storage_error(err),
            }
        }
        _ => ProtocolReply::json(404, &json!({ "error": format!("unknown sync op: {op}") })),
    }
}

fn map_storage_error(err: AitoolsError) -> ProtocolReply {
    let status = match err {
        AitoolsError::Forbidden(_) => 403,
        AitoolsError::BudgetExceeded(_) => 413,
        _ => 400,
    };
    ProtocolReply::json(status, &json!({ "error": err.to_string() }))
}

fn parse_scheme_authority_and_path(raw_uri: &str, scheme: &str) -> Option<(String, String)> {
    let prefix = format!("{scheme}://");
    let rest = raw_uri.strip_prefix(&prefix)?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if let Some((host, path)) = rest.split_once('/') {
        if host.is_empty() {
            return None;
        }
        Some((host.to_string(), path.to_string()))
    } else if !rest.is_empty() {
        Some((rest.to_string(), String::new()))
    } else {
        None
    }
}

fn guess_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
