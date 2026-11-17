---
name: utools-to-aitools-migration
description: Migrate legacy uTools or ZTools v1 plugins (plugin.json + preload.js) to aitools native-ui or rust-wasm v2 plugins.
---

# Migrating ZTools / uTools Plugins to `aitools` (`SPEC-008`)

1. Legacy v1 plugins (`plugin.json`) run automatically under Tier 1 (`legacy-compat`) with identical frozen globals (`window.aitools === window.ztools === window.utools`).
2. To upgrade to Tier 2 (`native-ui`), replace `plugin.json` with `aitools.manifest.json`, declare explicit `permissions` and `ai_tools`, and add a `SKILL.md` describing tool usage for the agent orchestrator.
3. For CPU-intensive or untrusted backends, extract logic from `preload.js` into a `wasm32-wasip2` component implementing `wit/aitools-plugin.wit`.
