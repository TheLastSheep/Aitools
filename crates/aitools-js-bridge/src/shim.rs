//! Frozen triple-global shim (`window.aitools === window.ztools === window.utools`) (`SPEC-008-R1`, `INV-SEC-02`).

/// Deterministic initialization script injected via `WebviewBuilder::initialization_script`
/// prior to navigation (`INV-SEC-02`).
pub const TRIPLE_GLOBAL_SHIM_JS: &str = r#"(function() {
    'use strict';

    if (typeof globalThis.window === 'undefined') {
        globalThis.window = globalThis;
    }

    const syncEndpoint = (pluginId, op) =>
        `aitools-sync://${encodeURIComponent(pluginId)}/${op}`;

    function createApi(pluginId) {
        const db = Object.freeze({
            put(doc) {
                if (typeof globalThis.__aitools_sync_call__ === 'function') {
                    return globalThis.__aitools_sync_call__(syncEndpoint(pluginId, 'db/put'), doc);
                }
                return { id: doc && doc._id ? doc._id : '', ok: true };
            },
            get(id) {
                if (typeof globalThis.__aitools_sync_call__ === 'function') {
                    return globalThis.__aitools_sync_call__(syncEndpoint(pluginId, 'db/get'), { id });
                }
                return null;
            },
            remove(docOrId) {
                if (typeof globalThis.__aitools_sync_call__ === 'function') {
                    return globalThis.__aitools_sync_call__(syncEndpoint(pluginId, 'db/remove'), docOrId);
                }
                return { id: typeof docOrId === 'string' ? docOrId : '', ok: true };
            },
            allDocs(key) {
                if (typeof globalThis.__aitools_sync_call__ === 'function') {
                    return globalThis.__aitools_sync_call__(syncEndpoint(pluginId, 'db/allDocs'), { key });
                }
                return [];
            },
        });

        const dbStorage = Object.freeze({
            setItem(key, value) {
                return db.put({ _id: String(key), value });
            },
            getItem(key) {
                const doc = db.get(String(key));
                return doc ? doc.value : null;
            },
            removeItem(key) {
                return db.remove(String(key));
            },
        });

        const api = {
            __pluginId: pluginId,
            isDev: false,
            db,
            dbStorage,
            onPluginEnter(cb) {
                globalThis.__aitools_on_enter__ = cb;
            },
            onPluginOut(cb) {
                globalThis.__aitools_on_out__ = cb;
            },
            copyText(text) {
                if (typeof globalThis.__aitools_sync_call__ === 'function') {
                    return globalThis.__aitools_sync_call__(syncEndpoint(pluginId, 'clipboard/copyText'), { text: String(text) });
                }
                return true;
            },
            hideMainWindow() {
                return true;
            },
            showMainWindow() {
                return true;
            },
        };

        return Object.freeze(api);
    }

    const pluginId = typeof globalThis.__AITOOLS_PLUGIN_ID__ === 'string'
        ? globalThis.__AITOOLS_PLUGIN_ID__
        : 'unknown';

    const sharedShim = createApi(pluginId);

    for (const globalName of ['aitools', 'ztools', 'utools']) {
        Object.defineProperty(globalThis, globalName, {
            value: sharedShim,
            writable: false,
            enumerable: true,
            configurable: false,
        });
        if (globalThis.window !== globalThis) {
            Object.defineProperty(globalThis.window, globalName, {
                value: sharedShim,
                writable: false,
                enumerable: true,
                configurable: false,
            });
        }
    }
})();"#;

/// Builds a plugin-scoped initialization script binding `__AITOOLS_PLUGIN_ID__` before `TRIPLE_GLOBAL_SHIM_JS`.
pub fn build_triple_global_shim(plugin_id: &str) -> String {
    let escaped = serde_json::to_string(plugin_id).unwrap_or_else(|_| "\"unknown\"".to_string());
    format!("globalThis.__AITOOLS_PLUGIN_ID__ = {escaped};\n{TRIPLE_GLOBAL_SHIM_JS}")
}

/// Alias matching `WebviewBuilder::initialization_script` callers (`INV-SEC-02`).
pub fn build_initialization_script(plugin_id: &str) -> String {
    build_triple_global_shim(plugin_id)
}
