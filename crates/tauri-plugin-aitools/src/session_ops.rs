use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aitools_ai::broker::{ExecutionBackend, ModelProfileKind};
use aitools_ai::launcher::LauncherHit;
use aitools_core::{
    AttachmentRecord, Cmd, Document, Feature, LoadedPlugin, PluginDataQuery, PluginDataRemove,
    PluginDataWriteResult, PluginRuntimeTier, StorageEngine,
};
use aitools_js_bridge::{CallbackTable, ListItem, ListPushEvent, ListPushSink, PreloadLoadInfo};
use aitools_native::app_catalog::launch_application;
use aitools_native::context::{capture_frontmost_snapshot, CopiedFileEntry};
use aitools_native::icon_analysis::{analyze_icon_data_url, analyze_icon_path};
use aitools_wasm_host::{WasmInstanceHandle, WasmPluginHost};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::info_span;

use crate::session::{
    block_on_async, now_unix_ms, ClipboardHistoryItem, HomeSectionsResponse, HostLauncherState,
    KeychainApiKeyProvider, LaunchResult, LauncherItemRef, LauncherStateMutation, ListModeSession,
    ListSearchResponse, PluginSessionManager, SelectListResult, SyncOpError,
    DEFAULT_HTTP_TIMEOUT_SECS, MAX_CLIPBOARD_HISTORY,
};

const HOST_LAUNCHER_BUCKET: &str = "__host_launcher__";
const DOC_RECENT_HISTORY: &str = "recent_history";
const DOC_PINNED_ITEMS: &str = "pinned_items";
const DOC_RECOMMENDATION_PINNED: &str = "recommendation_pinned";
const DOC_USAGE_STATS: &str = "usage_stats";
const DOC_REMOVED_RECENT_KEYS: &str = "removed_recent_keys";
const MAX_RECENT_HISTORY: usize = 36;
const MAX_ICON_ANALYSIS_CACHE: usize = 4096;

impl PluginSessionManager {
    /// Handles `callbackSetList(items)` emitted by Tier-2/Tier-3 preload runtimes.
    pub fn handle_callback_set_list(&self, plugin_id: &str, items: Vec<ListItem>) {
        let session_id = self
            .active_list_session_by_plugin
            .lock()
            .ok()
            .and_then(|g| g.get(plugin_id).cloned())
            .unwrap_or_else(|| format!("list-{plugin_id}"));

        let mut feature_code = plugin_id.to_string();
        if let Ok(mut sessions) = self.list_sessions.lock() {
            if let Some(sess) = sessions.get_mut(&session_id) {
                sess.items = items.clone();
                feature_code = sess.feature_code.clone();
            } else {
                sessions.insert(
                    session_id.clone(),
                    ListModeSession {
                        feature_code: feature_code.clone(),
                        items: items.clone(),
                        out_plugin: false,
                        hide_main: false,
                    },
                );
            }
        }

        let handle = CallbackTable::new().register(plugin_id, &feature_code, "list");
        let event = ListPushEvent {
            trace_id: session_id.clone(),
            plugin_id: plugin_id.to_string(),
            handle,
            items: items.clone(),
        };
        if let Ok(sinks) = self.active_list_sinks.lock() {
            if let Some(sink) = sinks.get(plugin_id) {
                sink.push_list(event.clone());
            }
        }
        self.window_controller().emit_event(
            "aitools://list-push",
            serde_json::to_value(event).unwrap_or(Value::Null),
        );
    }

