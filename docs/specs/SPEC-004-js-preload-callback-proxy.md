# SPEC-004: Legacy Preload Dual-Heap Callback Proxy & `CallbackTable`

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crates:** `crates/aitools-js-bridge`, `crates/tauri-plugin-aitools`

## 1. `window.exports` Dual-Heap Proxy

- **SPEC-004-R1 (CallbackTable Handle Proxy):** Legacy `preload.js` modules executing inside `rquickjs` register `window.exports` features (including `mode: "list"` handlers). `CallbackTable` MUST assign opaque handle IDs and expose proxy dispatch across the QuickJS <-> Tauri IPC boundary without leaking raw Rust or JS pointers.
- **SPEC-004-R2 (Real-Time `callbackSetList` Push):** When a QuickJS list-mode feature invokes `callbackSetList(items)`, the runtime MUST stream list items immediately to the host Webview via `tauri::ipc::Channel` (or subscribed event sink) tagged with the active `trace_id` and `plugin_id`.
- **SPEC-004-R3 (`llrt_modules` Compatible Node Builtin Surface):** `rquickjs` preload contexts MUST provide deterministic shims for `window.exports`, `module.exports`, `process`, and `aitools`/`ztools`/`utools` globals without blocking the main hotkey thread.

## 2. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented in `crates/aitools-js-bridge/src/callback_proxy.rs`, `lazy_worker.rs`, and `crates/tauri-plugin-aitools/src/lib.rs`:
  - `SPEC-004-R1` & `SPEC-004-R2`: `CallbackTable`, `CallbackHandle`, `ListPushSink`, `invoke_preload_list_feature`, and Tauri `Channel<ListPushEvent>` command `invoke_legacy_list_feature` — verified by `spec_004_r1_and_r2_callback_table_proxies_exports_and_streams_set_list`.
  - `SPEC-004-R3`: `PRELOAD_ENV_SHIM` + `JsBridgeHost` off-thread `Send + Sync` worker (`WorkerClient`).
- **Pending Scope:** Extended Node builtin modules (`llrt_modules` fs/crypto/path polyfills) for plugins requiring Node CJS APIs beyond `window.exports`.
