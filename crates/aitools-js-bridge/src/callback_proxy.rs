//! Dual-heap `rquickjs` callback proxy (`CallbackTable`) and real-time `callbackSetList` push (`SPEC-004`).

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::info_span;

use crate::shim::build_initialization_script;
use crate::{JsBridgeError, Result};

/// Opaque callback handle issued across the QuickJS <-> Tauri IPC boundary (`SPEC-004-R1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallbackHandle(u64);

impl CallbackHandle {
    /// Returns the numeric identifier backing this callback handle.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Standard list-mode item pushed by `callbackSetList` (`SPEC-004-R2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    /// Primary title displayed in the launcher or list row.
    pub title: String,
    /// Secondary descriptive subtitle for the row.
    #[serde(default)]
    pub description: String,
    /// Optional icon URL, data URI, or relative asset path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Arbitrary item payload forwarded back on selection.
    #[serde(default)]
    pub payload: Value,
}

/// Real-time `callbackSetList` event forwarded to the host Webview Channel (`SPEC-004-R2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPushEvent {
    /// Active wake or invocation trace correlation identifier.
    pub trace_id: String,
    /// Owning plugin identifier.
    pub plugin_id: String,
    /// Opaque handle identifying the active list callback session.
    pub handle: CallbackHandle,
    /// Batch of list items pushed by the plugin.
    pub items: Vec<ListItem>,
}

/// Callback sink invoked whenever QuickJS calls `callbackSetList`.
pub trait ListPushSink: Send + Sync {
    /// Delivers a pushed batch of [`ListItem`] values to the host consumer.
    fn push_list(&self, event: ListPushEvent);
}

/// In-memory collector sink useful for headless tests and DSH inspection.
#[derive(Default)]
pub struct RecordingListSink {
    events: Mutex<Vec<ListPushEvent>>,
}

impl RecordingListSink {
    /// Returns a snapshot of all [`ListPushEvent`] records pushed into this sink.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn events(&self) -> Vec<ListPushEvent> {
        self.events
            .lock()
            .expect("list sink mutex poisoned")
            .clone()
    }
}

impl ListPushSink for RecordingListSink {
    fn push_list(&self, event: ListPushEvent) {
        self.events
            .lock()
            .expect("list sink mutex poisoned")
            .push(event);
    }
}

#[derive(Debug, Clone)]
struct CallbackEntry {
    plugin_id: String,
    feature_code: String,
    mode: String,
}

/// Maps opaque [`CallbackHandle`] IDs to legacy plugin `window.exports` handlers (`SPEC-004-R1`).
#[derive(Default)]
pub struct CallbackTable {
    next_id: AtomicU64,
    entries: Mutex<HashMap<CallbackHandle, CallbackEntry>>,
}

impl CallbackTable {
    /// Creates an empty [`CallbackTable`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `(plugin_id, feature_code, mode)` and issues a unique [`CallbackHandle`].
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn register(&self, plugin_id: &str, feature_code: &str, mode: &str) -> CallbackHandle {
        let id = CallbackHandle(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        self.entries
            .lock()
            .expect("callback table mutex poisoned")
            .insert(
                id,
                CallbackEntry {
                    plugin_id: plugin_id.to_string(),
                    feature_code: feature_code.to_string(),
                    mode: mode.to_string(),
                },
            );
        id
    }

    /// Looks up `(plugin_id, feature_code, mode)` associated with `handle`.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn lookup(&self, handle: CallbackHandle) -> Option<(String, String, String)> {
        self.entries
            .lock()
            .expect("callback table mutex poisoned")
            .get(&handle)
            .map(|e| (e.plugin_id.clone(), e.feature_code.clone(), e.mode.clone()))
    }

    /// Removes all callback registrations belonging to `plugin_id`, returning the count removed.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn remove_plugin(&self, plugin_id: &str) -> usize {
        let mut map = self.entries.lock().expect("callback table mutex poisoned");
        let before = map.len();
        map.retain(|_, v| v.plugin_id != plugin_id);
        before - map.len()
    }
}

const PRELOAD_ENV_SHIM: &str = r#"
(function() {
    if (typeof globalThis.module === 'undefined') {
        globalThis.module = { exports: {} };
    }
    if (typeof globalThis.exports === 'undefined') {
        globalThis.exports = globalThis.module.exports;
    }
    if (typeof globalThis.window.exports === 'undefined') {
        globalThis.window.exports = globalThis.exports;
    }
    if (typeof globalThis.process === 'undefined') {
        globalThis.process = {
            platform: 'darwin',
            env: {},
            versions: { node: '20.0.0' },
            cwd: function() { return '/'; }
        };
    }
})();
"#;