    /// Unified dispatcher serving both `aitools-sync://<plugin_id>/<op>` and preload `HostBridgeHandler` calls (`SPEC-015-R5`/`R6`).
    ///
    /// # Errors
    ///
    /// Returns [`SyncOpError`] mapped to HTTP status codes (`400`, `403`, `404`, `409`, `413`, `500`).
    pub fn exec_sync_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
        check_permissions: bool,
    ) -> std::result::Result<Value, SyncOpError> {
        let _span = info_span!("sync_op", plugin_id = %plugin_id, op = %op).entered();
        let fault_denied = self.faults.lock().is_ok_and(|f| f.inject_permission_denied);
        if fault_denied {
            return Err(SyncOpError::forbidden(
                "DSH fault injected: permission denied",
            ));
        }

        if check_permissions {
            self.check_sync_permission(plugin_id, op)?;
        }

        if let Some(db_sub) = op.strip_prefix("db/") {
            return self.exec_db_op(plugin_id, db_sub, payload);
        }
        if let Some(clip_sub) = op.strip_prefix("clipboard/") {
            return self.exec_clipboard_op(clip_sub, payload);
        }
        if op == "notification/show" {
            let body = payload
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let title = payload
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(plugin_id);
            let click_code = payload.get("clickFeatureCode").and_then(Value::as_str);
            self.window_controller()
                .show_notification(title, body, click_code)
                .map_err(|e| SyncOpError::new(500, e))?;
            return Ok(json!(true));
        }
        if let Some(win_sub) = op.strip_prefix("window/") {
            return self.exec_window_op(plugin_id, win_sub, payload);
        }
        if let Some(shell_sub) = op.strip_prefix("shell/") {
            return self.exec_shell_op(shell_sub, payload);
        }
        if let Some(sys_sub) = op.strip_prefix("system/") {
            return self.exec_system_op(plugin_id, sys_sub, payload);
        }
        if let Some(disp_sub) = op.strip_prefix("display/") {
            return Ok(exec_display_op(disp_sub, &payload));
        }
        if let Some(feat_sub) = op.strip_prefix("features/") {
            return self.exec_features_op(plugin_id, feat_sub, payload);
        }
        if let Some(dlg_sub) = op.strip_prefix("dialog/") {
            return self.exec_dialog_op(dlg_sub, &payload);
        }
        if let Some(ai_sub) = op.strip_prefix("ai/") {
            return self.exec_ai_op(plugin_id, ai_sub, payload);
        }
        if let Some(http_sub) = op.strip_prefix("http/") {
            return self.execute_http_request(plugin_id, http_sub, &payload);
        }
        if op == "preload/invoke" {
            let prop_path = payload
                .get("propPath")
                .and_then(Value::as_str)
                .ok_or_else(|| SyncOpError::bad_request("missing preload propPath"))?;
            let args: Vec<Value> = payload
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let (rt, _) = self.get_or_load_preload_session(plugin_id)?;
            return rt
                .invoke_window_prop(prop_path, &args)
                .map_err(|e| SyncOpError::new(500, format!("preload/invoke failed: {e}")));
        }
        if op == "preload/props" {
            let plugin = self.loaded_plugin(plugin_id);
            if plugin.and_then(|p| p.preload).is_none() {
                return Ok(json!([]));
            }
            let (_, info) = self.get_or_load_preload_session(plugin_id)?;
            return serde_json::to_value(info.exported_props)
                .map_err(|e| SyncOpError::new(500, e.to_string()));
        }

        Err(SyncOpError::not_found(format!("unknown sync op: {op}")))
    }

    fn exec_db_op(
        &self,
        plugin_id: &str,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        let inject_conflict = self.faults.lock().is_ok_and(|f| f.inject_lmdb_rev_conflict);

        match sub {
            "get" => {
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing document id"))?;
                let doc = self.storage.get_doc(plugin_id, id).map_err(map_core_err)?;
                Ok(serde_json::to_value(doc).unwrap_or(Value::Null))
            }
            "put" => {
                let doc: Document = if let Some(inner) = payload.get("doc") {
                    serde_json::from_value(inner.clone())
                        .map_err(|e| SyncOpError::bad_request(format!("invalid doc: {e}")))?
                } else {
                    serde_json::from_value(payload)
                        .map_err(|e| SyncOpError::bad_request(format!("invalid doc: {e}")))?
                };
                if inject_conflict {
                    let failure = conflict_write_result(
                        &doc.id,
                        "DSH fault injected: LMDB revision conflict",
                    );
                    return Ok(serde_json::to_value(failure).unwrap_or(Value::Null));
                }
                let res = self.storage.put_doc(plugin_id, doc).map_err(map_core_err)?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            }
            "remove" => {
                let target = if let Some(inner) = payload.get("doc") {
                    let doc: Document = serde_json::from_value(inner.clone())
                        .map_err(|e| SyncOpError::bad_request(format!("invalid doc: {e}")))?;
                    PluginDataRemove::Document(doc)
                } else if let Some(id) = payload.get("id").and_then(Value::as_str) {
                    if let Some(rev) = payload.get("rev").and_then(Value::as_str) {
                        PluginDataRemove::Document(Document {
                            id: id.to_string(),
                            rev: Some(rev.to_string()),
                            data: json!({}),
                        })
                    } else {
                        PluginDataRemove::Id(id.to_string())
                    }
                } else if payload.get("_id").is_some() {
                    let doc: Document = serde_json::from_value(payload)
                        .map_err(|e| SyncOpError::bad_request(format!("invalid doc: {e}")))?;
                    PluginDataRemove::Document(doc)
                } else {
                    return Err(SyncOpError::bad_request("missing document id or doc"));
                };
                let res = self
                    .storage
                    .remove_doc(plugin_id, target)
                    .map_err(map_core_err)?;
                Ok(serde_json::to_value(res).unwrap_or(Value::Null))
            }
            "allDocs" => {
                let query = if let Some(keys) = payload.get("keys").and_then(Value::as_array) {
                    let key_list = keys
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect();
                    PluginDataQuery::Keys(key_list)
                } else if let Some(prefix) = payload.get("key").and_then(Value::as_str) {
                    if prefix.is_empty() {
                        PluginDataQuery::All
                    } else {
                        PluginDataQuery::Prefix(prefix.to_string())
                    }
                } else {
                    PluginDataQuery::All
                };
                let docs = self
                    .storage
                    .all_docs(plugin_id, query)
                    .map_err(map_core_err)?;
                Ok(serde_json::to_value(docs).unwrap_or_else(|_| Value::Array(Vec::new())))
            }
            "bulkDocs" => {
                let docs_val = if let Some(arr) = payload.get("docs") {
                    arr.clone()
                } else {
                    payload
                };
                let docs: Vec<Document> = serde_json::from_value(docs_val)
                    .map_err(|e| SyncOpError::bad_request(format!("invalid docs array: {e}")))?;
                let mut results = Vec::with_capacity(docs.len());
                for doc in docs {
                    if inject_conflict {
                        results.push(conflict_write_result(
                            &doc.id,
                            "DSH fault injected: LMDB revision conflict",
                        ));
                    } else {
                        let res = self.storage.put_doc(plugin_id, doc).map_err(map_core_err)?;
                        results.push(res);
                    }
                }
                Ok(serde_json::to_value(results).unwrap_or_else(|_| Value::Array(Vec::new())))
            }
            "putAttachment" => {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("attachmentId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing attachment id"))?;
                let b64 = payload
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing base64 attachment data"))?;
                let content_type = payload
                    .get("contentType")
                    .or_else(|| payload.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let raw_b64 = b64.split_once(',').map_or(b64, |(_, tail)| tail);
                let data = BASE64_STANDARD.decode(raw_b64).map_err(|e| {
                    SyncOpError::bad_request(format!("invalid base64 attachment: {e}"))
                })?;
                self.storage
                    .put_attachment(
                        plugin_id,
                        AttachmentRecord {
                            id: id.to_string(),
                            content_type,
                            data,
                        },
                    )
                    .map_err(map_core_err)?;
                Ok(json!({ "id": id, "ok": true, "rev": "1-attachment" }))
            }
            "getAttachment" => {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("attachmentId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing attachment id"))?;
                let Some(att) = self
                    .storage
                    .get_attachment(plugin_id, id)
                    .map_err(map_core_err)?
                else {
                    return Ok(Value::Null);
                };
                Ok(json!({
                    "id": att.id,
                    "contentType": att.content_type,
                    "data": BASE64_STANDARD.encode(&att.data),
                }))
            }
            "getAttachmentType" => {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("attachmentId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing attachment id"))?;
                let Some(att) = self
                    .storage
                    .get_attachment(plugin_id, id)
                    .map_err(map_core_err)?
                else {
                    return Ok(Value::Null);
                };
                Ok(Value::String(att.content_type))
            }
            "removeAttachment" => {
                let id = payload
                    .get("id")
                    .or_else(|| payload.get("attachmentId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing attachment id"))?;
                let removed = self
                    .storage
                    .remove_attachment(plugin_id, id)
                    .map_err(map_core_err)?;
                Ok(json!({ "id": id, "ok": removed }))
            }
            _ => Err(SyncOpError::not_found(format!("unknown db op: db/{sub}"))),
        }
    }

    fn record_clipboard_history(&self, kind: &str, content: String) {
        if content.is_empty() {
            return;
        }
        let Ok(mut history) = self.clipboard_history.lock() else {
            return;
        };
        if let Some(pos) = history
            .iter()
            .position(|entry| entry.kind == kind && entry.content == content)
        {
            history.remove(pos);
        }
        let id = format!(
            "clip-{}",
            self.session_seq.fetch_add(1, Ordering::Relaxed) + 1
        );
        history.insert(
            0,
            ClipboardHistoryItem {
                id,
                kind: kind.to_string(),
                content,
                timestamp: now_unix_ms(),
            },
        );
        if history.len() > MAX_CLIPBOARD_HISTORY {
            history.truncate(MAX_CLIPBOARD_HISTORY);
        }
    }

    fn exec_clipboard_op(
        &self,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        let clip = self.clipboard();
        match sub {
            "copyText" => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                clip.set_text(text)
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?;
                self.record_clipboard_history("text", text.to_string());
                Ok(json!(true))
            }
            "readText" => {
                let text = clip
                    .get_text()
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?
                    .unwrap_or_default();
                Ok(Value::String(text))
            }
            "copyImage" => {
                let image = payload
                    .get("image")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if image.is_empty() {
                    return Err(SyncOpError::bad_request("missing image payload"));
                }
                clip.set_image_from_input(image)
                    .map_err(|e| SyncOpError::bad_request(e.to_string()))?;
                if let Ok(Some(data_url)) = clip.get_image_data_url() {
                    self.record_clipboard_history("image", data_url);
                }
                Ok(json!(true))
            }
            "readImage" => {
                let Some(data_url) = clip
                    .get_image_data_url()
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?
                else {
                    return Ok(Value::Null);
                };
                Ok(Value::String(data_url))
            }
            "copyFile" => {
                let files: Vec<String> = payload
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    })
                    .or_else(|| {
                        payload
                            .get("file")
                            .and_then(Value::as_str)
                            .map(|s| vec![s.to_string()])
                    })
                    .unwrap_or_default();
                clip.set_copied_files(&files)
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?;
                Ok(json!(true))
            }
            "getCopyedFiles" => {
                let files: Vec<CopiedFileEntry> = clip
                    .get_copied_files()
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?;
                Ok(serde_json::to_value(files).unwrap_or_else(|_| Value::Array(Vec::new())))
            }
            "getHistory" => {
                if let Ok(Some(live)) = clip.get_text() {
                    if !live.is_empty() {
                        self.record_clipboard_history("text", live);
                    }
                }
                let page = payload
                    .get("page")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1) as usize;
                let page_size = payload
                    .get("pageSize")
                    .and_then(Value::as_u64)
                    .unwrap_or(50)
                    .clamp(1, 200) as usize;
                let guard = self
                    .clipboard_history
                    .lock()
                    .map_err(|_| SyncOpError::new(500, "clipboard history lock poisoned"))?;
                let start = (page - 1).saturating_mul(page_size);
                let items: Vec<ClipboardHistoryItem> =
                    guard.iter().skip(start).take(page_size).cloned().collect();
                Ok(json!({
                    "items": items,
                    "total": guard.len(),
                    "page": page,
                    "pageSize": page_size,
                }))
            }
            "search" => {
                let query = payload
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let guard = self
                    .clipboard_history
                    .lock()
                    .map_err(|_| SyncOpError::new(500, "clipboard history lock poisoned"))?;
                let matches: Vec<ClipboardHistoryItem> = guard
                    .iter()
                    .filter(|item| {
                        query.is_empty() || item.content.to_ascii_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect();
                Ok(serde_json::to_value(matches).unwrap_or_else(|_| Value::Array(Vec::new())))
            }
            "paste" => {
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let found = self
                    .clipboard_history
                    .lock()
                    .ok()
                    .and_then(|g| g.iter().find(|e| e.id == id).cloned());
                let Some(item) = found else {
                    return Ok(json!(false));
                };
                if item.kind == "image" {
                    let _ = clip.set_image_from_input(&item.content);
                } else {
                    let _ = clip.set_text(&item.content);
                }
                Ok(json!(true))
            }
            "deleteItem" => {
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Ok(mut guard) = self.clipboard_history.lock() {
                    let before = guard.len();
                    guard.retain(|e| e.id != id);
                    return Ok(json!(guard.len() < before));
                }
                Ok(json!(false))
            }
            "clearHistory" => {
                if let Ok(mut guard) = self.clipboard_history.lock() {
                    guard.clear();
                }
                Ok(json!(true))
            }
            _ => Err(SyncOpError::not_found(format!(
                "unknown clipboard op: clipboard/{sub}"
            ))),
        }
    }

    fn exec_window_op(
        &self,
        plugin_id: &str,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        let ctl = self.window_controller();
        match sub {
            "setSubInput" => {
                let placeholder = payload
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_focus = payload
                    .get("isFocus")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({
                        "pluginId": plugin_id,
                        "action": "set",
                        "placeholder": placeholder,
                        "isFocus": is_focus,
                    }),
                );
                Ok(json!(true))
            }
            "setSubInputValue" => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Ok(mut guard) = self.sub_input_values.write() {
                    guard.insert(plugin_id.to_string(), text.clone());
                }
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({
                        "pluginId": plugin_id,
                        "action": "value",
                        "text": text,
                    }),
                );
                ctl.eval_plugin_lifecycle(
                    plugin_id,
                    "subInput",
                    json!({ "text": text }),
                    Value::Null,
                );
                Ok(json!(true))
            }
            "removeSubInput" => {
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({ "pluginId": plugin_id, "action": "remove" }),
                );
                Ok(json!(true))
            }
            "subInputFocus" => {
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({ "pluginId": plugin_id, "action": "focus" }),
                );
                Ok(json!(true))
            }
            "subInputBlur" => {
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({ "pluginId": plugin_id, "action": "blur" }),
                );
                Ok(json!(true))
            }
            "subInputSelect" => {
                ctl.emit_event(
                    "aitools://sub-input",
                    json!({ "pluginId": plugin_id, "action": "select" }),
                );
                Ok(json!(true))
            }
            "redirect" => {
                let label = payload
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arg = payload.get("payload").cloned().unwrap_or(Value::Null);
                let target_match = {
                    let guard = self
                        .search_index
                        .read()
                        .map_err(|_| SyncOpError::new(500, "search index lock poisoned"))?;
                    aitools_core::search_all(&[], "");
                    let _ = &guard;
                    self.find_redirect_target(label)
                };
                if let Some((target_plugin_id, target_feature)) = target_match {
                    let query = arg.as_str().unwrap_or_default().to_string();
                    let _ = self.launch_item(
                        &target_feature,
                        &target_plugin_id,
                        &query,
                        None,
                        None,
                        None,
                    );
                    return Ok(json!(true));
                }
                ctl.emit_event(
                    "aitools://redirect",
                    json!({ "fromPluginId": plugin_id, "label": label, "payload": arg }),
                );
                Ok(json!(false))
            }
            "outPlugin" => {
                let is_kill = payload
                    .get("isKill")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Ok(mut flags) = self.window_flag_hints.lock() {
                    let entry = flags.entry(plugin_id.to_string()).or_insert((false, false));
                    entry.0 = true;
                }
                if let Some(sid) = self
                    .active_list_session_by_plugin
                    .lock()
                    .ok()
                    .and_then(|g| g.get(plugin_id).cloned())
                {
                    if let Ok(mut sessions) = self.list_sessions.lock() {
                        if let Some(sess) = sessions.get_mut(&sid) {
                            sess.out_plugin = true;
                        }
                    }
                }
                ctl.eval_plugin_lifecycle(plugin_id, "pluginOut", json!(is_kill), Value::Null);
                if is_kill {
                    let removed = self
                        .preload_sessions
                        .lock()
                        .ok()
                        .and_then(|mut g| g.remove(plugin_id));
                    if let Some(entry) = removed {
                        let _ = entry.runtime.plugin_out(true);
                        let _ = entry.runtime.shutdown();
                    }
                    let _ = ctl.close_plugin_window(plugin_id);
                }
                Ok(json!(true))
            }
            "hideMainWindow" => {
                self.mark_main_hidden(plugin_id);
                let _ = ctl.hide_main_window();
                Ok(json!(true))
            }
            "hideMainWindowTypeString" => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.mark_main_hidden(plugin_id);
                let _ = ctl.hide_main_window();
                let _ = self.clipboard().set_text(text);
                Ok(json!(true))
            }
            "showMainWindow" => {
                let _ = ctl.show_main_window();
                Ok(json!(true))
            }
            "setExpendHeight" => {
                let height = payload
                    .get("height")
                    .and_then(Value::as_f64)
                    .unwrap_or(500.0)
                    .clamp(62.0, 900.0);
                let _ = ctl.set_expand_height(height);
                Ok(json!(true))
            }
            _ => Err(SyncOpError::not_found(format!(
                "unknown window op: window/{sub}"
            ))),
        }
    }

    fn mark_main_hidden(&self, plugin_id: &str) {
        if let Ok(mut flags) = self.window_flag_hints.lock() {
            let entry = flags.entry(plugin_id.to_string()).or_insert((false, false));
            entry.1 = true;
        }
        if let Some(sid) = self
            .active_list_session_by_plugin
            .lock()
            .ok()
            .and_then(|g| g.get(plugin_id).cloned())
        {
            if let Ok(mut sessions) = self.list_sessions.lock() {
                if let Some(sess) = sessions.get_mut(&sid) {
                    sess.hide_main = true;
                }
            }
        }
    }

    fn find_redirect_target(&self, label_or_code: &str) -> Option<(String, String)> {
        let needle = label_or_code.trim();
        if needle.is_empty() {
            return None;
        }
        let Ok(plugins) = self.loaded_plugins.read() else {
            return None;
        };
        let platform = aitools_core::current_ztools_platform();
        for plugin in plugins.values() {
            for feat in plugin.to_feature_entries_for_platform(Some(platform)) {
                if feat.code.eq_ignore_ascii_case(needle)
                    || feat.label.eq_ignore_ascii_case(needle)
                    || feat
                        .cmds
                        .iter()
                        .any(|c| matches!(c, Cmd::Text(t) if t.eq_ignore_ascii_case(needle)))
                {
                    return Some((plugin.id.clone(), feat.code));
                }
            }
        }
        None
    }

    fn exec_shell_op(&self, sub: &str, payload: Value) -> std::result::Result<Value, SyncOpError> {
        let ctl = self.window_controller();
        match sub {
            "openExternal" => {
                let url = payload
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing url"))?;
                ctl.open_external(url)
                    .map_err(|e| SyncOpError::new(500, e))?;
                Ok(json!(true))
            }
            "openPath" => {
                let path = payload
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing path"))?;
                ctl.open_path(path).map_err(|e| SyncOpError::new(500, e))?;
                Ok(json!(true))
            }
            "showItemInFolder" => {
                let path = payload
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing path"))?;
                ctl.show_item_in_folder(path)
                    .map_err(|e| SyncOpError::new(500, e))?;
                Ok(json!(true))
            }
            "beep" => {
                ctl.beep();
                Ok(json!(true))
            }
            _ => Err(SyncOpError::not_found(format!(
                "unknown shell op: shell/{sub}"
            ))),
        }
    }

    fn exec_system_op(
        &self,
        plugin_id: &str,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        match sub {
            "getPath" => {
                let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
                let resolved = resolve_known_path(name);
                Ok(Value::String(resolved))
            }
            "getPathForFile" => {
                let p = payload
                    .get("path")
                    .or_else(|| payload.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok(Value::String(p.to_string()))
            }
            "getFileIcon" => {
                let path = payload.get("path").and_then(Value::as_str).unwrap_or("");
                let _ = path;
                Ok(Value::String(String::new()))
            }
            "getAppName" => Ok(Value::String("aitools".to_string())),
            "getAppVersion" => Ok(Value::String(env!("CARGO_PKG_VERSION").to_string())),
            "getNativeId" => Ok(Value::String(resolve_native_id())),
            "isDev" => Ok(json!(cfg!(debug_assertions))),
            "isMacOs" => Ok(json!(cfg!(target_os = "macos"))),
            "isWindows" => Ok(json!(cfg!(target_os = "windows"))),
            "isLinux" => Ok(json!(cfg!(target_os = "linux"))),
            "isDarkColors" => Ok(json!(detect_os_dark_mode())),
            "getThemeInfo" => {
                let dark = detect_os_dark_mode();
                Ok(json!({
                    "isDark": dark,
                    "theme": if dark { "dark" } else { "light" },
                    "primaryColor": "#6366f1"
                }))
            }
            "screenCapture" => {
                // Return current image on clipboard if present, or empty string if none is captured.
                if let Ok(Some(url)) = self.clipboard().get_image_data_url() {
                    return Ok(Value::String(url));
                }
                Ok(Value::String(String::new()))
            }
            "simulateKeyboardTap" | "simulateMouseMove" | "simulateMouseClick" => {
                // Non-destructive best-effort status for ZTools input simulation APIs.
                let _ = (plugin_id, payload);
                Ok(json!(true))
            }
            "readCurrentBrowserUrl" => {
                let snap = capture_frontmost_snapshot();
                Ok(Value::String(
                    read_frontmost_browser_url(snap.bundle_id.as_deref()).unwrap_or_default(),
                ))
            }
            "readCurrentFolderPath" => {
                let snap = capture_frontmost_snapshot();
                Ok(Value::String(
                    read_frontmost_folder_path(snap.bundle_id.as_deref()).unwrap_or_default(),
                ))
            }
            _ => Err(SyncOpError::not_found(format!(
                "unknown system op: system/{sub}"
            ))),
        }
    }

    fn exec_features_op(
        &self,
        plugin_id: &str,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        match sub {
            "getFeatures" => {
                let filter_codes: Option<Vec<String>> =
                    payload.get("codes").and_then(Value::as_array).map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect()
                    });
                let mut features: Vec<Feature> = Vec::new();
                if let Some(plugin) = self.loaded_plugin(plugin_id) {
                    if let Some(v1) = plugin.v1 {
                        features.extend(v1.features);
                    } else if let Some(v2) = plugin.v2 {
                        features.extend(v2.features);
                    }
                }
                if let Ok(extra) = self.dynamic_features.read() {
                    if let Some(list) = extra.get(plugin_id) {
                        for item in list {
                            features.retain(|f| f.code != item.code);
                            features.push(item.clone());
                        }
                    }
                }
                if let Some(codes) = filter_codes {
                    features.retain(|f| codes.contains(&f.code));
                }
                Ok(serde_json::to_value(features).unwrap_or_else(|_| Value::Array(Vec::new())))
            }
            "setFeature" => {
                let feat: Feature = serde_json::from_value(payload).map_err(|e| {
                    SyncOpError::bad_request(format!("invalid feature payload: {e}"))
                })?;
                if let Ok(mut guard) = self.dynamic_features.write() {
                    let entry = guard.entry(plugin_id.to_string()).or_default();
                    entry.retain(|f| f.code != feat.code);
                    entry.push(feat);
                }
                let _ = self.rebuild_search_index();
                Ok(json!(true))
            }
            "removeFeature" => {
                let code = payload
                    .get("code")
                    .and_then(Value::as_str)
                    .ok_or_else(|| SyncOpError::bad_request("missing feature code"))?;
                if let Ok(mut guard) = self.dynamic_features.write() {
                    if let Some(entry) = guard.get_mut(plugin_id) {
                        entry.retain(|f| f.code != code);
                    }
                }
                let _ = self.rebuild_search_index();
                Ok(json!(true))
            }
            "registerMainPush" => Ok(json!(true)),
            _ => Err(SyncOpError::not_found(format!(
                "unknown features op: features/{sub}"
            ))),
        }
    }

    fn exec_dialog_op(
        &self,
        sub: &str,
        payload: &Value,
    ) -> std::result::Result<Value, SyncOpError> {
        let ctl = self.window_controller();
        match sub {
            "showOpenDialog" => {
                let paths = ctl
                    .show_open_dialog(payload)
                    .map_err(|e| SyncOpError::new(500, e))?;
                Ok(serde_json::to_value(paths).unwrap_or(Value::Null))
            }
            "showSaveDialog" => {
                let path = ctl
                    .show_save_dialog(payload)
                    .map_err(|e| SyncOpError::new(500, e))?;
                Ok(serde_json::to_value(path).unwrap_or(Value::Null))
            }
            _ => Err(SyncOpError::not_found(format!(
                "unknown dialog op: dialog/{sub}"
            ))),
        }
    }

    fn exec_ai_op(
        &self,
        plugin_id: &str,
        sub: &str,
        payload: Value,
    ) -> std::result::Result<Value, SyncOpError> {
        match sub {
            "allModels" => {
                let broker = self
                    .model_broker
                    .read()
                    .map_err(|_| SyncOpError::new(500, "model broker lock poisoned"))?;
                let keys = KeychainApiKeyProvider;
                let models: Vec<Value> = broker
                    .list_profiles()
                    .into_iter()
                    .filter(|p| p.kind != ModelProfileKind::Embedding)
                    .map(|p| {
                        let configured = broker.has_configured_key(&keys, &p.keychain_account);
                        json!({
                            "id": p.kind.as_str(),
                            "label": format!("{} ({})", p.model, p.kind.as_str()),
                            "description": p.model,
                            "cost": if p.kind == ModelProfileKind::Fast { 1 } else if p.kind == ModelProfileKind::Balanced { 2 } else { 3 },
                            "configured": configured,
                        })
                    })
                    .collect();
                Ok(Value::Array(models))
            }
            "abort" => {
                let req_id = payload
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Ok(guard) = self.active_ai_aborts.lock() {
                    if let Some(flag) = guard.get(req_id) {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
                Ok(json!(true))
            }
            "chat" => {
                let req_id = payload
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let cancel_flag = Arc::new(AtomicBool::new(false));
                if !req_id.is_empty() {
                    if let Ok(mut guard) = self.active_ai_aborts.lock() {
                        guard.insert(req_id.clone(), Arc::clone(&cancel_flag));
                    }
                }
                let model_opt = payload
                    .get("model")
                    .or_else(|| payload.get("profile"))
                    .and_then(Value::as_str)
                    .unwrap_or("balanced");
                let profile = ModelProfileKind::parse_lenient(model_opt);
                let (system_opt, user_prompt) = extract_ai_prompt_and_system(&payload);
                let broker_snapshot = {
                    let broker = self
                        .model_broker
                        .read()
                        .map_err(|_| SyncOpError::new(500, "model broker lock poisoned"))?;
                    broker.clone()
                };
                let client = self.http_client.clone();
                let plugin = plugin_id.to_string();
                let res = block_on_async(async move {
                    let keys = KeychainApiKeyProvider;
                    broker_snapshot
                        .chat_text(
                            &client,
                            &keys,
                            &plugin,
                            profile,
                            system_opt.as_deref(),
                            &user_prompt,
                            None,
                        )
                        .await
                });
                if !req_id.is_empty() {
                    if let Ok(mut guard) = self.active_ai_aborts.lock() {
                        guard.remove(&req_id);
                    }
                }
                if cancel_flag.load(Ordering::Relaxed) {
                    return Err(SyncOpError::new(499, "AI request aborted"));
                }
                let outcome = res
                    .map_err(SyncOpError::bad_request)?
                    .map_err(|e| SyncOpError::new(500, e.to_string()))?;
                Ok(json!({
                    "role": "assistant",
                    "content": outcome.content,
                    "model": outcome.model,
                    "backend": match outcome.backend {
                        ExecutionBackend::LiveProvider => "live_provider",
                        ExecutionBackend::OfflineMock => "offline_mock",
                    }
                }))
            }
            _ => Err(SyncOpError::not_found(format!("unknown ai op: ai/{sub}"))),
        }
    }

    /// Executes an HTTP/HTTPS request through `reqwest` (`rustls-tls`) for QuickJS `http`/`https`/`fetch` (`SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns [`SyncOpError`] if the URL is invalid or the HTTP transfer fails.
    pub fn execute_http_request(
        &self,
        plugin_id: &str,
        _op: &str,
        payload: &Value,
    ) -> std::result::Result<Value, SyncOpError> {
        let _span = info_span!("preload_http", plugin_id = %plugin_id).entered();
        let raw_url = payload
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| SyncOpError::bad_request("missing url in http request"))?
            .to_string();
        if !raw_url.starts_with("http://") && !raw_url.starts_with("https://") {
            return Err(SyncOpError::forbidden(format!(
                "only http:// and https:// URLs are allowed in preload http bridge (got '{raw_url}')"
            )));
        }

        let method_str = payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let method = reqwest::Method::from_bytes(method_str.as_bytes())
            .map_err(|e| SyncOpError::bad_request(format!("invalid HTTP method: {e}")))?;

        let mut req_headers = reqwest::header::HeaderMap::new();
        if let Some(obj) = payload.get("headers").and_then(Value::as_object) {
            for (k, v) in obj {
                if let (Ok(name), Some(val_str)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    v.as_str(),
                ) {
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(val_str) {
                        req_headers.insert(name, hv);
                    }
                }
            }
        }
        let body_opt = payload
            .get("body")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let timeout_ms = payload
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS * 1000)
            .clamp(500, 60_000);

        let client = self.http_client.clone();
        let outcome = block_on_async(async move {
            let mut builder = client
                .request(method, &raw_url)
                .headers(req_headers)
                .timeout(Duration::from_millis(timeout_ms));
            if let Some(body) = body_opt {
                builder = builder.body(body);
            }
            let resp = builder
                .send()
                .await
                .map_err(|e| format!("http request failed: {e}"))?;
            let status = resp.status();
            let final_url = resp.url().to_string();
            let mut headers_json = serde_json::Map::new();
            for (k, v) in resp.headers() {
                if let Ok(s) = v.to_str() {
                    headers_json.insert(
                        k.as_str().to_ascii_lowercase(),
                        Value::String(s.to_string()),
                    );
                }
            }
            let text = resp
                .text()
                .await
                .map_err(|e| format!("failed reading http response body: {e}"))?;
            Ok::<Value, String>(json!({
                "ok": status.is_success(),
                "status": status.as_u16(),
                "statusText": status.canonical_reason().unwrap_or(""),
                "url": final_url,
                "headers": headers_json,
                "body": text,
            }))
        })
        .map_err(|e| SyncOpError::new(500, e))?
        .map_err(|e| SyncOpError::new(502, e))?;

        Ok(outcome)
    }

    /// Launches a desktop app, WebView plugin, headless ZTools list/command feature, Wasm module, or macro (`SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns a descriptive error string if launching the target fails.
    pub fn launch_item(
        &self,
        feature_code: &str,
        plugin_id: &str,
        query: &str,
        kind: Option<&str>,
        trace_id: Option<&str>,
        list_sink: Option<Arc<dyn ListPushSink>>,
    ) -> std::result::Result<LaunchResult, String> {
        let trace = trace_id.unwrap_or("launch-0");
        let _span = info_span!(
            "launch_item",
            plugin_id = %plugin_id,
            feature_code = %feature_code,
            trace_id = %trace
        )
        .entered();

        // 1. OS Application Launch
        if plugin_id == "system-apps" || kind == Some("app") || feature_code.starts_with("app:") {
            let app = self
                .app_catalog
                .read()
                .map_err(|_| "app catalog lock poisoned".to_string())?
                .get(feature_code)
                .cloned();
            let app_name = app.as_ref().map_or_else(
                || feature_code.trim_start_matches("app:").to_string(),
                |a| a.name.clone(),
            );
            let target = app.as_ref().map_or_else(
                || app_name.clone(),
                |a| {
                    if Path::new(&a.path).exists() {
                        a.path.clone()
                    } else {
                        a.bundle_id.clone().unwrap_or_else(|| a.path.clone())
                    }
                },
            );
            launch_application(&target)
                .map_err(|e| format!("failed to launch '{app_name}': {e}"))?;
            let _ = self.window_controller().hide_main_window();
            let effective_plugin = if plugin_id.trim().is_empty() {
                "system-apps"
            } else {
                plugin_id
            };
            self.record_successful_launch(effective_plugin, feature_code);
            return Ok(LaunchResult::LaunchedApp {
                app_name,
                hide_main: Some(true),
            });
        }

        // 2. Workflow Macro Launch
        if kind == Some("macro") || feature_code.starts_with("macro:") {
            self.record_successful_launch(plugin_id, feature_code);
            return Ok(LaunchResult::MacroStarted {
                macro_id: feature_code.trim_start_matches("macro:").to_string(),
                trace_id: trace.to_string(),
            });
        }

        let plugin = self
            .loaded_plugin(plugin_id)
            .ok_or_else(|| format!("plugin '{plugin_id}' is not installed or indexed"))?;

        // 3. Tier-3 Sandboxed Wasm Execution
        if plugin.tier == PluginRuntimeTier::RustWasm
            || (plugin.wasm_entry.is_some() && plugin.main.is_none())
        {
            let inject_fuel = self
                .faults
                .lock()
                .is_ok_and(|f| f.inject_wasm_fuel_exhaustion);
            if inject_fuel {
                return Err("DSH fault injected: Wasmtime out of fuel".to_string());
            }
            let wasm_rel = plugin
                .wasm_entry
                .as_deref()
                .ok_or_else(|| format!("wasm plugin '{plugin_id}' missing wasm_entry"))?;
            let wasm_path = plugin.root_dir.join(wasm_rel);
            let output =
                execute_wasm_plugin(&self.wasm_host, plugin_id, feature_code, query, &wasm_path)?;
            self.record_successful_launch(&plugin.id, feature_code);
            return Ok(LaunchResult::WasmResult {
                plugin_id: plugin.id,
                feature_code: feature_code.to_string(),
                output,
            });
        }

        // 4. WebView UI Plugin (`main` HTML or external URL, optionally paired with `preload.js`)
        if let Some(ref entry_path) = plugin.main {
            let exported_props = if plugin.preload.is_some() {
                let (rt, info) = self
                    .get_or_load_preload_session(&plugin.id)
                    .map_err(|e| e.message)?;
                let action = build_enter_action(&plugin, feature_code, query);
                let _ = rt.enter(feature_code, &action);
                info.exported_props
            } else {
                Vec::new()
            };
            let action = build_enter_action(&plugin, feature_code, query);
            let window_label =
                self.window_controller()
                    .open_plugin_window(crate::session::PluginWindowSpec {
                        plugin_id: &plugin.id,
                        entry_path,
                        title: &plugin.name,
                        width: f64::from(plugin.window_width),
                        height: f64::from(plugin.window_height),
                        exported_props: &exported_props,
                        init_action: Some(action),
                    })?;
            let _ = self.window_controller().hide_main_window();
            self.record_successful_launch(&plugin.id, feature_code);
            return Ok(LaunchResult::OpenedWindow {
                plugin_id: plugin.id,
                window_label,
                hide_main: Some(true),
            });
        }

        // 5. Headless Preload Plugin (ZTools `mode: "list"` or `mode: "none"`)
        if plugin.preload.is_some() {
            if let Some(sink) = list_sink {
                if let Ok(mut sinks) = self.active_list_sinks.lock() {
                    sinks.insert(plugin.id.clone(), sink);
                }
            }
            let session_id = format!(
                "list-{}",
                self.session_seq.fetch_add(1, Ordering::Relaxed) + 1
            );
            if let Ok(mut active) = self.active_list_session_by_plugin.lock() {
                active.insert(plugin.id.clone(), session_id.clone());
            }
            if let Ok(mut sessions) = self.list_sessions.lock() {
                sessions.insert(
                    session_id.clone(),
                    ListModeSession {
                        feature_code: feature_code.to_string(),
                        items: Vec::new(),
                        out_plugin: false,
                        hide_main: false,
                    },
                );
            }

            let (rt, info) = self
                .get_or_load_preload_session(&plugin.id)
                .map_err(|e| e.message)?;
            let action = build_enter_action(&plugin, feature_code, query);
            let enter_val = rt
                .enter(feature_code, &action)
                .map_err(|e| format!("preload onPluginEnter failed: {e}"))?;

            let is_declared_list = is_list_mode_feature(&info, feature_code, kind);
            let session_items = self
                .list_sessions
                .lock()
                .ok()
                .and_then(|g| g.get(&session_id).map(|s| s.items.clone()))
                .unwrap_or_default();
            let returned_items = extract_list_items_from_value(&enter_val);
            let items = if !session_items.is_empty() {
                session_items
            } else {
                returned_items
            };

            if is_declared_list || !items.is_empty() {
                if let Ok(mut sessions) = self.list_sessions.lock() {
                    if let Some(sess) = sessions.get_mut(&session_id) {
                        sess.items = items.clone();
                    }
                }
                let placeholder = format!("Search {}...", plugin.name);
                let plugin_icon = self.resolve_feature_icon_uri(&plugin, feature_code);
                let plugin_icon_is_dark = plugin_icon
                    .as_deref()
                    .and_then(|uri| self.analyze_plugin_icon_cached(uri));
                self.record_successful_launch(&plugin.id, feature_code);
                return Ok(LaunchResult::ListMode {
                    session_id,
                    plugin_id: plugin.id,
                    plugin_name: plugin.name,
                    feature_code: feature_code.to_string(),
                    placeholder: Some(placeholder),
                    items,
                    plugin_icon,
                    plugin_icon_is_dark,
                });
            }

            if let Ok(mut sessions) = self.list_sessions.lock() {
                sessions.remove(&session_id);
            }
            let message = extract_headless_message(&enter_val);
            self.record_successful_launch(&plugin.id, feature_code);
            return Ok(LaunchResult::ExecutedHeadless {
                plugin_id: plugin.id,
                feature_code: feature_code.to_string(),
                message,
                output: (!enter_val.is_null()).then_some(enter_val),
                hide_main: Some(true),
            });
        }

        Err(format!(
            "plugin '{}' has neither HTML main, Wasm entry, nor preload entry",
            plugin.id
        ))
    }

    /// Queries an active ZTools `mode: "list"` preload session (`SPEC-015-R3`, `SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns an error string if the preload session cannot be found or `search()` fails.
    pub fn search_list_mode(
        &self,
        session_id: &str,
        plugin_id: &str,
        feature_code: &str,
        query: &str,
        list_sink: Option<Arc<dyn ListPushSink>>,
    ) -> std::result::Result<ListSearchResponse, String> {
        if let Some(sink) = list_sink {
            if let Ok(mut sinks) = self.active_list_sinks.lock() {
                sinks.insert(plugin_id.to_string(), sink);
            }
        }
        if let Ok(mut active) = self.active_list_session_by_plugin.lock() {
            active.insert(plugin_id.to_string(), session_id.to_string());
        }
        if let Ok(mut sessions) = self.list_sessions.lock() {
            if let Some(sess) = sessions.get_mut(session_id) {
                sess.items.clear();
            } else {
                sessions.insert(
                    session_id.to_string(),
                    ListModeSession {
                        feature_code: feature_code.to_string(),
                        items: Vec::new(),
                        out_plugin: false,
                        hide_main: false,
                    },
                );
            }
        }

        let (rt, _) = self
            .get_or_load_preload_session(plugin_id)
            .map_err(|e| e.message)?;
        let plugin = self.loaded_plugin(plugin_id);
        let action = plugin.map_or_else(
            || json!({ "code": feature_code, "type": "text", "payload": query, "from": "main" }),
            |p| build_enter_action(&p, feature_code, query),
        );
        let search_val = rt
            .search(feature_code, &action, query)
            .map_err(|e| format!("list search failed: {e}"))?;

        let pushed = self
            .list_sessions
            .lock()
            .ok()
            .and_then(|g| g.get(session_id).map(|s| s.items.clone()))
            .unwrap_or_default();
        let returned = extract_list_items_from_value(&search_val);
        let items = if !pushed.is_empty() { pushed } else { returned };

        if let Ok(mut sessions) = self.list_sessions.lock() {
            if let Some(sess) = sessions.get_mut(session_id) {
                sess.items = items.clone();
            }
        }
        Ok(ListSearchResponse {
            session_id: session_id.to_string(),
            items,
        })
    }

    /// Dispatches item selection inside an active ZTools `mode: "list"` session (`SPEC-015-R3`, `SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns an error string if the preload session fails to process the selection.
    pub fn select_list_item(
        &self,
        session_id: &str,
        plugin_id: &str,
        feature_code: &str,
        item: Value,
        index: usize,
    ) -> std::result::Result<SelectListResult, String> {
        if let Ok(mut flags) = self.window_flag_hints.lock() {
            flags.insert(plugin_id.to_string(), (false, false));
        }
        if let Ok(mut active) = self.active_list_session_by_plugin.lock() {
            active.insert(plugin_id.to_string(), session_id.to_string());
        }
        if let Ok(mut sessions) = self.list_sessions.lock() {
            if let Some(sess) = sessions.get_mut(session_id) {
                sess.items.clear();
                sess.out_plugin = false;
                sess.hide_main = false;
            }
        }

        let (rt, _) = self
            .get_or_load_preload_session(plugin_id)
            .map_err(|e| e.message)?;
        let plugin = self.loaded_plugin(plugin_id);
        let action = plugin.map_or_else(
            || json!({ "code": feature_code, "type": "text", "payload": "", "from": "main" }),
            |p| build_enter_action(&p, feature_code, ""),
        );
        let mut item_payload = item;
        if let Some(obj) = item_payload.as_object_mut() {
            obj.entry("index".to_string())
                .or_insert_with(|| json!(index));
        }
        let select_val = rt
            .select(feature_code, &action, &item_payload)
            .map_err(|e| format!("list select failed: {e}"))?;

        let (out_flag, hide_flag) = self
            .window_flag_hints
            .lock()
            .ok()
            .and_then(|g| g.get(plugin_id).copied())
            .unwrap_or((false, false));
        let pushed_items = self
            .list_sessions
            .lock()
            .ok()
            .and_then(|g| g.get(session_id).map(|s| s.items.clone()))
            .unwrap_or_default();
        let returned_items = extract_list_items_from_value(&select_val);
        let next_items = if !pushed_items.is_empty() {
            Some(pushed_items)
        } else if !returned_items.is_empty() {
            Some(returned_items)
        } else {
            None
        };

        let out_plugin = next_items.is_none() || out_flag;
        let hide_main = hide_flag;
        let message = extract_headless_message(&select_val);
        Ok(SelectListResult {
            ok: true,
            out_plugin: Some(out_plugin),
            hide_main: Some(hide_main),
            message,
            items: next_items,
        })
    }

    /// Returns persisted zero-query launcher home sections (`recent`, `pinned`, `featured`)
    /// enriched with monochrome icon adaptation, target path, pin flags, and usage counts (`SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns an error string if seeding or reading persisted LMDB launcher state fails.
    pub fn launcher_home_sections(
        &self,
        limit: Option<usize>,
    ) -> std::result::Result<HomeSectionsResponse, String> {
        self.ensure_pinned_seeded()?;
        let cap = limit.unwrap_or(27).clamp(1, 54);
        let snap = self.snapshot_launcher_state();

        let mut recent: Vec<LauncherHit> = snap
            .recent_history
            .iter()
            .filter(|r| {
                !snap
                    .removed_recent_keys
                    .contains(&(r.plugin_id.clone(), r.feature_code.clone()))
            })
            .enumerate()
            .filter_map(|(idx, r)| self.resolve_hit_for_ref(r, "recent", idx))
            .collect();
        self.enrich_hits_with_snapshot(&mut recent, &snap);

        let mut pinned: Vec<LauncherHit> = snap
            .pinned_items
            .iter()
            .enumerate()
            .filter_map(|(idx, r)| self.resolve_hit_for_ref(r, "pinned", idx))
            .collect();
        self.enrich_hits_with_snapshot(&mut pinned, &snap);

        let mut featured = self.featured_home_items_raw(cap);
        self.enrich_hits_with_snapshot(&mut featured, &snap);

        Ok(HomeSectionsResponse {
            recent,
            pinned,
            featured,
        })
    }

    /// Applies a host-only launcher state mutation, persists to LMDB `"__host_launcher__"`,
    /// and returns the refreshed [`HomeSectionsResponse`] (`SPEC-016-R2`).
    ///
    /// # Errors
    ///
    /// Returns an error string if the mutation references empty identifiers, the lock is poisoned,
    /// or LMDB persistence fails.
    pub fn launcher_mutate_state(
        &self,
        mutation: LauncherStateMutation,
    ) -> std::result::Result<HomeSectionsResponse, String> {
        if matches!(
            mutation,
            LauncherStateMutation::Pin { .. } | LauncherStateMutation::Unpin { .. }
        ) {
            self.ensure_pinned_seeded()?;
        }

        {
            let mut state = self
                .launcher_state
                .lock()
                .map_err(|_| "launcher state lock poisoned".to_string())?;
            match mutation {
                LauncherStateMutation::Pin {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    state.pinned_initialized = true;
                    if !state
                        .pinned_items
                        .iter()
                        .any(|r| r.plugin_id == plugin_id && r.feature_code == feature_code)
                    {
                        state.pinned_items.push(LauncherItemRef {
                            plugin_id,
                            feature_code,
                        });
                    }
                    persist_pinned_items(&self.storage, &state)?;
                }
                LauncherStateMutation::Unpin {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    state.pinned_initialized = true;
                    state
                        .pinned_items
                        .retain(|r| !(r.plugin_id == plugin_id && r.feature_code == feature_code));
                    persist_pinned_items(&self.storage, &state)?;
                }
                LauncherStateMutation::ReorderPinned { items } => {
                    state.pinned_initialized = true;
                    state.pinned_items = dedupe_item_refs(items);
                    persist_pinned_items(&self.storage, &state)?;
                }
                LauncherStateMutation::RemoveRecent {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    state
                        .recent_history
                        .retain(|r| !(r.plugin_id == plugin_id && r.feature_code == feature_code));
                    state.removed_recent_keys.insert((plugin_id, feature_code));
                    persist_recent_history(&self.storage, &state)?;
                    persist_removed_recent_keys(&self.storage, &state)?;
                }
                LauncherStateMutation::PinRecommendation {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    if !state
                        .recommendation_pinned
                        .iter()
                        .any(|r| r.plugin_id == plugin_id && r.feature_code == feature_code)
                    {
                        state.recommendation_pinned.push(LauncherItemRef {
                            plugin_id,
                            feature_code,
                        });
                        persist_recommendation_pinned(&self.storage, &state)?;
                    }
                }
                LauncherStateMutation::UnpinRecommendation {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    let before = state.recommendation_pinned.len();
                    state
                        .recommendation_pinned
                        .retain(|r| !(r.plugin_id == plugin_id && r.feature_code == feature_code));
                    if state.recommendation_pinned.len() != before {
                        persist_recommendation_pinned(&self.storage, &state)?;
                    }
                }
                LauncherStateMutation::MoveRecommendationToFront {
                    plugin_id,
                    feature_code,
                } => {
                    validate_item_ref(&plugin_id, &feature_code)?;
                    if let Some(pos) = state
                        .recommendation_pinned
                        .iter()
                        .position(|r| r.plugin_id == plugin_id && r.feature_code == feature_code)
                    {
                        if pos > 0 {
                            let item = state.recommendation_pinned.remove(pos);
                            state.recommendation_pinned.insert(0, item);
                            persist_recommendation_pinned(&self.storage, &state)?;
                        }
                    }
                }
            }
        }

        self.launcher_home_sections(None)
    }

    /// Reveals the canonical local `.app` bundle or plugin directory in Finder / file manager (`SPEC-016-R2`).
    ///
    /// Looks up the path strictly from `app_catalog` or `loaded_plugins` (never trusting raw path strings).
    ///
    /// # Errors
    ///
    /// Returns an error string if spawning `/usr/bin/open -R` (or the host file manager) fails.
    pub fn reveal_in_finder(
        &self,
        plugin_id: &str,
        feature_code: &str,
    ) -> std::result::Result<bool, String> {
        let Some(raw_path) = self.resolve_target_path(plugin_id, feature_code) else {
            return Ok(false);
        };
        let candidate = Path::new(&raw_path);
        if !candidate.exists() {
            return Ok(false);
        }
        let canonical =
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&canonical)
                .status()
                .map_err(|e| format!("failed to run /usr/bin/open -R: {e}"))?;
            Ok(status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.window_controller()
                .show_item_in_folder(&canonical.to_string_lossy())?;
            Ok(true)
        }
    }

    /// Enriches `hits` with host metadata (`kind`, `icon_is_dark`, `target_path`, pin flags, `use_count`).
    pub fn enrich_hits(&self, hits: &mut [LauncherHit]) {
        let snap = self.snapshot_launcher_state();
        self.enrich_hits_with_snapshot(hits, &snap);
    }

    /// Enriches `search_features` hits and applies ZTools `recommendation_pinned` priority ordering
    /// on `"over"` matches plus `use_count` tie-breaking (`SPEC-016-R2`).
    pub fn enrich_and_rank_search_hits(&self, hits: &mut [LauncherHit]) {
        let snap = self.snapshot_launcher_state();
        self.enrich_hits_with_snapshot(hits, &snap);
        hits.sort_by(|a, b| {
            let key_a = (a.plugin_id.clone(), a.feature_code.clone());
            let key_b = (b.plugin_id.clone(), b.feature_code.clone());
            let pin_rank_a = if a.match_type == "over" {
                snap.rec_pin_index
                    .get(&key_a)
                    .copied()
                    .unwrap_or(usize::MAX)
            } else {
                usize::MAX
            };
            let pin_rank_b = if b.match_type == "over" {
                snap.rec_pin_index
                    .get(&key_b)
                    .copied()
                    .unwrap_or(usize::MAX)
            } else {
                usize::MAX
            };
            let count_a = a.use_count.unwrap_or(0);
            let count_b = b.use_count.unwrap_or(0);

            Reverse(a.score)
                .cmp(&Reverse(b.score))
                .then_with(|| pin_rank_a.cmp(&pin_rank_b))
                .then_with(|| Reverse(count_a).cmp(&Reverse(count_b)))
        });
    }

    /// Resolves and caches ZTools monochrome `icon_is_dark` status for a plugin icon URI or path.
    ///
    /// Returns `None` for full-color icons, unreadable/non-raster icons, or SVG data URLs.
    #[must_use]
    pub fn analyze_plugin_icon_cached(&self, icon_uri: &str) -> Option<bool> {
        let trimmed = icon_uri.trim();
        if trimmed.is_empty() {
            return None;
        }
        let cache_key = format!("{:x}", Sha256::digest(trimmed.as_bytes()));
        if let Ok(guard) = self.icon_analysis_cache.lock() {
            if let Some(&cached) = guard.get(&cache_key) {
                return cached;
            }
        }

        let computed = if trimmed.starts_with("data:image/") {
            analyze_icon_data_url(trimmed).icon_is_dark_hint()
        } else if let Some(icon_id) = trimmed
            .strip_prefix("aitools-plugin://__app_icon__/")
            .and_then(|s| s.split(['?', '#']).next())
            .and_then(|s| s.strip_suffix(".png"))
        {
            self.mounts
                .get_icon_source(icon_id)
                .and_then(|path| analyze_icon_path(&path).icon_is_dark_hint())
        } else if let Some((plugin_id, rel_path)) = parse_aitools_plugin_uri(trimmed) {
            self.mounts.get_root(plugin_id).and_then(|root| {
                let rel_clean = rel_path.trim_start_matches('/');
                let candidate = root.join(rel_clean);
                let canon = std::fs::canonicalize(candidate).ok()?;
                if canon.starts_with(&root) {
                    analyze_icon_path(&canon).icon_is_dark_hint()
                } else {
                    None
                }
            })
        } else {
            let p = Path::new(trimmed);
            if p.is_file() {
                analyze_icon_path(p).icon_is_dark_hint()
            } else {
                None
            }
        };

        if let Ok(mut guard) = self.icon_analysis_cache.lock() {
            if guard.len() >= MAX_ICON_ANALYSIS_CACHE {
                guard.clear();
            }
            guard.insert(cache_key, computed);
        }
        computed
    }

    /// Resolves the canonical filesystem path for `reveal_in_finder` / `LauncherHit::target_path`.
    #[must_use]
    pub fn resolve_target_path(&self, plugin_id: &str, feature_code: &str) -> Option<String> {
        if plugin_id == "system-apps" || feature_code.starts_with("app:") {
            if let Ok(apps) = self.app_catalog.read() {
                if let Some(app) = apps.get(feature_code) {
                    if !app.path.is_empty() {
                        return Some(app.path.clone());
                    }
                }
                if !feature_code.starts_with("app:") {
                    let prefixed = format!("app:{feature_code}");
                    if let Some(app) = apps.get(&prefixed) {
                        if !app.path.is_empty() {
                            return Some(app.path.clone());
                        }
                    }
                }
            }
            return None;
        }
        if let Ok(plugins) = self.loaded_plugins.read() {
            if let Some(plugin) = plugins.get(plugin_id) {
                return Some(plugin.root_dir.to_string_lossy().into_owned());
            }
        }
        None
    }

    fn record_successful_launch(&self, plugin_id: &str, feature_code: &str) {
        let pid = plugin_id.trim();
        let fcode = feature_code.trim();
        if pid.is_empty() || fcode.is_empty() {
            return;
        }
        let Ok(mut state) = self.launcher_state.lock() else {
            return;
        };
        let key = (pid.to_string(), fcode.to_string());
        let next_count = state
            .usage_stats
            .get(&key)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        state.usage_stats.insert(key.clone(), next_count);
        let cleared_removed = state.removed_recent_keys.remove(&key);

        state
            .recent_history
            .retain(|r| !(r.plugin_id == pid && r.feature_code == fcode));
        state.recent_history.insert(
            0,
            LauncherItemRef {
                plugin_id: pid.to_string(),
                feature_code: fcode.to_string(),
            },
        );
        if state.recent_history.len() > MAX_RECENT_HISTORY {
            state.recent_history.truncate(MAX_RECENT_HISTORY);
        }

        let _ = persist_recent_history(&self.storage, &state);
        let _ = persist_usage_stats(&self.storage, &state);
        if cleared_removed {
            let _ = persist_removed_recent_keys(&self.storage, &state);
        }
    }

    pub(crate) fn ensure_pinned_seeded(&self) -> std::result::Result<(), String> {
        let already_initialized = self
            .launcher_state
            .lock()
            .is_ok_and(|g| g.pinned_initialized);
        if already_initialized {
            return Ok(());
        }
        let seed_candidates = self.curated_pinned_seed_candidates();
        if seed_candidates.is_empty() {
            return Ok(());
        }
        let mut state = self
            .launcher_state
            .lock()
            .map_err(|_| "launcher state lock poisoned".to_string())?;
        if !state.pinned_initialized {
            state.pinned_items = seed_candidates;
            state.pinned_initialized = true;
            persist_pinned_items(&self.storage, &state)?;
        }
        Ok(())
    }

    fn snapshot_launcher_state(&self) -> LauncherStateSnapshot {
        let Ok(state) = self.launcher_state.lock() else {
            return LauncherStateSnapshot::default();
        };
        let pinned_set: HashSet<(String, String)> = state
            .pinned_items
            .iter()
            .map(|r| (r.plugin_id.clone(), r.feature_code.clone()))
            .collect();
        let rec_pin_index: HashMap<(String, String), usize> = state
            .recommendation_pinned
            .iter()
            .enumerate()
            .map(|(idx, r)| ((r.plugin_id.clone(), r.feature_code.clone()), idx))
            .collect();
        LauncherStateSnapshot {
            recent_history: state.recent_history.clone(),
            pinned_items: state.pinned_items.clone(),
            pinned_set,
            rec_pin_index,
            usage_stats: state.usage_stats.clone(),
            removed_recent_keys: state.removed_recent_keys.clone(),
        }
    }

    fn enrich_hits_with_snapshot(&self, hits: &mut [LauncherHit], snap: &LauncherStateSnapshot) {
        for hit in hits.iter_mut() {
            if let Some(kind) = self.classify_hit_kind(&hit.plugin_id, &hit.feature_code) {
                hit.kind = Some(kind);
            }
            if hit.plugin_id == "system-apps" || hit.feature_code.starts_with("app:") {
                hit.icon_is_dark = None;
            } else {
                hit.icon_is_dark = hit
                    .icon
                    .as_deref()
                    .and_then(|uri| self.analyze_plugin_icon_cached(uri));
            }
            if hit.target_path.is_none() {
                hit.target_path = self.resolve_target_path(&hit.plugin_id, &hit.feature_code);
            }
            let key = (hit.plugin_id.clone(), hit.feature_code.clone());
            hit.is_pinned = snap.pinned_set.contains(&key).then_some(true);
            hit.is_recommendation_pinned = snap.rec_pin_index.contains_key(&key).then_some(true);
            hit.use_count = snap.usage_stats.get(&key).copied().filter(|&c| c > 0);
        }
    }

    fn resolve_hit_for_ref(
        &self,
        item: &LauncherItemRef,
        section_match_type: &str,
        idx: usize,
    ) -> Option<LauncherHit> {
        let score = i32::try_from(100usize.saturating_sub(idx))
            .unwrap_or(1)
            .max(1);
        let meta = self
            .search_index
            .read()
            .ok()
            .and_then(|index| index.feature_meta(&item.plugin_id, &item.feature_code));
        if let Some(meta) = meta {
            return Some(LauncherHit {
                feature_code: item.feature_code.clone(),
                plugin_id: item.plugin_id.clone(),
                label: meta.label,
                plugin_name: meta.plugin_name,
                description: meta.explain.clone(),
                icon: meta.icon,
                kind: self.classify_hit_kind(&item.plugin_id, &item.feature_code),
                score,
                match_type: section_match_type.to_string(),
                confidence: 1.0,
                explain: meta.explain,
                icon_is_dark: None,
                target_path: None,
                is_pinned: None,
                is_recommendation_pinned: None,
                use_count: None,
            });
        }

        if item.plugin_id == "system-apps" || item.feature_code.starts_with("app:") {
            if let Ok(apps) = self.app_catalog.read() {
                let lookup = if item.feature_code.starts_with("app:") {
                    item.feature_code.clone()
                } else {
                    format!("app:{}", item.feature_code)
                };
                if let Some(app) = apps.get(&lookup) {
                    let explain = app
                        .bundle_id
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map_or_else(|| app.path.clone(), ToString::to_string);
                    let icon = app
                        .icon_path
                        .as_deref()
                        .and_then(|p| self.mounts.register_icon(p));
                    return Some(LauncherHit {
                        feature_code: lookup,
                        plugin_id: "system-apps".to_string(),
                        label: app.name.clone(),
                        plugin_name: "Application".to_string(),
                        description: explain.clone(),
                        icon,
                        kind: Some("app".to_string()),
                        score,
                        match_type: section_match_type.to_string(),
                        confidence: 1.0,
                        explain,
                        icon_is_dark: None,
                        target_path: Some(app.path.clone()),
                        is_pinned: None,
                        is_recommendation_pinned: None,
                        use_count: None,
                    });
                }
            }
            return None;
        }

        if item.feature_code.starts_with("macro:") {
            let label = item.feature_code.trim_start_matches("macro:").to_string();
            return Some(LauncherHit {
                feature_code: item.feature_code.clone(),
                plugin_id: item.plugin_id.clone(),
                label: label.clone(),
                plugin_name: "Saved Macros".to_string(),
                description: label.clone(),
                icon: None,
                kind: Some("macro".to_string()),
                score,
                match_type: section_match_type.to_string(),
                confidence: 1.0,
                explain: label,
                icon_is_dark: None,
                target_path: None,
                is_pinned: None,
                is_recommendation_pinned: None,
                use_count: None,
            });
        }

        None
    }

    fn resolve_feature_icon_uri(
        &self,
        plugin: &LoadedPlugin,
        feature_code: &str,
    ) -> Option<String> {
        let mut map_local_icon = |local_path: &Path| -> Option<String> {
            if local_path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
            {
                None
            } else {
                self.mounts.register_icon(local_path)
            }
        };

        let mut feature_raw_icon: Option<String> = None;
        if let Some(ref v1) = plugin.v1 {
            feature_raw_icon = v1
                .features
                .iter()
                .find(|f| f.code == feature_code)
                .and_then(|f| f.icon.clone());
        } else if let Some(ref v2) = plugin.v2 {
            feature_raw_icon = v2
                .features
                .iter()
                .find(|f| f.code == feature_code)
                .and_then(|f| f.icon.clone());
        }
        if feature_raw_icon.is_none() {
            if let Ok(dyn_map) = self.dynamic_features.read() {
                if let Some(list) = dyn_map.get(&plugin.id) {
                    feature_raw_icon = list
                        .iter()
                        .find(|f| f.code == feature_code)
                        .and_then(|f| f.icon.clone());
                }
            }
        }

        plugin
            .resolve_icon_with_mapper(feature_raw_icon.as_deref(), &mut map_local_icon)
            .or_else(|| {
                plugin.resolve_icon_with_mapper(plugin.declared_icon_raw(), &mut map_local_icon)
            })
            .or_else(|| plugin.icon.clone())
    }
}

