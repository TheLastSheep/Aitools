# SPEC-015: ZTools Zero-Modification Runtime, 3-Tier Rust-First Preload & Bridge Parity

- **Status:** Active
- **Version:** 2.0.0
- **Owner Crates:** `crates/aitools-core`, `crates/aitools-js-bridge`, `crates/tauri-plugin-aitools`
- **Extends:** `SPEC-003` (Manifests), `SPEC-004` (Callback Proxy), `SPEC-008` (Tier 1 `legacy-compat`)

## 1. Zero-Modification Upstream Invariant (`INV-ZTOOLS-IMMUTABLE`)

- **SPEC-015-R1 (`INV-ZTOOLS-IMMUTABLE` Upstream Read-Only Guarantee):** Loading, parsing, indexing, bundling, serving (`aitools-plugin://`), or executing (`QuickJsNodeCompat`, `SystemNodeFallback`, or pure-JS fallback) any upstream ZTools or uTools plugin directory (including `ZTools-plugins/*`, `plugin.json`, `main` HTML/JS/CSS assets, `preload.js`, `window.exports` `none`/`list`/`doc` definitions, `logo`, and `node_modules`) MUST NOT create, modify, rename, chmod, or delete any file or directory inside the plugin root.
  - SHA-256 digests of every file in the plugin directory before and after load, search, enter, select, and unload MUST remain byte-for-byte identical.
  - All host runtime artifacts (LMDB databases, logs, session metadata, fallback runner scripts, and temporary staging files) MUST reside outside the plugin directory (under `<app_data_dir>` or process-private temp paths cleaned on `Drop`).
  - Manifest normalization MUST preserve non-ASCII / Unicode plugin identifiers deterministically (`sanitize_plugin_id` MUST derive a deterministic collision-free identifier such as `plugin-<short-sha256>` when ASCII folding yields empty, rather than collapsing distinct Unicode names to `"unnamed-plugin"`), and MUST preserve external `http://` or `https://` `main` URLs verbatim instead of rewriting them to `aitools-plugin://`.
  - Development overrides (`development.main`, `development.preload`) and `pluginSetting` (`single`, `height`) MUST be honored read-only without rewriting `plugin.json`.

## 2. 3-Tier Rust-First Preload & Node-Elimination Architecture

`aitools` targets progressive elimination of external Node.js while maintaining zero-modification compatibility with ZTools `preload.js` plugins through a strict three-tier hierarchy (`PreloadTier`) controlled by `PreloadExecutionMode`:

- **Preload Tiers (`PreloadTier`):**
  1. `WebViewOnly` — Plugins without `"preload"` (or `development.preload`) spawn zero preload workers (`SPEC-008-R2`).
  2. `QuickJsNodeCompat` (Primary) — In-process pure-Rust `rquickjs` (`0.8`) + Rust CommonJS loader & capability-scoped Node/Electron compatibility layer (`<= 64 MiB` QuickJS heap ceiling accommodating large CommonJS bundles such as `xlsx.js`, `<= 2 ms` P50 / `<= 10 ms` P99 `quickjs_cold_start` per `SPEC-000`).
  3. `SystemNodeFallback` (Escape Hatch) — Out-of-process host `node` CommonJS sidecar (`NodePreloadRuntime`), NEVER bundled with `aitools` and invoked ONLY on explicit `JsBridgeError::NodeCompatGap` under `RustFirstWithFallback`.
- **Execution Modes (`PreloadExecutionMode`):**
  - `PureRustOnly` — Strictly forbids spawning external `node`; returns `Err(JsBridgeError::NodeCompatGap { plugin_id, capability })` if a plugin requires an unimplemented native addon or V8 internal. Activated by `AITOOLS_PURE_RUST_PRELOAD=1` or explicit host configuration.
  - `RustFirstWithFallback` (Default) — ALWAYS executes Tier 2 `QuickJsNodeCompat` first (even when host `node` is installed), falling back to Tier 3 `SystemNodeFallback` ONLY when `QuickJsNodeCompat` raises `JsBridgeError::NodeCompatGap`.

