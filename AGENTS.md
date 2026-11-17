# AGENTS.md — aitools Engineering & SDD Constitution

## 1. Project Identity
`aitools` is an AI-native, cross-platform desktop launcher and agentic tool platform built on Rust and Tauri v2, maintaining full backward compatibility with ZTools and uTools plugins.

## 2. Spec-Driven Development (SDD) Laws
1. **Spec Before Code:** Never alter runtime behavior, IPC schemas, manifest fields, or storage layouts without first updating the corresponding `docs/specs/SPEC-*.md` file in the same commit.
2. **Spec After Acceptance:** After each task is completed and accepted, update the corresponding `docs/specs/SPEC-*.md` file to record implementation status, verified `spec_<num>_<behavior>` test traceability, measured benchmark results, and any accepted scope deltas.
3. **Traceability:** Every new requirement clause in `SPEC-*` must link to a test function named `spec_<num>_<behavior>` in `crates/*/tests/` or `crates/*/src/`.
4. **Hard Performance & Footprint Budgets:**
   - Global hotkey wake-to-visible latency: P50 <= 8 ms, P99 <= 15 ms (<= 55 ms with selection copy).
   - Idle resident memory (RSS): <= 45 MB (excluding optional local embedding models).
   - Release installer size: <= 18 MB.
5. **Security & Isolation Invariants:**
   - Custom URI protocols (`aitools-plugin://`, `aitools-sync://`) MUST validate `UriSchemeContext::webview_label()` on every request.
   - Preload bootstrap scripts MUST be injected via `WebviewBuilder::initialization_script` before navigation—never via post-load `webview.eval()`.
   - All agent tool executions MUST pass through `aitools_ai::hitl::PolicyGuard` according to their declared risk tier (`Tier0ReadOnly`, `Tier1Mutating`, `Tier2Destructive`).
6. **Chronological Observability:** Every cross-boundary operation (hotkey wake, context capture, DB sync, AI tool call, plugin IPC) must emit a `tracing` span carrying `trace_id` and `plugin_id` into `aitools-telemetry`.

## 3. Build & Verification Commands
- Run workspace tests: `cargo test --workspace`
- Run telemetry tests: `cargo test -p aitools-telemetry`
- Run parity suite: `cargo test -p aitools-core --test parity`
- Run strict lints: `cargo clippy --workspace --all-targets -- -D warnings`
- Audit latency budgets: `cargo bench -p aitools-core --bench wake_latency`

## 4. Specification Index (`docs/specs/`)
- `SPEC-000` — Architecture invariants, crate DAG, latency/footprint budgets, security laws
- `SPEC-001` — Native hooks, double-tap detector, bounded app catalog, pre-focus context, keychain
- `SPEC-002` — LMDB (`heed`) storage engine, PouchDB `_rev` sync, `INV-SEC-01` caller verification
- `SPEC-003` — Unified v1 (`plugin.json`) & v2 (`aitools.manifest.json`) manifests and ZTools quirks
- `SPEC-004` — Legacy `rquickjs` `CallbackTable` proxy & real-time `callbackSetList` channel
- `SPEC-005` — Hybrid search, 20,902+ CJK Pinyin, JS regex parity & media rules
- `SPEC-006` — Desktop host shell, least-privilege capabilities, wake channel & Svelte 5 renderer
- `SPEC-007` — Dual-mode launcher promotion, model broker, semantic Top-K routing & 3-tier HITL
- `SPEC-008` — Three-tier plugin taxonomy (`legacy-compat`, `native-ui`, `rust-wasm`)
- `SPEC-009` — Official `aitools` CLI, MCP bridge, dual-layer `SKILL.md` & `.ztrace` replay
- `SPEC-010` — Chronological JSONL observability, secret redaction, panic flush & `.zip` bundles
- `SPEC-011` — Ambient `ContextEnvelope`, zero-query recommendations, bounded DAG orchestrator & memory
- `SPEC-012` — Cross-tier plugin synthesis, `@aitools/sdk` & `aitools plugin migrate`
- `SPEC-013` — Bidirectional MCP gateway, lazy `SKILL.md` injection & DSH fault injector
- `SPEC-014` — End-to-end `trace_id` propagation, `aitools doctor` profiling & `import-diag` loop