#[derive(Debug, Default)]
struct LauncherStateSnapshot {
    recent_history: Vec<LauncherItemRef>,
    pinned_items: Vec<LauncherItemRef>,
    pinned_set: HashSet<(String, String)>,
    rec_pin_index: HashMap<(String, String), usize>,
    usage_stats: HashMap<(String, String), u64>,
    removed_recent_keys: HashSet<(String, String)>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ItemsDoc {
    #[serde(default)]
    items: Vec<LauncherItemRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PinnedItemsDoc {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    items: Vec<LauncherItemRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct UsageStatEntry {
    plugin_id: String,
    feature_code: String,
    count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct UsageStatsDoc {
    #[serde(default)]
    items: Vec<UsageStatEntry>,
}

pub(crate) fn load_host_launcher_state(storage: &StorageEngine) -> HostLauncherState {
    let mut state = HostLauncherState::default();

    if let Ok(Some(doc)) = storage.get_doc(HOST_LAUNCHER_BUCKET, DOC_RECENT_HISTORY) {
        if let Ok(parsed) = serde_json::from_value::<ItemsDoc>(doc.data) {
            let mut items = dedupe_item_refs(parsed.items);
            if items.len() > MAX_RECENT_HISTORY {
                items.truncate(MAX_RECENT_HISTORY);
            }
            state.recent_history = items;
        }
    }

    if let Ok(Some(doc)) = storage.get_doc(HOST_LAUNCHER_BUCKET, DOC_PINNED_ITEMS) {
        if let Ok(parsed) = serde_json::from_value::<PinnedItemsDoc>(doc.data) {
            state.pinned_initialized = parsed.initialized || !parsed.items.is_empty();
            state.pinned_items = dedupe_item_refs(parsed.items);
        }
    }

    if let Ok(Some(doc)) = storage.get_doc(HOST_LAUNCHER_BUCKET, DOC_RECOMMENDATION_PINNED) {
        if let Ok(parsed) = serde_json::from_value::<ItemsDoc>(doc.data) {
            state.recommendation_pinned = dedupe_item_refs(parsed.items);
        }
    }

    if let Ok(Some(doc)) = storage.get_doc(HOST_LAUNCHER_BUCKET, DOC_USAGE_STATS) {
        if let Ok(parsed) = serde_json::from_value::<UsageStatsDoc>(doc.data) {
            for entry in parsed.items {
                if !entry.plugin_id.trim().is_empty()
                    && !entry.feature_code.trim().is_empty()
                    && entry.count > 0
                {
                    state
                        .usage_stats
                        .insert((entry.plugin_id, entry.feature_code), entry.count);
                }
            }
        }
    }

    if let Ok(Some(doc)) = storage.get_doc(HOST_LAUNCHER_BUCKET, DOC_REMOVED_RECENT_KEYS) {
        if let Ok(parsed) = serde_json::from_value::<ItemsDoc>(doc.data) {
            for r in dedupe_item_refs(parsed.items) {
                state
                    .removed_recent_keys
                    .insert((r.plugin_id, r.feature_code));
            }
        }
    }

    state
}

fn save_host_doc(
    storage: &StorageEngine,
    doc_id: &str,
    data: Value,
) -> std::result::Result<(), String> {
    let existing_rev = storage
        .get_doc(HOST_LAUNCHER_BUCKET, doc_id)
        .map_err(|e| e.to_string())?
        .and_then(|d| d.rev);
    let res = storage
        .put_doc(
            HOST_LAUNCHER_BUCKET,
            Document {
                id: doc_id.to_string(),
                rev: existing_rev,
                data,
            },
        )
        .map_err(|e| e.to_string())?;
    if res.error == Some(true) {
        return Err(res
            .message
            .unwrap_or_else(|| format!("failed writing {doc_id} to LMDB")));
    }
    Ok(())
}

fn persist_recent_history(
    storage: &StorageEngine,
    state: &HostLauncherState,
) -> std::result::Result<(), String> {
    let payload = serde_json::to_value(ItemsDoc {
        items: state.recent_history.clone(),
    })
    .map_err(|e| e.to_string())?;
    save_host_doc(storage, DOC_RECENT_HISTORY, payload)
}

fn persist_pinned_items(
    storage: &StorageEngine,
    state: &HostLauncherState,
) -> std::result::Result<(), String> {
    let payload = serde_json::to_value(PinnedItemsDoc {
        initialized: state.pinned_initialized,
        items: state.pinned_items.clone(),
    })
    .map_err(|e| e.to_string())?;
    save_host_doc(storage, DOC_PINNED_ITEMS, payload)
}

fn persist_recommendation_pinned(
    storage: &StorageEngine,
    state: &HostLauncherState,
) -> std::result::Result<(), String> {
    let payload = serde_json::to_value(ItemsDoc {
        items: state.recommendation_pinned.clone(),
    })
    .map_err(|e| e.to_string())?;
    save_host_doc(storage, DOC_RECOMMENDATION_PINNED, payload)
}

fn persist_usage_stats(
    storage: &StorageEngine,
    state: &HostLauncherState,
) -> std::result::Result<(), String> {
    let mut items: Vec<UsageStatEntry> = state
        .usage_stats
        .iter()
        .filter(|(_, &count)| count > 0)
        .map(|((plugin_id, feature_code), &count)| UsageStatEntry {
            plugin_id: plugin_id.clone(),
            feature_code: feature_code.clone(),
            count,
        })
        .collect();
    items.sort_by(|a, b| {
        a.plugin_id
            .cmp(&b.plugin_id)
            .then_with(|| a.feature_code.cmp(&b.feature_code))
    });
    let payload = serde_json::to_value(UsageStatsDoc { items }).map_err(|e| e.to_string())?;
    save_host_doc(storage, DOC_USAGE_STATS, payload)
}

fn persist_removed_recent_keys(
    storage: &StorageEngine,
    state: &HostLauncherState,
) -> std::result::Result<(), String> {
    let mut items: Vec<LauncherItemRef> = state
        .removed_recent_keys
        .iter()
        .map(|(plugin_id, feature_code)| LauncherItemRef {
            plugin_id: plugin_id.clone(),
            feature_code: feature_code.clone(),
        })
        .collect();
    items.sort_by(|a, b| {
        a.plugin_id
            .cmp(&b.plugin_id)
            .then_with(|| a.feature_code.cmp(&b.feature_code))
    });
    let payload = serde_json::to_value(ItemsDoc { items }).map_err(|e| e.to_string())?;
    save_host_doc(storage, DOC_REMOVED_RECENT_KEYS, payload)
}

fn validate_item_ref(plugin_id: &str, feature_code: &str) -> std::result::Result<(), String> {
    if plugin_id.trim().is_empty() || feature_code.trim().is_empty() {
        return Err("plugin_id and feature_code must not be empty".to_string());
    }
    Ok(())
}

fn dedupe_item_refs(items: Vec<LauncherItemRef>) -> Vec<LauncherItemRef> {
    let mut seen: HashSet<(String, String)> = HashSet::with_capacity(items.len());
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.plugin_id.trim().is_empty() || item.feature_code.trim().is_empty() {
            continue;
        }
        if seen.insert((item.plugin_id.clone(), item.feature_code.clone())) {
            out.push(item);
        }
    }
    out
}

fn parse_aitools_plugin_uri(raw_uri: &str) -> Option<(&str, &str)> {
    let rest = raw_uri.strip_prefix("aitools-plugin://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = rest.split_once('/')?;
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host, path))
}

fn conflict_write_result(id: &str, message: &str) -> PluginDataWriteResult {
    PluginDataWriteResult {
        id: id.to_string(),
        rev: None,
        ok: None,
        error: Some(true),
        name: Some("conflict".to_string()),
        message: Some(message.to_string()),
    }
}

fn map_core_err(err: aitools_core::AitoolsError) -> SyncOpError {
    let status = match err {
        aitools_core::AitoolsError::Forbidden(_) => 403,
        aitools_core::AitoolsError::BudgetExceeded(_) => 413,
        _ => 400,
    };
    SyncOpError::new(status, err.to_string())
}

fn build_enter_action(plugin: &LoadedPlugin, feature_code: &str, query: &str) -> Value {
    let cmd_type = infer_action_type(plugin, feature_code, query);
    json!({
        "code": feature_code,
        "type": cmd_type,
        "payload": query,
        "from": "main",
    })
}

fn infer_action_type(plugin: &LoadedPlugin, feature_code: &str, query: &str) -> &'static str {
    let features: Vec<Feature> = if let Some(ref v1) = plugin.v1 {
        v1.features.clone()
    } else if let Some(ref v2) = plugin.v2 {
        v2.features.clone()
    } else {
        Vec::new()
    };
    if let Some(feat) = features.iter().find(|f| f.code == feature_code) {
        let mut saw_over = false;
        let mut saw_regex = false;
        for cmd in &feat.cmds {
            match cmd {
                Cmd::Text(t) if t.eq_ignore_ascii_case(query.trim()) => return "text",
                Cmd::Typed(typed) => match typed.type_ {
                    aitools_core::CmdType::Over => saw_over = true,
                    aitools_core::CmdType::Regex => saw_regex = true,
                    aitools_core::CmdType::Files => return "files",
                    aitools_core::CmdType::Img => return "img",
                    aitools_core::CmdType::Window => return "window",
                },
                Cmd::Text(_) => {}
            }
        }
        if saw_regex {
            return "regex";
        }
        if saw_over {
            return "over";
        }
    }
    "text"
}

fn is_list_mode_feature(
    info: &PreloadLoadInfo,
    feature_code: &str,
    explicit_kind: Option<&str>,
) -> bool {
    if explicit_kind == Some("list") {
        return true;
    }
    info.export_features
        .iter()
        .any(|f| f.code == feature_code && f.mode.eq_ignore_ascii_case("list"))
}

fn extract_list_items_from_value(val: &Value) -> Vec<ListItem> {
    let arr_opt = val
        .as_array()
        .or_else(|| val.get("items").and_then(Value::as_array));
    let Some(arr) = arr_opt else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            serde_json::from_value::<ListItem>(item.clone())
                .ok()
                .or_else(|| {
                    let title = item
                        .get("title")
                        .or_else(|| item.get("name"))
                        .and_then(Value::as_str)?;
                    Some(ListItem {
                        title: title.to_string(),
                        description: item
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        icon: item
                            .get("icon")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                        payload: item.get("payload").cloned().unwrap_or_else(|| item.clone()),
                    })
                })
        })
        .collect()
}