- **SPEC-015-R2 (Tier 2 Primary In-Process `QuickJsNodeCompat` Runtime):** For any `legacy-compat` plugin declaring `"preload"` (or `development.preload`), `PreloadRuntime` MUST execute `QuickJsNodeCompat` (`crates/aitools-js-bridge/src/rust_node_compat.rs`, built on workspace `rquickjs = 0.8` without `llrt_modules` and adhering to `SPEC-000` crate DAG `aitools-core` + `aitools-telemetry` only) FIRST:
  1. **Non-Reentrant Native Host Boundary:** Rust native primitives (`__aitools_native_node_op(op, json_args)`) and host bridge calls (`__aitools_host_bridge_op(domain, json_args)`) exposed into `rquickjs::Context` MUST return plain Rust/JSON strings synchronously back to JS without recursively calling `ctx.eval` inside host callbacks.
  2. **CommonJS Module System:** Provide `require`, `require.resolve`, `require.cache`, `require('module')` (`builtinModules`, `createRequire`, `_load`, `_initPaths`, `_nodeModulePaths`, `globalPaths`), and the standard `(function(exports, require, module, __filename, __dirname) { ... })` module wrapper, resolving:
     - Read-only relative files (`./*`, `../*` with `.js`, `.cjs`, `.json`, `/index.js`, `/index.cjs`);
     - `<plugin_dir>/node_modules/<pkg>` (walking parent `node_modules` up to `<plugin_dir>`) and `Module.globalPaths` / `NODE_PATH` entries strictly bounded inside `<plugin_dir>` using `package.json` `main` / `exports` fields without mutating `node_modules`.
  3. **Rust-Backed Node & Electron Built-ins (`node:*` and bare specifiers):**
     - `electron`: `clipboard`, `nativeImage`, `shell`, `ipcRenderer`, `screen`, `dialog` routed through `HostBridgeHandler`;
     - `fs`, `fs/promises`, `original-fs`: capability-gated by `PreloadCapabilities::allow_fs` (`INV-ZTOOLS-IMMUTABLE` enforced on plugin bundles);
     - `path`, `path/posix`, `path/win32`, `os` (including cross-platform `os.networkInterfaces()` enumerating all active IPv4/IPv6 interfaces via `ifconfig` on macOS/BSD, `ip -o addr` on Linux, and UDP probe fallback);
     - `crypto`: SHA-256, SHA-512, MD5, SHA-1, HMAC, `randomUUID`, `randomBytes`;
     - `buffer` and global `Buffer`;
     - `url`, `querystring`, `util`, `events`, `stream`, `timers`, `readline`, `zlib` (via `flate2`), `assert`, `string_decoder`, `process`;
     - `child_process`: strictly capability-gated by `PreloadCapabilities::allow_child_process`;
     - `http`, `https`, and global `fetch`: strictly capability-gated by `PreloadCapabilities::allow_net` and routed through `HostBridgeHandler::http_op`.
  4. **Preload Globals & Browser Shims:** Install `globalThis.window = globalThis`, non-crashing DOM stubs (`document`, `HTMLElement`, `navigator`, `location`, `localStorage`), and frozen `ztools === utools === aitools === rubick` globals prior to evaluating `preload.js`.

- **SPEC-015-R2b (Tier 3 `SystemNodeFallback` & `NodeCompatGap` Burn-Down Telemetry):**
  1. **Explicit `NodeCompatGap` Trapping:** When `QuickJsNodeCompat` encounters a `.node` native N-API addon or an unimplemented Node/V8 internal (`worker_threads`, `vm`, `dgram`, `tls`, `inspector`, or unsupported native package), `require()` MUST throw `__AITOOLS_NODE_COMPAT_GAP__:<specifier>`, mapped to `JsBridgeError::NodeCompatGap { plugin_id, capability }`.
  2. **Mode-Governed Behavior:**
     - Under `PreloadExecutionMode::PureRustOnly`, `PreloadRuntime::load` / execution MUST fail fast with `Err(JsBridgeError::NodeCompatGap { ... })` and MUST NOT probe or spawn `node`.
     - Under `PreloadExecutionMode::RustFirstWithFallback`, `PreloadRuntime` MUST emit a structured warning (`tracing::warn!(target: "aitools::node_compat_gap", plugin_id = %plugin_id, capability = %capability, ...)`) for burn-down telemetry (`SPEC-016-R4`) and transparently re-load the plugin inside Tier 3 `SystemNodeFallback` (`NodePreloadRuntime`).
  3. **`NodePreloadRuntime` Fallback Sidecar (`crates/aitools-js-bridge/src/node_runtime.rs`):** When triggered, spawns host `node` (never bundled) with a host-owned runner script outside the plugin directory, `current_dir = plugin_dir`, `NODE_PATH` including `<plugin_dir>/node_modules`, `global.window = global`, DOM stubs, `ztools`/`utools`/`aitools`/`rubick` globals, `Module._load` `electron` facade, line-delimited JSON (NDJSON) over stdio, bounded per-call timeouts, and deterministic termination on `shutdown()` / `Drop`.
  4. **Legacy QuickJS Preservation:** Existing `LegacyPreloadRuntime` (`JsBridgeHost`), `CallbackTable`, and `invoke_preload_list_feature` APIs remain preserved for direct `SPEC-004` callers.

