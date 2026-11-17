# SPEC-009: Official CLI, MCP Bridge, Skills Ecosystem & DSH Harness

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crates:** `crates/aitools-cli`, `packages/dsh-ui`

## 1. Official Developer CLI (`aitools`)

- **SPEC-009-R1 (Scaffolding & Lifecycle Commands):** The `aitools` CLI binary MUST provide:
  - `aitools plugin init <name> --template <native-ui|rust-wasm|hybrid|legacy-compat>`
  - `aitools plugin dev`: Watches source files, hot-reloads the plugin inside a running `aitools` host over local IPC, and opens the DSH inspector.
  - `aitools plugin audit`: Validates `aitools.manifest.json`, checks declared permissions against static imports, verifies bundle size, and validates `SKILL.md` frontmatter.
  - `aitools plugin pack --sign <key_path>`: Produces a deterministic `.atp` (or `.ztp`) archive with an Ed25519 signature manifest.
  - `aitools logs export`: Exports chronological logs or diagnostic `.zip` bundles (`SPEC-010`).

## 2. Bidirectional Model Context Protocol (MCP)

- **SPEC-009-R2 (MCP Client Host):** `aitools` reads `<app_data_dir>/mcp_servers.json` and connects to external `stdio` or Streamable HTTP MCP servers, registering discovered tools into `aitools-ai::ToolRegistry` (`SPEC-007-R5`) with default `Tier2DestructiveOrExternal` HITL protection unless overridden by user policy.
- **SPEC-009-R3 (MCP Server Mode):** `aitools mcp serve --transport <stdio|http>` exposes installed `aitools` plugins, pre-focus context readers, and safe OS actions as standard MCP tools so external agents (Claude Code, Gemini CLI, Cursor) can drive `aitools`.

## 3. Dual-Layer `SKILL.md` Integration

- **SPEC-009-R4 (Developer Skills):** Official skill packs under `skills/` document `aitools.manifest.json`, `wit/aitools-plugin.wit`, and uTools-to-`aitools` migration patterns for AI coding assistants.
- **SPEC-009-R5 (Runtime Plugin Skills):** When `aitools-ai` selects a plugin's tool during an agent session and the plugin bundle contains `SKILL.md`, the orchestrator MUST inject that `SKILL.md` body (capped at 4,000 tokens) into the system prompt for that turn.

## 4. Developer & Diagnostic Harness (DSH)

- **SPEC-009-R6 (Context & Fault Simulation):** DSH (`aitools harness` or `Cmd+Shift+D` in dev mode) MUST allow dispatching synthetic `onPluginEnter` contexts (mock text, file lists, bundle IDs), simulating OS permission denials, and inspecting live `aitools-sync://`, QuickJS, and Wasmtime call traces.
- **SPEC-009-R7 (Deterministic `.ztrace` Replay):** DSH MUST record interactive or agent sessions into `.ztrace` JSON files and support headless CI verification via `aitools harness replay <file.ztrace> --assert-no-anomalies`.

## 5. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Core CLI, MCP stdio server, skill packs, and `.ztrace` replay implemented and verified:
  - `SPEC-009-R1`: `aitools plugin init`, `aitools plugin audit`, `aitools plugin pack --out <archive.atp> [--sign <key_path>]`, and `aitools logs export` — verified by `spec_009_r1_init_audit_and_deterministic_pack` (`crates/aitools-cli/src/lib.rs`). (`aitools plugin dev` file watcher & live IPC hot-reload pending desktop host.)
  - `SPEC-009-R3`: `aitools mcp serve --transport stdio` (`handle_mcp_request`) — verified by `spec_009_r3_mcp_server_lists_and_enforces_hitl` (`crates/aitools-cli/src/lib.rs`). (Streamable HTTP transport & external `mcp_servers.json` client host `SPEC-009-R2` pending.)
  - `SPEC-009-R4`: Official skill packs added under `skills/` (`aitools-manifest`, `aitools-wit-plugin`, `utools-to-aitools-migration`).
  - `SPEC-009-R5`: `SkillDocument::parse` & 4,000-token cap — verified by `spec_009_r5_parses_skill_md_and_caps_at_4000_tokens` (`crates/aitools-ai/src/skills.rs`).
  - `SPEC-009-R7`: `replay_ztrace` (`aitools harness replay --assert-no-anomalies`) — verified by `spec_009_r7_ztrace_replay_catches_anomalies` (`crates/aitools-cli/src/lib.rs`). (`packages/dsh-ui` fault-simulation UI `SPEC-009-R6` pending.)