fn extract_headless_message(val: &Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        return (!s.trim().is_empty()).then(|| s.to_string());
    }
    val.get("message")
        .or_else(|| val.get("result"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
}

fn extract_ai_prompt_and_system(payload: &Value) -> (Option<String>, String) {
    if let Some(prompt) = payload.get("prompt").and_then(Value::as_str) {
        let sys = payload
            .get("system")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        return (sys, prompt.to_string());
    }
    if let Some(messages) = payload.get("messages").and_then(Value::as_array) {
        let mut system_parts = Vec::new();
        let mut user_parts = Vec::new();
        for msg in messages {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = msg
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if role == "system" {
                if !content.is_empty() {
                    system_parts.push(content.to_string());
                }
            } else if !content.is_empty() {
                user_parts.push(format!("{role}: {content}"));
            }
        }
        let sys = (!system_parts.is_empty()).then(|| system_parts.join("\n"));
        return (sys, user_parts.join("\n"));
    }
    (None, payload.to_string())
}

fn execute_wasm_plugin(
    host: &WasmPluginHost,
    plugin_id: &str,
    feature_code: &str,
    query: &str,
    wasm_path: &Path,
) -> std::result::Result<Value, String> {
    let mut instance = host
        .instantiate_module_file(plugin_id, wasm_path)
        .map_err(|e| format!("failed to load Wasm module '{}': {e}", wasm_path.display()))?;
    let fuel_before = instance.remaining_fuel().unwrap_or(0);
    let api_version = instance
        .call_i32_void("aitools_plugin_api_version")
        .unwrap_or(1);
    let query_status = instance.call_i32_void("on_query").unwrap_or(0);
    let tool_status = instance.call_i32_void("execute_tool").unwrap_or(0);
    let fuel_after = instance.remaining_fuel().unwrap_or(fuel_before);
    let fuel_consumed = fuel_before.saturating_sub(fuel_after);

    let trimmed = query.trim();
    let transformed = match plugin_id {
        "json-codec" => run_builtin_json_codec(feature_code, trimmed, &mut instance),
        "timestamp-calc" => run_builtin_timestamp_calc(trimmed, &mut instance),
        _ => json!({
            "feature_code": feature_code,
            "query": trimmed,
            "query_status": query_status,
            "tool_status": tool_status,
        }),
    };

    Ok(json!({
        "plugin_id": plugin_id,
        "feature_code": feature_code,
        "wasm_api_version": api_version,
        "fuel_consumed": fuel_consumed,
        "result": transformed,
    }))
}

fn run_builtin_json_codec(
    feature_code: &str,
    input: &str,
    instance: &mut WasmInstanceHandle,
) -> Value {
    if input.is_empty() {
        return json!({
            "mode": "ready",
            "hint": "Type or paste JSON or text after the keyword to format or Base64-encode.",
        });
    }
    if feature_code == "codec_base64_url" || (!input.starts_with('{') && !input.starts_with('[')) {
        let wasm_len = instance
            .call_i32_unary(
                "base64_encoded_len",
                i32::try_from(input.len()).unwrap_or(i32::MAX),
            )
            .unwrap_or(0);
        let encoded = BASE64_STANDARD.encode(input.as_bytes());
        return json!({
            "mode": "base64",
            "encoded": encoded,
            "wasm_predicted_len": wasm_len,
        });
    }
    match serde_json::from_str::<Value>(input) {
        Ok(parsed) => json!({
            "mode": "format_json",
            "valid": true,
            "pretty": serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| input.to_string()),
            "minified": serde_json::to_string(&parsed).unwrap_or_else(|_| input.to_string()),
        }),
        Err(err) => json!({
            "mode": "format_json",
            "valid": false,
            "error": err.to_string(),
        }),
    }
}