- **SPEC-015-R3 (`PreloadRuntime` & `HostBridgeHandler` Bidirectional Contract):** `PreloadRuntime` MUST expose `active_tier(&self) -> PreloadTier` and coordinate host capabilities via `PreloadCapabilities { allow_fs, allow_net, allow_child_process }` and a unified `HostBridgeHandler` trait (`Send + Sync + 'static`):
  1. **Host RPC Domains:** `callback_set_list`, `db_op`, `clipboard_op`, `notification_op`, `window_op`, `shell_op`, `system_op`, `features_op`, `dialog_op`, `ai_op`, `input_op`, and `http_op`.
  2. **Preload Lifecycle Methods:**
     - `load(preload_path, plugin_id, plugin_dir) -> Result<PreloadLoadInfo>` (captures `window.exports`, exported `window.*` props, and lifecycle hooks);
     - `enter(feature_code, action)` (dispatches `window.exports[feature_code]` `mode: "list" | "none" | "doc"` and/or `onPluginEnter` / `onPluginReady`);
     - `search(feature_code, action, query)` (invokes list-mode `args.search` and pushes `ListItem[]` via `HostBridgeHandler::callback_set_list`);
     - `select(feature_code, action, item)` (invokes list-mode `args.select`, returning action directives including `url` or `outPlugin`);
     - `main_push(feature_code, action)` (executes `onMainPush` callbacks);
     - `invoke_window_prop(prop_path, args)` (invokes functions or reads serializable values attached to `window.*` by `preload.js`, e.g., `window.charTransformApi.transform(...)`, `window.services.readFile(...)`, `window.__nativeCopy(...)`);
     - `get_exported_props()` (enumerates `ExportedWindowProp` entries for WebView proxy binding);
     - `plugin_out(is_kill)` and `shutdown()`.

## 3. WebView Triple-Global Shim & `aitools-sync://<plugin_id>/<op>` Contract

- **SPEC-015-R4 (WebView Synchronous Transport & `preload/invoke` Proxy):** `TRIPLE_GLOBAL_SHIM_JS` (`build_initialization_script` / `build_initialization_script_with_preload_props`, injected via `WebviewWindowBuilder::initialization_script` prior to navigation per `INV-SEC-02`) MUST bind `window.aitools === window.ztools === window.utools === window.rubick` and:
  1. Implement `globalThis.__aitools_sync_call__(op, payload)` inside the WebView using synchronous `XMLHttpRequest` `POST` to `aitools-sync://<plugin_id>/<op>` (resolving `<plugin_id>` from `globalThis.__AITOOLS_PLUGIN_ID__` or `location.host`), returning parsed JSON `data` on HTTP `2xx` / `{ ok: true }` or throwing a descriptive `Error` on `{ ok: false }` / non-`2xx`.
  2. Install dynamic WebView proxies (`window.<preloadProp>`) routing UI calls into `aitools-sync://<plugin_id>/preload/invoke` (`{ propPath, args }`) and expose `globalThis.__aitools_dispatch_lifecycle__` (`pluginReady`, `pluginEnter`, `pluginOut`, `pluginDetach`, `subInput`, `themeChange`) so UI pages and backend `PreloadRuntime` stay synchronized across the WebView isolation boundary.

