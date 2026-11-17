# SPEC-000: System Architecture Invariants & Governance

- **Status:** Active
- **Version:** 1.0.0
- **Scope:** Workspace-wide (`aitools` core, native hooks, AI orchestrator, plugin hosts, CLI)

## 1. Crate Dependency DAG (Strict Acyclic Enforcement)

Dependencies MUST flow downward strictly in the following order. Circular crate dependencies are forbidden:

1. `aitools-telemetry` (Zero internal workspace dependencies; initialized first in `main()`)
2. `aitools-core` (Depends only on `aitools-telemetry`)
3. `aitools-native` (Depends only on `aitools-telemetry`)
4. `aitools-ai`, `aitools-js-bridge`, `aitools-wasm-host` (Depend on `aitools-core` and `aitools-telemetry`)
5. `tauri-plugin-aitools` (Integrates core, native, AI, JS bridge, and Wasm host into Tauri v2)
6. `src-tauri` & `aitools-cli` (Top-level binary entrypoints)

## 2. Hard Non-Functional Budgets

| Metric | Target (P50) | Hard CI Ceiling (P99) | Measurement Span |
| :--- | :--- | :--- | :--- |
| Global hotkey wake-to-visible (no selection copy) | <= 8 ms | <= 15 ms | `wake_to_paint` |
| Global hotkey wake-to-visible (with selection copy) | <= 25 ms | <= 55 ms | `pre_focus_capture` + `wake_to_paint` |
| Synchronous LMDB read/write (`aitools-sync://`) | <= 0.3 ms | <= 5 ms | `db_sync` |
| QuickJS legacy `preload.js` cold start | <= 2 ms | <= 10 ms | `quickjs_cold_start` |
| Wasm plugin instantiation (`wasm32-wasip2`) | <= 1 ms | <= 5 ms | `wasm_instantiate` |
| Idle resident memory (RSS, no active plugins) | <= 32 MB | <= 45 MB | Process RSS |
| Release desktop installer footprint | <= 14 MB | <= 18 MB | Release artifact |

## 3. Security & Isolation Invariants

- **INV-SEC-01 (Protocol Caller Verification):** Every custom URI scheme (`aitools-plugin://`, `aitools-sync://`) MUST inspect `UriSchemeContext::webview_label()`, extract `plugin-<id>`, and reject cross-plugin or non-plugin callers with HTTP `403`.
- **INV-SEC-02 (Deterministic Preload Injection):** Runtime bridges (`window.aitools`, `window.ztools`, `window.utools`) MUST be injected via `WebviewBuilder::initialization_script` prior to navigation. Post-load `webview.eval()` for API bootstrapping is prohibited.
- **INV-SEC-03 (Credential Isolation):** LLM provider keys and WebDAV secrets MUST reside in the OS Keychain (`keyring`) and MUST NEVER be written to LMDB, JSON configs, or telemetry logs.
- **INV-SEC-04 (HITL Policy Enforcement):** No AI agent tool call may execute without passing `aitools_ai::hitl::PolicyGuard`.

## 4. Test Traceability Convention

Every numbered requirement clause (`SPEC-XXX-RY`) MUST have at least one corresponding automated test containing `spec_xxx_ry` in its function name.

## 5. Implementation & Verification Status (Updated 2026-11-16)

- **Active Workspace Crates (All 9 DAG Crates Compiled & Verified):** `aitools-telemetry`, `aitools-core`, `aitools-native`, `aitools-ai`, `aitools-js-bridge`, `aitools-wasm-host`, `tauri-plugin-aitools`, `aitools-cli`, `src-tauri` (`aitools-desktop`).
- **Security Invariant Traceability:**
  - `INV-SEC-01`: `spec_002_r4_validates_sync_caller_webview_label`, `inv_sec_01_blocks_cross_plugin_and_path_traversal`, `spec_002_r4_aitools_sync_enforces_caller_and_returns_real_rev` (`crates/tauri-plugin-aitools/src/lib.rs`).
  - `INV-SEC-02`: `build_plugin_window` injects `aitools_js_bridge::build_initialization_script` via `WebviewWindowBuilder::initialization_script` before navigation; `spec_008_r1_triple_global_aliasing_is_identical_and_frozen`.
  - `INV-SEC-03`: `spec_001_r5_secret_store_isolates_and_deletes_credentials`.
  - `INV-SEC-04`: `spec_007_r7_three_tier_hitl_policy_guard`, `spec_011_r2_executes_parallel_dag_and_suspends_on_hitl`.
- **Measured Benchmarks (`cargo bench -p aitools-core --bench wake_latency`):**
  - LMDB `get_doc`: `P50 = 0 us`, `P99 = 3 us` (within `SPEC-000` `<= 0.3 ms` P50 / `<= 5 ms` P99 ceiling).
  - 1,200-feature hybrid search: `P50 = 383 us`, `P99 = 1,798 us`.
