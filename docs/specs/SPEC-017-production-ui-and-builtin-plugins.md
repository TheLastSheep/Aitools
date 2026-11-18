# SPEC-017: ZTools-Fidelity Production Desktop UI/UX, Accessibility & CSP-Compliant Built-in Plugins

- **Status:** Active
- **Version:** 2.0.0
- **Owner Crates / Packages:** `packages/aitools-sdk`, `packages/renderer-svelte`, `plugins/builtin/*`, `src-tauri`
- **Extends:** `SPEC-006` (Desktop Shell & Renderer), `SPEC-012` (`@aitools/sdk` & Built-in Plugins), `SPEC-016` (`v1.2.0` Host Wiring, `__app_icon__` & Top-Anchored Resize)

## 1. ZTools-Faithful `800×58` Window Geometry, Vibrancy, Minimal Bar & Routing

- **SPEC-017-R1 (Unconstrained `800×58..594` Sizing, `FullScreenUI` Vibrancy, Minimal Bar & `/#dsh` Route):**
  1. **Unconstrained Dynamic Geometry (`800×58` Idle -> `<= 594px` Expanded):**
     - `src-tauri/tauri.conf.json` MUST configure `"main"` at `800×58` (`transparent: true`, `decorations: false`, `macOSPrivateApi: true`).
     - Frontend `setMainWindowHeight(height)` MUST invoke `resize_main_window` (`58..=594`, top-edge-anchored via `SPEC-016-R3`), falling back to `getCurrentWindow().setSize(new LogicalSize(800, h))` outside custom command contexts.
     - Compact idle bar height is **`58px`** when `mode === "search"` and no chips, hits, suggestions, home grid, status banner, or DSH overlay are rendered.
     - **Anti-Clipping Unconstrained Layout Contract:** `.shell` MUST NOT apply `max-height: 100vh; overflow: hidden;` and `.body` MUST NOT apply `flex: 1; min-height: 0;` self-clamping compression against the current `58px` viewport height. Instead, `.search-bar` MUST remain fixed at `58px` and all expanded content MUST render inside an unconstrained `.dropdown-panel` (`max-height: 536px; overflow-y: auto; overflow-x: hidden;`), measuring `58 + (dropdownOpen ? Math.min(536, Math.ceil(dropdownEl.scrollHeight) + 2) : 0)` (capped at `594px = 58 + 536`) so `WKWebView` resize-clipping cannot deadlock.
  2. **macOS `FullScreenUI` Vibrancy, Transparency & Drag Region:**
     - `aitools_native::vibrancy::apply_hud_vibrancy` MUST configure macOS `NSVisualEffectMaterial::FullScreenUI` with `NSVisualEffectState::Active` and `10.0` corner radius.
     - `html, body` MUST enforce `background: transparent !important; overflow: hidden;` so macOS vibrancy shines cleanly with zero rectangular corner bleed.
     - `.shell` MUST pair ZTools's translucent shell (`rgba(244, 244, 244, 0.82)`–`0.84` light / `rgba(48, 49, 51, 0.86)`–`0.88` dark, `10px` radius, neutral hairline rim, ZERO neon purple divider bar) with `backdrop-filter: blur(...)`.
     - `.search-bar` MUST expose `data-tauri-drag-region` (`core:window:allow-start-dragging`, excluding inputs and interactive buttons).
  3. **Clean Minimal ZTools `58px` Bar & Standalone `/#dsh` Route:**
     - `.search-bar` (`height: 58px; padding: 5px 12px; gap: 10px`) MUST omit clutter chrome (removing the `[A]` left badge, `[Search]` pill, `[sun]` theme toggle button, and `[DSH]` button). DSH remains bound to `Cmd+Shift+D` and System Tray (`/#dsh`, `960×640`), with manual theme switching provided inside `DshPanel` alongside OS `prefers-color-scheme`.
     - Left of input: inline `.pasted-text` capsule (for active clipboard/selection chips with `1px dashed` border, emerald icon, click-to-toggle or `×` remove) OR `.plugin-tag` pill (in `list` / `agent` mode: `Plugin · feature_code ×`).
     - Center: spacious borderless `21px–22px` `.search-input`.
     - Right: subtle `AI` + `<kbd>Tab</kbd>` hint (`>= 4.5:1` contrast) and a `36px×36px` circular `AToolsMark` avatar displaying a streaming ring while Agent execution is active.
     - When `window.location.hash === "#dsh"`, `packages/renderer-svelte` MUST render `DshPanel` as a standalone full-viewport diagnostic workbench rather than the launcher bar.

## 2. Combobox Accessibility, Navigation, IME Guard & ZTools Multi-Mode Views

