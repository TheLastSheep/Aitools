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

## 3. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Tier 1 JS bridge & Tier 3 Wasmtime host implemented and verified:
  - `SPEC-008-R1`: `TRIPLE_GLOBAL_SHIM_JS` / `build_triple_global_shim` (`window.aitools === window.ztools === window.utools`, `Object.freeze`) — verified in `rquickjs` by `spec_008_r1_triple_global_aliasing_is_identical_and_frozen` (`crates/aitools-js-bridge/src/lazy_worker.rs`).
  - `SPEC-008-R2`: `JsBridgeHost::activate_legacy_plugin` — verified by `spec_008_r2_skips_quickjs_without_preload` (`crates/aitools-js-bridge/src/lazy_worker.rs`).
  - `SPEC-008-R3`: `WasmPluginHost` (`wasmtime`, 64 MB `StoreLimits`, fuel & epoch limits) and `wit/aitools-plugin.wit` — verified by `spec_008_r3_enforces_64mb_memory_and_fuel_caps` (`crates/aitools-wasm-host/src/lib.rs`).
  - `SPEC-008-R4`: `ManifestV2` schema (`wasm_entry`, `ai_quotas`, `features`, `skill_file`) in `crates/aitools-core/src/models.rs`.
- **Completed Since Initial Draft:** Tauri child webview registration helper `build_plugin_window` with `WebviewWindowBuilder::initialization_script` added in `crates/tauri-plugin-aitools/src/lib.rs`, plus 8 core built-in plugins under `plugins/builtin/` (`clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`, `json-codec`, `timestamp-calc`, `snipaste-legacy`, `todo-legacy`) verified by `spec_008_builtin_plugins_pass_audit_and_classification` (`crates/aitools-cli/src/lib.rs`) and `spec_012_r3_executes_builtin_wasm_plugins` (`crates/aitools-wasm-host/src/lib.rs`).
- **Pending Scope:** Full WIT host-capability component bindings for live Wasm component invocations.
