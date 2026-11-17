# SPEC-012: Cross-Tier Plugin Adaptation, Legacy Tool Synthesis & SDK Contract

- **Status:** Active
- **Version:** 1.1.0
- **Owner Crates / Packages:** `crates/aitools-core`, `crates/aitools-ai`, `crates/aitools-cli`, `packages/aitools-sdk`

## 1. Unified Adaptation Across Three Plugin Tiers

- **SPEC-012-R1 (Legacy Feature-to-Tool Synthesizer):** For `legacy-compat` plugins (`plugin.json` without `ai_tools`), `aitools-ai` MUST automatically synthesize tool descriptors (`legacy_<plugin_id>_<feature_code>`, sanitized `[a-z0-9_]`, inferred input schemas from `cmds[]`, `3,000 ms` execution timeout).
- **SPEC-012-R2 (Native v2 First-Class SDK):** `native-ui` plugins (`aitools.manifest.json`) MUST use `@aitools/sdk` (`onContextEnter`, `db.get/put/remove/allDocs`, `ai.chat/embed`, `log.info/warn/error` with `x-aitools-trace-id` header forwarding) over typed Tauri v2 commands.
- **SPEC-012-R3 (Zero-Webview Headless Rust/Wasm Execution):** `rust-wasm` plugins (`wasm32-wasip2` / `wasmtime`) executing background indexers or AI tool calls MUST run directly in `aitools-wasm-host` without spawning a Webview.

## 2. Automated Legacy Migration Toolchain (`aitools plugin migrate`)

- **SPEC-012-R4 (Static AST & Manifest Migration):** Running `aitools plugin migrate <legacy_dir> [--out <target_dir>]` MUST:
  1. Parse `plugin.json` (including v1 quirks) and generate a valid `aitools.manifest.json` (Schema v2).
  2. Scan `preload.js` for Node.js built-in imports (`fs`, `child_process`, `crypto`, `path`, `os`, `http`, `electron`) and populate minimal required `permissions`.
  3. Generate stubbed `ai_tools` schemas from `features[]` and create a starter `SKILL.md` file.
  4. Output a `MIGRATION_REPORT.md` flagging any incompatible native C++ `.node` addons.

## 3. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented and verified across SDK, host IPC, Wasm host, CLI migration, and 3 benchmark plugins:
  - `SPEC-012-R2`: `packages/aitools-sdk/src/index.ts` verified via `tsc -p packages/aitools-sdk/tsconfig.json --noEmit` (0 errors); host commands (`db_get`, `db_put`, `db_remove`, `db_all_docs`, `ai_chat`, `ai_embed`, `telemetry_log`) implemented in `crates/tauri-plugin-aitools/src/lib.rs` + `permissions/default.toml` and verified by `spec_012_r2_sdk_commands_enforce_plugin_isolation_and_persist_docs`.
  - `SPEC-012-R3`: `wit/aitools-plugin.wit` (`0.2.0` `aitools-extension`) + `examples/plugins/aitools-plugin-devtools/plugin.wasm` verified by `spec_012_r3_instantiates_devtools_benchmark_wasm_plugin` (`crates/aitools-wasm-host/src/lib.rs`).
  - `SPEC-012-R4`: `migrate_legacy_plugin` + `MIGRATION_REPORT.md` (`.node` native addon detection) verified by `spec_012_r2_migrates_legacy_plugin_and_detects_node_deps`; all 3 benchmark plugins (`aitools-plugin-devtools`, `aitools-plugin-smart-clip`, `legacy-color-sample`) verified by `spec_012_benchmark_plugins_pass_audit` (`crates/aitools-cli/src/lib.rs`).