- **SPEC-017-R2 (ARIA Combobox/Listbox Semantics, Keyboard/Mouse Navigation & CJK IME Guard):**
  1. **ARIA Semantics:** The search input MUST declare `role="combobox"`, `aria-autocomplete="list"`, `aria-expanded`, `aria-controls`, `aria-activedescendant`, and an accessible label; hit/list/home containers MUST declare `role="listbox"` with `role="option"` + `aria-selected`; HITL confirmation cards MUST declare `role="alertdialog"`.
  2. **Keyboard & Pointer Navigation:**
     - `ArrowUp` / `ArrowDown` MUST wrap cleanly through visible home grid items, search results, or list items, updating `selectedIndex` and `aria-activedescendant`.
     - `Enter` MUST launch the selected hit (`launch_item`), confirm the selected list item (`select_list_item`), or submit the Agent prompt (`submit_agent_prompt`).
     - `Cmd/Ctrl + 1..9` MUST quick-launch the corresponding top visible hit.
     - `Cmd/Ctrl + Shift + D` MUST toggle DSH.
     - `Tab` scoped strictly to the search input promotes to Agent mode (without hijacking keyboard focus from chip buttons or interactive controls).
     - `Escape` MUST step back deterministically: close open DSH drawer -> exit `list` mode to `search` -> exit `agent` mode or clear query -> invoke `hide_main_window`.
     - Pointer click (`onclick`) and hover (`onmouseenter`) MUST select and launch hits, grid items, list items, and suggestions.
  3. **CJK IME Composition Guard:** Input and keydown handlers MUST track `compositionstart` / `compositionend` and ignore `e.isComposing || e.keyCode === 229` so Pinyin/IME composition strokes never trigger premature IPC queries or misfire `Enter`, `Tab`, or `Escape`.

- **SPEC-017-R3 (Zero-Query ZTools Home Grid, Highlighted `.list-item` Rows, List Mode & Interactive Agent/HITL):**
  1. **Zero-Query ZTools Home View (`mode === "search"` and `query.trim() === ""`):**
     - When `suggestions.length > 0`, render a compact `智能推荐` `.main-push-section` displaying clean 1-line recommendation titles and concise action hints (omitting raw internal `act.signature` strings).
     - Render ZTools's **9-column `.app-grid`** (`最佳匹配`, `grid-template-columns: repeat(9, 1fr)`) populated by `featuredHomeItems(27)` (`plugin:aitools|featured_home_items(limit)`), initially showing 2 rows (`18` items) with an `展开 (N)` expand toggle, crisp `32×32` Retina PNG icons (`128×128` source via `aitools-plugin://__app_icon__/<hash>.png` with `image-rendering: auto` and deterministic squircle SVG/monogram fallback), and 2-line centered `12px` labels.
  2. **Highlighted Search Rows (`mode === "search"` and `query.trim() !== ""`):**
     - `LauncherHit` MUST carry `feature_code`, `plugin_id`, `label`, `plugin_name`, `description`, `icon`, `kind`, `score`, `match_type`, `confidence`, and `explain`.
     - Render clean ZTools `.list-item` rows (`padding: 8px 12px; gap: 12px; border-radius: 6px`): crisp `32×32` icon (`<img>` or squircle fallback), `14px` `.item-title` with **XSS-safe** Svelte array-segment `<mark class="highlight">` highlighting matched query tokens in orange (`#c2410c` light / `#fb923c` dark, strictly NO `{@html}`), `12px` `.item-subtitle` (`hit.description || hit.plugin_name`), single-line ellipsis truncation (`min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap`), `tabular-nums`, clean `.item-badge` (`应用`/`插件`/`列表`/`Wasm`) + `<kbd>⌘N</kbd>`, and NO raw debug tokens (`pinyin · 98`).
  3. **Headless ZTools List Mode (`mode: "list"`):** Renderer MUST display the `.plugin-tag` context pill, a dedicated `SubInput` field calling `search_list_mode`, live-streamed `ListItem` rows (`title`, `description`, `icon`), and `Enter`/click dispatch to `select_list_item`.
  4. **Interactive Agent & Tier-2 HITL Card:** Agent mode (`mode: "agent"`) MUST bind `submit_agent_prompt` with loading/empty/error states, live streaming tokens, and DAG node status timeline. `pendingHitl` cards MUST bound `<pre>` height (`max-height: 160px; overflow: auto`), display distinct risk-tier badges, and **hide or disable `"Approve Session"` when `riskTier === "Tier2DestructiveOrExternal"`** (enforcing `SPEC-007-R7` / `INV-SEC-04` at the UI layer).

## 3. WCAG 2.2 AA Design System, Theme Cascade & DSH Polish

