# SPEC-017: Production Desktop UI/UX, Accessibility & CSP-Compliant Built-in Plugins

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crates / Packages:** `packages/aitools-sdk`, `packages/renderer-svelte`, `plugins/builtin/*`, `src-tauri`
- **Extends:** `SPEC-006` (Desktop Shell & Renderer), `SPEC-012` (`@aitools/sdk` & Built-in Plugins)

## 1. Launcher Window Sizing, Vibrancy, Drag Region & Routing

- **SPEC-017-R1 (Dynamic Window Sizing, macOS Vibrancy, Drag & `/#dsh` Route):**
  1. **Dynamic Compact Sizing:** The `main` launcher window (`780px` width) MUST dynamically invoke `getCurrentWindow().setSize(new LogicalSize(780, targetHeight))` (safely guarded outside Tauri in browser preview):
     - Compact idle bar height **`62px`** when `mode === "search"` and no chips, hits, suggestions, or DSH drawer are rendered (eliminating dead vertical whitespace).
     - Smoothly expanded bounded height up to `480px–560px` across search hits, zero-query suggestions, `list` mode, `agent` mode, HITL confirmation cards, and open DSH drawer.
     - `.body` and `.results` MUST set `min-height: 0; overflow-y: auto;` so scrollable content never clips header controls or the DSH panel.
  2. **Vibrancy & Drag Region:** The launcher search bar header MUST expose `data-tauri-drag-region` (backed by `core:window:allow-start-dragging`, excluding interactive inputs/buttons) and pair macOS native vibrancy / `backdrop-filter: blur(...)` with a clean elevated outer border/shadow.
  3. **Standalone `/#dsh` Window Route:** When `window.location.hash === "#dsh"`, `packages/renderer-svelte` MUST render `DshPanel` as a standalone full-viewport diagnostic workbench rather than the search launcher bar.

## 2. Combobox Accessibility, Navigation, IME Guard & Multi-Mode Views

- **SPEC-017-R2 (ARIA Combobox/Listbox Semantics, Keyboard/Mouse Navigation & CJK IME Guard):**
  1. **ARIA Semantics:** The search input MUST declare `role="combobox"`, `aria-autocomplete="list"`, `aria-expanded`, `aria-controls`, `aria-activedescendant`, and an accessible label; hit/list containers MUST declare `role="listbox"` with `role="option"` + `aria-selected`; the DSH toggle MUST declare `aria-expanded`; HITL confirmation cards MUST declare `role="alertdialog"`.
  2. **Keyboard & Pointer Navigation:**
     - `ArrowUp` / `ArrowDown` MUST wrap cleanly through visible results or list items, updating `selectedIndex` and `aria-activedescendant`.
     - `Enter` MUST launch the selected hit (`launch_item`), confirm the selected list item (`select_list_item`), or submit the Agent prompt (`submit_agent_prompt`).
     - `Cmd/Ctrl + 1..9` MUST quick-launch the corresponding top visible hit.
     - `Tab` scoped strictly to the search input promotes to Agent mode (without hijacking keyboard focus from `.dsh-toggle`, chip buttons, or suggestions).
     - `Escape` MUST step back deterministically: close open DSH drawer -> exit `list` mode to `search` -> exit `agent` mode or clear query -> invoke `hide_main_window`.
     - Pointer click (`onclick`) and hover (`onmouseenter`) MUST select and launch hits, list items, and suggestions.
  3. **CJK IME Composition Guard:** Input and keydown handlers MUST track `compositionstart` / `compositionend` and ignore `e.isComposing || e.keyCode === 229` so Pinyin/IME composition strokes never trigger premature IPC queries or misfire `Enter`, `Tab`, or `Escape`.

