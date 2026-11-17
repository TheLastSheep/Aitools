# SPEC-013: Bidirectional MCP Gateway, Runtime Skill Injection & DSH Workbench

- **Status:** Active
- **Version:** 1.1.0
- **Owner Crates / Packages:** `crates/aitools-cli`, `crates/aitools-ai`, `crates/tauri-plugin-aitools`, `packages/renderer-svelte`

## 1. Bidirectional MCP Gateway

- **SPEC-013-R1 (Outbound MCP Server Exposure):** `aitools mcp serve` MUST implement the Model Context Protocol specification (`2025-03-26` / `2025-11-25` JSON-RPC 2.0) over `stdio` and local loopback Streamable HTTP (`127.0.0.1`), exposing `tools/list`, `tools/call` (with `canonicalize_json` + `ErrorObservation`), `resources/list`, and `resources/read` (`aitools://harness`, `aitools://context/current`, `aitools://telemetry/anomalies`).
- **SPEC-013-R2 (Inbound MCP Client Lifecycle):** External MCP servers configured in `<app_data_dir>/mcp_servers.json` MUST enforce a `10,000 ms` per-call timeout and register discovered tools into `ToolRegistry` with default `Tier2DestructiveOrExternal` `PolicyGuard` gating.

## 2. Dynamic `SKILL.md` Context Injection

- **SPEC-013-R3 (Lazy Skill Loading):** Plugin `SKILL.md` bodies MUST ONLY be injected into the system prompt when `ToolRegistry::select_for_prompt` selects a tool belonging to that `plugin_id`, strictly capped at `4,000 tokens` (`16 KiB`).

## 3. DSH Fault Injection & Golden Replay

- **SPEC-013-R4 (Deterministic Fault Injector):** `FaultInjectionConfig` MUST allow DSH to deterministically inject LMDB `_rev` conflicts, synthetic IPC latency, permission denials, Wasm fuel exhaustion, and LLM stream interruptions without affecting production when disabled.
- **SPEC-013-R5 (Golden `.ztrace` Recording & CI Assertion):** `.ztrace` recordings MUST replay deterministically via `aitools harness replay <file.ztrace> --assert-no-anomalies`, exiting non-zero on behavioral or latency regressions.

## 4. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented and verified (including golden `.ztrace` suite and CI gate):
  - `SPEC-013-R1`: `handle_mcp_request` (`crates/aitools-cli/src/lib.rs`) — verified by `spec_009_r3_mcp_server_lists_and_enforces_hitl`.
  - `SPEC-013-R2`: `McpServersFile` & `register_mcp_tools` (`crates/aitools-ai/src/mcp_client.rs`) — verified by `spec_013_r1_parses_mcp_servers_json_and_registers_tier2_tools`.
  - `SPEC-013-R3`: `SkillDocument::render_for_system_prompt` — verified by `spec_009_r5_parses_skill_md_and_caps_at_4000_tokens`.
  - `SPEC-013-R4`: `FaultInjectionConfig` + `dsh_set_fault_injection` — LMDB `_rev` conflict verified by `spec_002_r4_aitools_sync_enforces_caller_and_returns_real_rev`.
  - `SPEC-013-R5`: `replay_ztrace` + 5 golden fixtures under `tests/traces/*.ztrace` + `.github/workflows/sdd-gate.yml` — verified by `spec_009_r7_ztrace_replay_catches_anomalies` and `spec_013_r5_replays_all_golden_ztrace_fixtures` (`crates/aitools-cli/src/lib.rs`).