- **SPEC-017-R4 (WCAG 2.2 AA Contrast, ZTools Emerald/Orange Tokens, CJK Stack & DSH Polish):**
  1. **WCAG 2.2 AA Text & Placeholder Contrast (`>= 4.5:1`):** All text/background and `::placeholder`/background pairs across light and dark modes MUST satisfy `>= 4.5:1` contrast (including `--text-tertiary`, `--accent` on `--accent-subtle` and `--bg-primary`, `--highlight` on `--bg-primary`, `--warning` on `--bg-tertiary`, and `--danger` on `.row.anomaly`), using ZTools emerald (`#047857` light / `#34d399` dark, with accessible `--accent-foreground`) and orange match highlight (`#c2410c` light / `#fb923c` dark).
  2. **Complete `[data-atools-theme]` Cascade:** `:root[data-atools-theme="light"]` and `:root[data-atools-theme="dark"]` MUST override the full semantic token palette (`--bg-*`, `--text-*`, `--border*`, `--accent*`, `--highlight`, `--danger`, `--success`, `--warning`), and `@media (prefers-color-scheme: dark)` MUST scope via `:root:not([data-atools-theme="light"])` so manual theme selection never mixes opposing light/dark semantic tokens.
  3. **Typography, Focus, Motion & Brand Mark:** `--font-sans` MUST include CJK system fallbacks (`"PingFang SC"`, `"Hiragino Sans GB"`, `"Microsoft YaHei"`), `tabular-nums` for logs/scores, tokenized `4px` spacing and radius scales (`10px` shell radius), visible `:focus-visible` outlines, `@media (prefers-reduced-motion: reduce)` rules, and strictly monotonic `AToolsMark` sizing (`small` `20px` < avatar/default `32px–36px` < `large` `72px`).
  4. **DSH Workbench Polish:** `DshPanel` MUST enforce `overflow-x: auto` / `min-height: 0` on `.waterfall`, severity badge coloring plus non-color anomaly markers, fault toggle pending/error feedback, theme switcher controls, `"Close"` button label (instead of `"Esc"`), and clean user-facing copy free of internal spec tags (`SPEC-011`, `SPEC-013`, raw `_rev`).

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
| `SPEC-017-R1` | `800×58..594` top-anchored `resize_main_window`, unconstrained `.dropdown-panel`, `FullScreenUI` vibrancy (`radius: 10.0`), transparent `html, body`, minimal bar & `/#dsh` route | `spec_017_r1_window_sizing_drag_capabilities_and_dsh_route`, `spec_017_r1_vibrancy_symbol_compiles_safely` | `src-tauri`, `aitools-native`, `renderer-svelte` |
| `SPEC-017-R2` | Combobox/listbox ARIA, Arrow/Enter/Escape/Tab/`Cmd+Shift+D`/click navigation & CJK IME composition guard | `spec_017_r2_launcher_aria_keymap_and_ime_composition_guard` | `renderer-svelte`, `src-tauri` |
| `SPEC-017-R3` | Zero-query `.main-push-section` + 9-col `.app-grid` (`featuredHomeItems`), XSS-safe `<mark class="highlight">` `.list-item` rows, headless list-mode UI & Tier-2 HITL `alertdialog` contract | `spec_017_r3_enriched_launcher_hit_list_mode_and_tier2_hitl_contract`, `spec_017_r3_feature_entry_carries_optional_icon_into_feature_meta` | `src-tauri`, `aitools-ai`, `aitools-core`, `renderer-svelte` |
| `SPEC-017-R4` | WCAG 2.2 AA `>= 4.5:1` contrast (light/dark ZTools emerald & orange tokens + `::placeholder`), complete `[data-atools-theme]`, CJK font stack & `AToolsMark` scale (`20 < 36 < 72`) | `spec_017_r4_design_tokens_contrast_theme_cascade_and_mark_scale` | `renderer-svelte`, `src-tauri` |
| `SPEC-017-R5` | Zero inline `<script>`/`<style>` in `plugins/builtin/*/index.html`, external `.js`/`.css`, SDK alignment & audit | `spec_017_r5_builtin_plugins_csp_clean_styled_and_auditable` | `aitools-cli`, `plugins/builtin/*`, `aitools-sdk` |

## 6. Implementation & Verification Status

- **Status:** Active (`v2.0.0` ZTools-faithful `800×58..594` geometry, `FullScreenUI` vibrancy, unconstrained `.dropdown-panel`, minimal bar, 9-column `.app-grid`, and `<mark class="highlight">` rows specified; `v1.0.0` previously verified; `v2.0.0` pending Wave 1/2 verification)
- **Previously Verified (`v1.0.0`, 2026-11-18):**
  - All `SPEC-017-R1` through `SPEC-017-R5` `v1.0.0` linked tests passed (`81/81` workspace tests, `svelte-check` `0 errors`, 8/8 `plugin audit` passing, and programmatic WCAG 2.2 AA `>= 4.5:1` verification).