- **SPEC-017-R3 (Enriched `LauncherHit`, Headless List Mode UI & Interactive Agent/HITL):**
  1. **Enriched Search Row:** `LauncherHit` MUST carry and render `feature_code`, `plugin_id`, `label`, `plugin_name`, `description`, `icon`, `kind`, `score`, `match_type`, `confidence`, and `explain`. Result rows MUST display app/plugin icons, primary labels, plugin/kind badges, single-line ellipsis truncation (`min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap`), and `font-variant-numeric: tabular-nums` on scores.
  2. **Headless ZTools List Mode (`mode: "list"`):** Renderer MUST display the active feature header, a dedicated `SubInput` field calling `search_list_mode`, live-streamed `ListItem` rows (`title`, `description`, `icon`), and `Enter`/click dispatch to `select_list_item`.
  3. **Interactive Agent & Tier-2 HITL Card:** Agent mode (`mode: "agent"`) MUST bind `submit_agent_prompt` with loading/empty/error states, live streaming tokens, and DAG node status timeline. `pendingHitl` cards MUST bound `<pre>` height (`max-height: 160px; overflow: auto`), display distinct risk-tier badges, and **hide or disable `"Approve Session"` when `riskTier === "Tier2DestructiveOrExternal"`** (enforcing `SPEC-007-R7` / `INV-SEC-04` at the UI layer).

## 3. WCAG 2.2 AA Design System, Theme Cascade & DSH Polish

- **SPEC-017-R4 (WCAG 2.2 AA Contrast, Complete Theme Tokens, CJK Stack & DSH Polish):**
  1. **WCAG 2.2 AA Text Contrast (`>= 4.5:1`):** All text/background pairs across light and dark modes MUST satisfy `>= 4.5:1` contrast (including `--text-tertiary`, `--accent` on `--accent-subtle`, `--warning` on `--bg-tertiary`, and `--danger` on `.row.anomaly`).
  2. **Complete `[data-atools-theme]` Cascade:** `:root[data-atools-theme="light"]` and `:root[data-atools-theme="dark"]` MUST override the full semantic token palette (`--bg-*`, `--text-*`, `--border*`, `--accent*`, `--danger`, `--success`, `--warning`), and `@media (prefers-color-scheme: dark)` MUST scope via `:root:not([data-atools-theme="light"])` so manual theme selection never mixes opposing light/dark semantic tokens.
  3. **Typography, Focus, Motion & Brand Mark:** `--font-sans` MUST include CJK system fallbacks (`"PingFang SC"`, `"Hiragino Sans GB"`, `"Microsoft YaHei"`), `tabular-nums` for logs/scores, tokenized `4px` spacing and radius scales, visible `:focus-visible` outlines, `@media (prefers-reduced-motion: reduce)` rules, and strictly monotonic `AToolsMark` sizing (`small` `20px` < default `32px–38px` < `large` `72px`).
  4. **DSH Workbench Polish:** `DshPanel` MUST enforce `overflow-x: auto` / `min-width: 0` on `.waterfall`, severity badge coloring plus non-color anomaly markers, fault toggle pending/error feedback, `"Close"` button label (instead of `"Esc"`), and clean user-facing copy free of internal spec tags (`SPEC-011`, `SPEC-013`, raw `_rev`).

## 4. Strict CSP Compliance & Production Built-in Plugins

- **SPEC-017-R5 (Zero Inline Scripts/Styles & Production Built-in Plugins):**
  1. **Strict `default-src 'self'` CSP Compliance:** Every HTML entry under `plugins/builtin/*/index.html` (`clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`, `todo-legacy`, `snipaste-legacy`) MUST contain **zero inline `<script>` blocks, zero inline `<style>` blocks, and zero inline `on*` event handler attributes**. All logic MUST load from external `main.js` and all styles from external `style.css`.
  2. **Cohesive Styling & Real Plugin Functionality:** Every built-in UI plugin MUST ship light/dark CSS (`prefers-color-scheme`), empty/loading/error states, and aligned SDK/shim contracts:
     - `clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`: aligned with `@aitools/sdk` (`ContextEnvelope` fields `app_name`, `window_title`, `selected_text`, `clipboard_preview`, `captured_at_ms`, `bundle_id`; `AiChatRequest` profiles `"fast" | "balanced" | "deep"`; reconciled `Document` shapes) and working interactive UIs.
     - `todo-legacy` & `snipaste-legacy`: full Tier 1 `window.utools` / `window.ztools` flows (CRUD todo checklist via `db` / `dbStorage`; working Snipaste UI + `preload.js` list/select parity).
     - `json-codec` & `timestamp-calc` (Tier 3 `rust-wasm`): valid `aitools.manifest.json`, declared `features`, `SKILL.md`, and passing `aitools plugin audit`.
     - All 8 `plugins/builtin/*` directories MUST declare searchable `features` and pass `aitools plugin audit`.

