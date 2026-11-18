# SPEC-008: Three-Tier Plugin Taxonomy & Runtime Adaptation

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crates:** `crates/aitools-core`, `crates/aitools-js-bridge`, `crates/aitools-wasm-host`, `crates/tauri-plugin-aitools`

## 1. Plugin Runtime Classification

`aitools` classifies every installed plugin into one of three explicit runtime tiers at load time:

| Tier | Identifier | Manifest File | Execution Runtime | API Surface |
| :--- | :--- | :--- | :--- | :--- |
| **Tier 1** | `legacy-compat` | `plugin.json` (v1) | Child Webview (`aitools-plugin://`) + `aitools-sync://` + lazy `rquickjs` (`llrt_modules`) | Synchronous `window.aitools` / `window.ztools` / `window.utools` + `window.exports` RPC |
| **Tier 2** | `native-ui` | `aitools.manifest.json` (v2) | Child Webview + async Tauri IPC & `Channel` streams | `@aitools/sdk` (`aitools.ai.*`, `aitools.db.*`, `aitools.events.*`) |
| **Tier 3** | `rust-wasm` / `rust-native` | `aitools.manifest.json` (v2) + `.wasm` | Sandboxed `wasm32-wasip2` (`wasmtime`) with fuel/memory caps (or signed `cdylib` for OS plugins) | Versioned WIT contract (`wit/aitools-plugin.wit`) |

## 2. Behavioral Requirements

- **SPEC-008-R1 (Triple-Global Aliasing):** In `legacy-compat` webviews, `window.aitools`, `window.ztools`, and `window.utools` MUST resolve to the exact same frozen shim object so unmodified ZTools and uTools plugins execute identically.
- **SPEC-008-R2 (Lazy QuickJS Activation):** A `legacy-compat` plugin without a `"preload"` field in `plugin.json` MUST NOT spawn an `rquickjs` worker.
- **SPEC-008-R3 (Wasm Resource Quotas):** Every `rust-wasm` plugin instance MUST execute inside a `wasmtime::Store` capped at 64 MB linear memory and configurable epoch/fuel execution timeouts (default 5,000 ms per tool call) to prevent runaway plugins from stalling the host.
- **SPEC-008-R4 (Manifest v2 Schema):** `aitools.manifest.json` MUST declare `id`, `name`, `version`, `runtime` (`native-ui` | `rust-wasm` | `hybrid`), `permissions` (explicit capability allowlist), `ai_tools` (JSON Schema tool definitions), and optional `skill_file` (default `SKILL.md`).

## 3. Implementation & Verification Status (Updated 2026-11-18)

- **Status:** Tier 1 JS bridge, 3-tier ZTools zero-mod preload runtime, and Tier 3 Wasmtime host implemented and Wave 3 verified:
  - `SPEC-008-R1`: `TRIPLE_GLOBAL_SHIM_JS` / `build_triple_global_shim` (`window.aitools === window.ztools === window.utools`, `Object.freeze`) — verified in `rquickjs` by `spec_008_r1_triple_global_aliasing_is_identical_and_frozen` (`crates/aitools-js-bridge/src/lazy_worker.rs`).
  - `SPEC-008-R2`: `JsBridgeHost::activate_legacy_plugin` and 3-tier `PreloadRuntime` (`QuickJsNodeCompat` primary + `SystemNodeFallback` for compat gaps) — verified by `spec_008_r2_skips_quickjs_without_preload`, `spec_015_r2_pure_rust_quickjs_node_compat_runs_real_ztools_without_node`, and `spec_015_r2_node_compat_gap_fallback_and_pure_rust_only_mode` (`crates/aitools-js-bridge`).
  - `SPEC-008-R3`: `WasmPluginHost` (`wasmtime`, 64 MB `StoreLimits`, fuel & epoch limits) and `wit/aitools-plugin.wit` — verified by `spec_008_r3_enforces_64mb_memory_and_fuel_caps` (`crates/aitools-wasm-host/src/lib.rs`).
  - `SPEC-008-R4`: `ManifestV2` schema (`wasm_entry`, `ai_quotas`, `features`, `skill_file`) in `crates/aitools-core/src/models.rs`.
- **Completed & Verified Built-ins / WebView Wiring:** Tauri child webview registration helper `build_plugin_window` with `WebviewWindowBuilder::initialization_script` in `crates/tauri-plugin-aitools/src/lib.rs`, plus all 8 core built-in plugins under `plugins/builtin/` (`clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`, `json-codec`, `timestamp-calc`, `snipaste-legacy`, `todo-legacy`) verified by `spec_008_builtin_plugins_pass_audit_and_classification`, `spec_017_r5_builtin_plugins_csp_clean_styled_and_auditable` (`crates/aitools-cli/src/lib.rs`), `spec_012_r3_executes_builtin_wasm_plugins` (`crates/aitools-wasm-host/src/lib.rs`), and `aitools plugin audit` (`skill_valid: true`, `warnings: []` across all 8).
- **Verified By Wave 3 (`@verifier`, 2026-11-18):** `cargo test --workspace` (81/81 passed, 0 failed, 0 ignored) and `AITOOLS_PURE_RUST_PRELOAD=1 cargo test -p aitools-js-bridge` (5 unit + 8 integration passed).
- **Pending Scope:** Full WIT host-capability component bindings for live Wasm component invocations beyond current Wasmtime host checks.