- **SPEC-015-R5 (Complete PouchDB Storage, Attachments, Promises & `dbStorage` Parity):** Both `aitools-sync://<plugin_id>/db/*` (`crates/tauri-plugin-aitools/src/protocols.rs`) and `TRIPLE_GLOBAL_SHIM_JS` MUST implement:
  - `db/put`: persists `Document` (`_id`, `_rev`, arbitrary fields) with optimistic `_rev` conflict detection (`SPEC-002-R1`) and `256 KiB` size caps (`SPEC-002-R2`).
  - `db/get`: accepts `{ id }`, returning `Document | null`.
  - `db/remove`: accepts **both** string document IDs (`"doc-1"` -> `{ id }`) and `{ _id, _rev }` document objects (`{ doc }`).
  - `db/allDocs`: accepts omitted/empty argument (all docs), string prefix (`"todo:"` -> `{ key }`), or `string[]` (`{ keys }`).
  - `db/bulkDocs`: atomic batch write (`docs[]`) enforcing duplicate-ID rejection and response size caps (`SPEC-002-R2`).
  - `db/putAttachment`, `db/getAttachment`, `db/getAttachmentType`, `db/removeAttachment`: per-plugin isolated binary attachments (`256 KiB` cap).
  - `db.promises.*`: Promise-returning wrappers (`get`, `put`, `remove`, `bulkDocs`, `allDocs`, `putAttachment`, `getAttachment`, `getAttachmentType`, `removeAttachment`).
  - `dbStorage` (`setItem`, `getItem`, `removeItem`) and `dbCryptoStorage` (`setItem`, `getItem`, `removeItem`).

- **SPEC-015-R6 (Complete ZTools Lifecycle, SubInput, OS, Display & AI Parity):** `TRIPLE_GLOBAL_SHIM_JS`, `HostBridgeHandler`, and `aitools-sync://<plugin_id>/<op>` MUST implement:
  - **Lifecycle & SubInput:** `onPluginEnter`, `onPluginReady`, `onPluginOut`, `onPluginDetach`, `onPluginAttach`, `onMainPush`, `setSubInput(onChange, placeholder?, isFocus?)`, `setSubInputValue(text)`, `removeSubInput()`, `subInputFocus()`, `subInputBlur()`, `subInputSelect()`, `redirect(label, payload)`, `outPlugin(isKill?)`, `hideMainWindow(isRestore?)` (returning a resolved `Promise<boolean>`), `hideMainWindowTypeString(text)`, `showMainWindow()`, `setExpendHeight(height)`.
  - **Clipboard & Paths:** `clipboard/copyText`, `clipboard/copyImage`, `clipboard/copyFile`, `clipboard/getCopyedFiles`, `clipboard/readText`, `clipboard/readImage`, `system/getPath`, `system/getPathForFile`, `system/getFileIcon`, `system/readCurrentFolderPath`, `system/readCurrentBrowserUrl`.
  - **System, Shell & Notifications:** `system/getAppName`, `system/getAppVersion`, `system/getNativeId`, `isDev`, `isMacOs` (`isMacOS`), `isWindows`, `isLinux`, `system/isDarkColors`, `system/getThemeInfo`, `shell: { openExternal, openPath, showItemInFolder, beep }` (mutable namespace object attached prior to `Object.freeze`), `shell/openExternal` (`shellOpenExternal`), `shell/openPath` (`shellOpenPath`), `shell/showItemInFolder` (`shellShowItemInFolder`), `shell/beep` (`shellBeep`), `notification/show` (`showNotification`).
  - **Screen, Dialogs & Input Simulation:** `system/screenCapture`, `dialog/showOpenDialog`, `dialog/showSaveDialog`, `system/simulateKeyboardTap`, `system/simulateMouseMove`, `system/simulateMouseClick`, `display/getPrimaryDisplay`, `display/getAllDisplays`, `display/getDisplayNearestPoint`, `display/getCursorScreenPoint`, `display/dipToScreenPoint`, `display/screenToDipPoint`.
  - **Dynamic Features:** `features/getFeatures`, `features/setFeature`, `features/removeFeature`, `features/registerMainPush`.
  - **ZTools AI Parity:** `window.ztools.ai(options, streamCallback?)` returning a Promise-compatible request handle with `.abort()` (`ai/chat`, `ai/abort`), and `window.ztools.allAiModels()` (`ai/allModels`), routed through the host `ModelBroker` (`SPEC-016-R6`, `SPEC-007-R4`).
  - Every `aitools-sync://<plugin_id>/<op>` request MUST enforce `INV-SEC-01` (`validate_sync_caller`) and manifest permissions (`SPEC-016-R4`).