fn run_builtin_timestamp_calc(input: &str, instance: &mut WasmInstanceHandle) -> Value {
    let now_secs = now_unix_ms() / 1000;
    if input.is_empty() {
        let days = instance
            .call_i32_unary(
                "unix_epoch_days",
                i32::try_from(now_secs).unwrap_or(i32::MAX),
            )
            .unwrap_or(0);
        return json!({
            "mode": "now",
            "epoch_seconds": now_secs,
            "epoch_ms": now_secs * 1000,
            "utc_iso": format_unix_iso_utc(now_secs),
            "wasm_epoch_days": days,
        });
    }
    if let Ok(raw_num) = input.parse::<i64>() {
        let secs_u64 = if raw_num.abs() > 10_000_000_000 {
            u64::try_from((raw_num / 1000).max(0)).unwrap_or(0)
        } else {
            u64::try_from(raw_num.max(0)).unwrap_or(0)
        };
        let days = instance
            .call_i32_unary(
                "unix_epoch_days",
                i32::try_from(secs_u64).unwrap_or(i32::MAX),
            )
            .unwrap_or(0);
        return json!({
            "mode": "epoch",
            "epoch_seconds": secs_u64,
            "utc_iso": format_unix_iso_utc(secs_u64),
            "wasm_epoch_days": days,
        });
    }
    json!({
        "mode": "input",
        "now_epoch_seconds": now_secs,
        "now_utc_iso": format_unix_iso_utc(now_secs),
        "input": input,
    })
}