## 5. Clause-to-Test Traceability

| Clause | Requirement Summary | Linked Test / Check (`crates/*/src/`, `crates/*/tests/`, or Workspace Check) | Owner Crate / Package |
| :--- | :--- | :--- | :--- |
| `SPEC-017-R1` | `allow-set-size` (`62px` idle -> bounded), `allow-start-dragging`, macOS vibrancy & `/#dsh` route | `spec_017_r1_window_sizing_drag_capabilities_and_dsh_route`, `spec_017_r1_vibrancy_symbol_compiles_safely` | `src-tauri`, `aitools-native`, `renderer-svelte` |
| `SPEC-017-R2` | Combobox/listbox ARIA, Arrow/Enter/Escape/click navigation & CJK IME composition guard | `spec_017_r2_launcher_aria_keymap_and_ime_composition_guard` | `renderer-svelte`, `src-tauri` |
| `SPEC-017-R3` | Enriched `LauncherHit` metadata, headless list-mode UI & Tier-2 HITL `alertdialog` contract | `spec_017_r3_enriched_launcher_hit_list_mode_and_tier2_hitl_contract` | `src-tauri`, `aitools-ai`, `renderer-svelte` |
| `SPEC-017-R4` | WCAG 2.2 AA `>= 4.5:1` contrast, complete `[data-atools-theme]`, CJK font stack & `AToolsMark` scale | `spec_017_r4_design_tokens_contrast_theme_cascade_and_mark_scale` | `renderer-svelte`, `src-tauri` |
| `SPEC-017-R5` | Zero inline `<script>`/`<style>` in `plugins/builtin/*/index.html`, external `.js`/`.css`, SDK alignment & audit | `spec_017_r5_builtin_plugins_csp_clean_styled_and_auditable` | `aitools-cli`, `plugins/builtin/*`, `aitools-sdk` |

## 6. Implementation & Verification Status

- **Status:** Verified (Production Svelte UI, WCAG 2.2 AA tokens, `AToolsMark` scale, window sizing/vibrancy/DSH route, and 8 CSP-clean built-in plugins implemented and verified)
- **Verified By Wave 3 (`@verifier`, 2026-11-18):**
  - All `SPEC-017-R1` through `SPEC-017-R5` linked tests passed in `cargo test --workspace` (`spec_017_r1`–`spec_017_r4` in `src-tauri/src/lib.rs`, `spec_017_r1_vibrancy_symbol_compiles_safely` in `aitools-native`, `spec_017_r5_builtin_plugins_csp_clean_styled_and_auditable` in `aitools-cli`; 81/81 workspace tests passed, 0 failed, 0 ignored).
  - `pnpm --filter @aitools/renderer-svelte check && pnpm --filter @aitools/renderer-svelte build` -> `svelte-check found 0 errors and 0 warnings`; Vite built production bundle in `packages/renderer-svelte/dist/`.
  - `cargo run -q -p aitools-cli -- plugin audit plugins/builtin/*` passed across all 8 built-in plugins (`skill_valid: true`, `warnings: []`), with 0 inline `<script>`, `<style>`, `on*`, or `style=` attributes across `plugins/builtin/*/index.html`.
  - Programmatic WCAG 2.2 AA contrast (`>= 4.5:1`) across light and dark semantic tokens verified by `spec_017_r4_design_tokens_contrast_theme_cascade_and_mark_scale`.
