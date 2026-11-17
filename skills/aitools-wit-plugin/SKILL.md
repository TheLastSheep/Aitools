---
name: aitools-wit-plugin
description: Build sandboxed wasm32-wasip2 plugins implementing wit/aitools-plugin.wit for aitools.
---

# Rust/Wasm Plugin Guide (`SPEC-008-R3`)

Tier 3 `rust-wasm` plugins target `wasm32-wasip2` and implement `world aitools-plugin` from `wit/aitools-plugin.wit`.

## Resource Caps
- Linear memory is strictly capped at **64 MB** (`wasmtime::StoreLimits`).
- Execution fuel/epoch timeouts default to **5,000 ms** per tool invocation.
- Export `agent-tool-provider` (`list-tools`, `invoke-tool`) so `aitools-ai::ToolRegistry` automatically discovers your tools.