## 4. Clause-to-Test Traceability

| Clause | Requirement Summary | Linked Test (`crates/*/src/` or `crates/*/tests/`) | Owner Crate |
| :--- | :--- | :--- | :--- |
| `SPEC-015-R1` | `INV-ZTOOLS-IMMUTABLE` zero-mod load/SHA-256 invariance, Unicode IDs & HTTP(S) `main` preservation | `spec_015_r1_zero_mod_upstream_sha256_unchanged`, `spec_015_r1_loads_unmodified_ztools_plugin_dir` | `aitools-js-bridge`, `aitools-core` |
| `SPEC-015-R2` | Primary Tier 2 `QuickJsNodeCompat` CommonJS/Node builtins runs real ZTools plugins without `node` (`< 10 ms` cold start) & Node sidecar env parity | `spec_015_r2_pure_rust_quickjs_node_compat_runs_real_ztools_without_node`, `spec_015_r2_node_preload_spawns_commonjs_with_electron_and_globals` | `aitools-js-bridge` |
| `SPEC-015-R2b` | `NodeCompatGap` trapping, `PureRustOnly` strict fail-fast, Tier 3 `SystemNodeFallback` (`NodePreloadRuntime`) & `doctor` gap reporting | `spec_015_r2_node_compat_gap_fallback_and_pure_rust_only_mode`, `spec_015_r2b_doctor_reports_preload_tier_and_node_compat_gaps` | `aitools-js-bridge`, `aitools-cli` |
| `SPEC-015-R3` | `PreloadRuntime` & `HostBridgeHandler` RPC (`enter`/`search`/`select`/`main_push`/`invoke_window_prop`/`out`/`http_op`) | `spec_015_r3_node_preload_list_mode_enter_search_select_and_invoke` | `aitools-js-bridge` |
| `SPEC-015-R4` | WebView `__aitools_sync_call__` synchronous XHR & `preload/invoke` dynamic `window.*` proxy | `spec_015_r4_webview_shim_sync_call_and_preload_proxy` | `aitools-js-bridge`, `tauri-plugin-aitools` |
| `SPEC-015-R5` | `db/*` (`remove(id\|doc)`, `allDocs(prefix\|keys[])`, `bulkDocs`, attachments, `db.promises`, `dbStorage`) | `spec_015_r5_sync_protocol_db_all_ops_attachments_and_promises` | `aitools-js-bridge`, `tauri-plugin-aitools` |
| `SPEC-015-R6` | Full ZTools lifecycle, SubInput, clipboard, shell, dialog, display, input sim, features & `ai()`/`allAiModels()` | `spec_015_r6_ztools_lifecycle_os_clipboard_shell_window_and_ai_apis` | `aitools-js-bridge`, `tauri-plugin-aitools` |

## 5. Implementation & Verification Status

- **Status:** Verified (v2.0 3-Tier Rust-first ZTools zero-mod runtime & sync bridge implemented and verified)
- **Verified By Wave 3 (`@verifier`, 2026-11-18):**
  - `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` -> exit 0; `0` `.unwrap(` calls outside tests/`main.rs`.
  - `cargo test --workspace` -> 81/81 passed, 0 failed, 0 ignored (`aitools-js-bridge`: 5 unit + 8 `spec_015_ztools_parity`; `aitools-core`: 8 unit + 4 `parity`; `tauri-plugin-aitools`: 9; `aitools-cli`: 9).
  - `AITOOLS_PURE_RUST_PRELOAD=1 cargo test -p aitools-js-bridge` -> 5 unit + 8 integration passed (`PreloadExecutionMode::PureRustOnly` with nonexistent Node binary confirms zero-Node execution for supported ZTools plugins + SHA-256 fixture tree invariance + `NodeCompatGap` fallback behavior).
  - `cargo run -q -p aitools-cli -- doctor --log-dir <dir>` -> exit 0 (exposes `preload_tier_counts`, `node_compat_gap_count`, `node_compat_gaps`).
