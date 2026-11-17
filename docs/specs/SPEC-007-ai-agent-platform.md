# SPEC-007: Native AI & Agent-Based Tool Platform

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-ai`

## 1. Unified Dual-Mode Launcher Bar

- **SPEC-007-R1 (Deterministic Fast Path):** Every keystroke in the main search input MUST execute `SPEC-005` hybrid search synchronously first. Deterministic app/command results MUST render within 5 ms without waiting on local or remote model inference.
- **SPEC-007-R2 (Agent Mode Promotion):** Pressing `Tab` (or submitting a query prefixed with `@` or matching no deterministic command above confidence `0.45`) promotes the input bar into an **Agent Session**, attaching pre-focus context (`SPEC-001`) as removable context chips (`selection`, `clipboard`, `active_app`, `active_path`).

## 2. Centralized Model & Credential Broker

- **SPEC-007-R3 (Multi-Provider Routing):** `aitools-ai` MUST support OpenAI-compatible endpoints, Anthropic Messages API, Google Gemini API, and local runtimes (Ollama / llama.cpp) configured via named profiles (`fast`, `reasoning`, `embedding`).
- **SPEC-007-R4 (Shared Plugin AI Proxy):** Plugins calling `aitools.ai.chat()`, `aitools.ai.stream()`, or `aitools.ai.embed()` route through the host broker using user-configured credentials, subject to per-plugin rate limits and token quotas declared in `aitools.manifest.json`.

## 3. Unified Tool Registry & Semantic Routing

- **SPEC-007-R5 (Cross-Tier Tool Unification):** At startup and plugin hot-reload, `ToolRegistry` aggregates tool schemas from:
  1. Built-in OS capabilities (`clipboard`, `files`, `apps`, `window`)
  2. Native v2 plugins (`ai_tools` in `aitools.manifest.json`)
  3. Rust/Wasm plugins (exported via WIT `agent-tool-provider`)
  4. Legacy ZTools/uTools plugins (synthesized from `features[].cmds` + `explain`)
  5. Connected external MCP servers (`SPEC-009`)
- **SPEC-007-R6 (Top-K Semantic Pre-Filtering):** When more than 12 tools are registered, `aitools-ai` MUST rank tools locally via cosine similarity against the user prompt + active context chips and pass only the top 10 tools to the LLM unless the user explicitly mentions `@<plugin>`.

## 4. Human-in-the-Loop (HITL) Risk Tiers

- **SPEC-007-R7 (Three-Tier Execution Gate):** Every tool declaration MUST specify a `RiskTier`:
  - `Tier0ReadOnly`: Auto-approved without UI prompt (e.g., reading notes, searching web/files).
  - `Tier1StateMutating`: Prompts once per agent session with target resource summary (e.g., creating a file, writing plugin DB).
  - `Tier2DestructiveOrExternal`: Requires explicit per-invocation approval showing full arguments/diff before execution (e.g., shell execution, deleting files, sending network messages).

## 5. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Core AI platform primitives implemented in `crates/aitools-ai` and verified:
  - `SPEC-007-R1` & `SPEC-007-R2`: `evaluate_launcher_input` (`LauncherDecision`, `PromotionReason`) — verified by `spec_007_r1_and_r2_fast_path_and_agent_promotion` (`crates/aitools-ai/src/launcher.rs`).
  - `SPEC-007-R3` & `SPEC-007-R4`: `ModelBroker` profile routing & per-plugin quota enforcement (`PluginAiQuotas`) — verified by `spec_007_r3_and_r4_routes_profiles_and_enforces_quotas` (`crates/aitools-ai/src/broker.rs`). Live HTTP streaming transport for external LLM endpoints is planned for runtime integration.
  - `SPEC-007-R5` & `SPEC-007-R6`: `ToolRegistry` 5-source aggregation + local character-trigram cosine similarity top-10 filtering (`SEMANTIC_FILTER_THRESHOLD = 12`, `SEMANTIC_TOP_K = 10`, `@<plugin>` mention bypass) — verified by `spec_007_r5_and_r6_unifies_tiers_and_filters_top_10` (`crates/aitools-ai/src/registry.rs`).
  - `SPEC-007-R7`: `PolicyGuard` 3-tier HITL gate — verified by `spec_007_r7_three_tier_hitl_policy_guard` (`crates/aitools-ai/src/hitl.rs`).
- **Salvaged Harness Primitives (`crates/aitools-ai/src/harness.rs`):** `ErrorObservation`, `DangerousCommandGuard`, `PromptSanitizer`, `prune_tool_output`, and `canonicalize_json`.
