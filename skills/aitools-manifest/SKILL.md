---
name: aitools-manifest
description: Author and validate aitools.manifest.json (v2) manifests for native-ui, rust-wasm, and hybrid plugins.
---

# `aitools.manifest.json` Authoring Guide (`SPEC-008-R4`)

Every Tier 2 (`native-ui`) and Tier 3 (`rust-wasm` / `hybrid`) plugin declares `aitools.manifest.json` at its package root:

- `id`: Unique kebab-case plugin identifier.
- `name`: Human-readable display name.
- `version`: SemVer string (e.g., `"0.1.0"`).
- `runtime`: `"native-ui"`, `"rust-wasm"`, or `"hybrid"`.
- `main`: HTML entry path (required for `native-ui` and `hybrid`).
- `wasm_entry`: Compiled `wasm32-wasip2` component path (required for `rust-wasm` and `hybrid`).
- `permissions`: Explicit capability allowlist (e.g., `["clipboard.read", "db.read", "ai.chat"]`).
- `ai_tools`: Array of `{ name, description, risk_tier, input_schema }` definitions.
- `skill_file`: Optional path to `SKILL.md` (defaults to `"SKILL.md"`).

Validate with `aitools plugin audit <dir>`.