/// Executes a legacy `preload.js` feature inside `rquickjs` with `CallbackTable` & `callbackSetList` proxy (`SPEC-004`).
///
/// # Errors
///
/// Returns [`JsBridgeError::QuickJs`] if QuickJS runtime/context initialization, script evaluation,
/// or JSON serialization fails.
#[expect(
    clippy::too_many_arguments,
    reason = "preserving public signature used by tauri-plugin-aitools"
)]
pub fn invoke_preload_list_feature(
    plugin_id: &str,
    trace_id: &str,
    preload_code: &str,
    feature_code: &str,
    action_payload: &Value,
    search_query: Option<&str>,
    table: &CallbackTable,
    sink: Arc<dyn ListPushSink>,
) -> Result<CallbackHandle> {
    let _span = info_span!(
        "quickjs_callback_proxy",
        trace_id = %trace_id,
        plugin_id = %plugin_id,
        feature = %feature_code
    )
    .entered();

    let runtime = Runtime::new().map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
    runtime.set_memory_limit(16 * 1024 * 1024);
    let context = Context::full(&runtime).map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

    let handle = table.register(plugin_id, feature_code, "list");
    let bootstrap = build_initialization_script(plugin_id);

    let pid_owned = plugin_id.to_string();
    let tid_owned = trace_id.to_string();

    context.with(|ctx| -> Result<()> {
        let push_fn = Function::new(ctx.clone(), move |json_items: String| {
            let items: Vec<ListItem> = serde_json::from_str(&json_items).unwrap_or_default();
            sink.push_list(ListPushEvent {
                trace_id: tid_owned.clone(),
                plugin_id: pid_owned.clone(),
                handle,
                items,
            });
        })
        .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

        ctx.globals()
            .set("__aitools_push_list__", push_fn)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

        ctx.eval::<(), _>(bootstrap)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
        ctx.eval::<(), _>(PRELOAD_ENV_SHIM)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
        ctx.eval::<(), _>(preload_code)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

        let feature_lit = serde_json::to_string(feature_code)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
        let action_lit = serde_json::to_string(action_payload)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;

        let dispatch_script = if let Some(query) = search_query {
            let query_lit =
                serde_json::to_string(query).map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
            format!(
                r#"(function() {{
                    const reg = (globalThis.window && globalThis.window.exports) || globalThis.exports || {{}};
                    const entry = reg[{feature_lit}];
                    if (!entry || !entry.args) throw new Error('missing exports entry');
                    const push = (items) => globalThis.__aitools_push_list__(JSON.stringify(items || []));
                    if (typeof entry.args.enter === 'function') entry.args.enter({action_lit}, push);
                    if (typeof entry.args.search === 'function') entry.args.search({action_lit}, {query_lit}, push);
                }})();"#
            )
        } else {
            format!(
                r#"(function() {{
                    const reg = (globalThis.window && globalThis.window.exports) || globalThis.exports || {{}};
                    const entry = reg[{feature_lit}];
                    if (!entry || !entry.args) throw new Error('missing exports entry');
                    const push = (items) => globalThis.__aitools_push_list__(JSON.stringify(items || []));
                    if (typeof entry.args.enter === 'function') entry.args.enter({action_lit}, push);
                }})();"#
            )
        };

        ctx.eval::<(), _>(dispatch_script)
            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
        Ok(())
    })?;

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_004_r1_and_r2_callback_table_proxies_exports_and_streams_set_list() {
        let preload = r#"
            window.exports = {
                "quick-calc": {
                    mode: "list",
                    args: {
                        enter: function(action, callbackSetList) {
                            callbackSetList([{ title: "Ready", description: action.code }]);
                        },
                        search: function(action, searchWord, callbackSetList) {
                            callbackSetList([{ title: "Result: " + searchWord, description: "live" }]);
                        }
                    }
                }
            };
        "#;

        let table = CallbackTable::new();
        let sink = Arc::new(RecordingListSink::default());
        let handle = invoke_preload_list_feature(
            "calc-plugin",
            "wake-42",
            preload,
            "quick-calc",
            &json!({ "code": "quick-calc" }),
            Some("2+2"),
            &table,
            Arc::clone(&sink) as Arc<dyn ListPushSink>,
        )
        .unwrap();

        assert_eq!(
            table.lookup(handle),
            Some(("calc-plugin".into(), "quick-calc".into(), "list".into()))
        );
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].items[0].title, "Ready");
        assert_eq!(events[1].items[0].title, "Result: 2+2");
        assert_eq!(events[1].trace_id, "wake-42");
        assert_eq!(table.remove_plugin("calc-plugin"), 1);
    }
}