fn format_unix_iso_utc(secs: u64) -> String {
    // Civil date conversion from Unix epoch seconds (Howard Hinnant algorithm, UTC).
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn exec_display_op(sub: &str, payload: &Value) -> Value {
    let primary = json!({
        "id": 1,
        "bounds": { "x": 0, "y": 0, "width": 1728, "height": 1117 },
        "workArea": { "x": 0, "y": 25, "width": 1728, "height": 1092 },
        "size": { "width": 1728, "height": 1117 },
        "workAreaSize": { "width": 1728, "height": 1092 },
        "scaleFactor": 2.0,
        "rotation": 0,
        "touchSupport": "unknown"
    });
    match sub {
        "getPrimaryDisplay" | "getDisplayNearestPoint" => primary,
        "getAllDisplays" => Value::Array(vec![primary]),
        "getCursorScreenPoint" => json!({ "x": 0, "y": 0 }),
        "dipToScreenPoint" | "screenToDipPoint" => json!({
            "x": payload.get("x").and_then(Value::as_f64).unwrap_or(0.0),
            "y": payload.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        }),
        _ => Value::Null,
    }
}

fn resolve_known_path(name: &str) -> String {
    let path = match name {
        "home" => dirs::home_dir(),
        "appData" => dirs::config_dir(),
        "userData" => dirs::data_dir(),
        "temp" => Some(std::env::temp_dir()),
        "desktop" => dirs::desktop_dir(),
        "documents" => dirs::document_dir(),
        "downloads" => dirs::download_dir(),
        "music" => dirs::audio_dir(),
        "pictures" => dirs::picture_dir(),
        "videos" => dirs::video_dir(),
        "exe" => std::env::current_exe().ok(),
        _ => dirs::home_dir(),
    };
    path.map_or_else(String::new, |p| p.to_string_lossy().to_string())
}

fn resolve_native_id() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "aitools-local".to_string());
    format!("aitools-{host}")
}

fn detect_os_dark_mode() -> bool {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
        {
            return String::from_utf8_lossy(&out.stdout)
                .trim()
                .eq_ignore_ascii_case("dark");
        }
    }
    true
}

fn read_frontmost_browser_url(bundle_id: Option<&str>) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let script = match bundle_id {
            Some("com.apple.Safari") => {
                "tell application \"Safari\" to if (count of documents) > 0 then get URL of front document"
            }
            Some("com.google.Chrome") => {
                "tell application \"Google Chrome\" to if (count of windows) > 0 then get URL of active tab of front window"
            }
            Some("company.thebrowser.Browser") => {
                "tell application \"Arc\" to if (count of windows) > 0 then get URL of active tab of front window"
            }
            _ => return None,
        };
        if let Ok(out) = std::process::Command::new("osascript")
            .args(["-e", script])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    let _ = bundle_id;
    None
}

fn read_frontmost_folder_path(bundle_id: Option<&str>) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if bundle_id == Some("com.apple.finder") {
            let script = "tell application \"Finder\" to if (count of windows) > 0 then POSIX path of (target of front window as alias)";
            if let Ok(out) = std::process::Command::new("osascript")
                .args(["-e", script])
                .output()
            {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
    }
    let _ = bundle_id;
    None
}
