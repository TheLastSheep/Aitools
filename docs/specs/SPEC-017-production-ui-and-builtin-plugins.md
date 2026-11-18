# SPEC-017: ZTools-Fidelity Production Desktop UI/UX, Interaction Mechanics & CSP-Compliant Built-in Plugins

- **Status:** Active
- **Version:** 2.1.0
- **Owner Crates / Packages:** `packages/aitools-sdk`, `packages/renderer-svelte`, `plugins/builtin/*`, `src-tauri`
- **Extends:** `SPEC-006` (Desktop Shell & Renderer), `SPEC-012` (`@aitools/sdk` & Built-in Plugins), `SPEC-016` (`v1.3.0` Host Launcher State, `IconAdaptation`, `launcher_home_sections`, `launcher_mutate_state` & `reveal_in_finder`)
- **Open-Source Reference:** `/Users/harris/Desktop/ZTools/src/renderer/src/` (`style.css`, `App.vue`, `SearchBox.vue`, `SearchResults.vue`, `AggregateView.vue`, `navigationGrid.ts`, `useNavigation.ts`, `CollapsibleList.vue`, `CommandList.vue`, `VerticalList.vue`, `AdaptiveIcon.vue`, `useSearchResults.ts`, `commandDataStore.ts`)

## 1. ZTools-Faithful `800×58` Window Geometry, Vibrancy, Unified Single-Input `SearchBox` & Routing

- **SPEC-017-R1 (Unconstrained `800×58..594` Sizing, `FullScreenUI` Vibrancy, Unified Single-Input `58px` `SearchBox` & `/#dsh` Route):**
  1. **Unconstrained Dynamic Geometry (`800×58` Idle -> `<= 594px` Expanded):**
     - `src-tauri/tauri.conf.json` MUST configure `"main"` at `800×58` (`transparent: true`, `decorations: false`, `macOSPrivateApi: true`).
     - Frontend `setMainWindowHeight(height)` MUST invoke `resize_main_window` (`58..=594`, top-edge-anchored via `SPEC-016-R3`), falling back to `getCurrentWindow().setSize(new LogicalSize(800, h))` outside custom command contexts.
     - Compact idle bar height is **`58px`** when `mode === "search"` and no chips, hits, suggestions, home grid, status banner, context menu, or DSH overlay are rendered.
     - **Anti-Clipping Unconstrained Layout Contract:** `.shell` MUST NOT apply `max-height: 100vh; overflow: hidden;` and `.body` MUST NOT apply `flex: 1; min-height: 0;` self-clamping compression against the current `58px` viewport height. Instead, `.search-bar` MUST remain fixed at `58px` and all expanded content MUST render inside an unconstrained `.dropdown-panel` (`max-height: 536px; overflow-y: auto; overflow-x: hidden;`), measuring `58 + (dropdownOpen ? Math.min(536, Math.ceil(dropdownEl.scrollHeight) + 2) : 0)` (capped at `594px = 58 + 536`) and accommodating any open context menu bottom edge inside `syncWindowSize()` so `WKWebView` resize-clipping cannot deadlock or clip menus.
  2. **macOS `FullScreenUI` Vibrancy, Transparency & Drag Region:**
     - `aitools_native::vibrancy::apply_hud_vibrancy` MUST configure macOS `NSVisualEffectMaterial::FullScreenUI` with `NSVisualEffectState::Active` and `10.0` corner radius.
     - `html, body` MUST enforce `background: transparent !important; overflow: hidden;` so macOS vibrancy shines cleanly with zero rectangular corner bleed.
     - `.shell` MUST pair ZTools's translucent shell (`rgba(244, 244, 244, 0.84)` light / `rgba(48, 49, 51, 0.88)` dark, `10px` radius, neutral hairline rim, ZERO neon purple divider bar) with `backdrop-filter: blur(...)`.
     - `.search-bar` MUST expose `data-tauri-drag-region` (`core:window:allow-start-dragging`, excluding inputs, chips, and interactive controls).
  3. **Unified Single-Input `58px` ZTools `SearchBox` & In-Place `SubInput` (`SearchBox.vue`, `App.svelte`):**
     - `.search-bar` (`height: 58px; padding: 5px 12px; gap: 10px`) MUST omit clutter chrome (no `[A]` left badge, `[Search]` pill, `[sun]` button, or `[DSH]` button). DSH remains bound to `Cmd/Ctrl+Shift+D` and System Tray (`/#dsh`, `960×640`), with theme & search-view-mode controls inside `DshPanel` alongside OS `prefers-color-scheme`.
     - **Single Top Input (No Duplicate `SubInput` Box in `list` Mode):** The renderer MUST NOT render a duplicate `.list-header` + second `.sub-input` box inside `.dropdown-panel` when `mode === "list"`. Instead, the ONE top `.search-input` MUST bind directly to `store.subInput` in `list` mode while rendering ZTools's `.plugin-tag.has-cmd` pill on the left: optional `.plugin-tag-icon` (`listSession.plugin_icon` with `.adaptive-invert` support), angled `clip-path` `.plugin-tag-left` displaying plugin title, `.plugin-tag-cmd` displaying `feature_code`, and SVG `×` `.plugin-tag-close` button (`:active { transform: scale(0.9) }`).
     - **Dynamic Auto-Width `.search-input` & Context-Aware `.placeholder-text`:** `.search-input` width MUST be dynamically measured from its current text/composing text using a hidden `.measure-text` element (`minWidth = 2px` when empty, capped at available bar space minus `.search-actions` and tags), leaving the right-side `.search-bar` / `.input-wrapper` empty area open as a `data-tauri-drag-region`. Clicking anywhere on `.input-wrapper`, empty `.search-bar`, or empty `.dropdown-panel` background MUST focus `.search-input`. An independent `.placeholder-text` element (`>= 4.5:1` WCAG AA contrast) MUST show `"搜索"` whenever active context chips/pasted items exist and the full placeholder otherwise.
     - **`.pasted-text` Chips, Paste, Secondary Filter (`Cmd+F`) & File Drag `.drag-overlay`:**
       - `.pasted-text` capsules (`1px dashed` border, emerald icon) MUST middle-truncate text via `truncateMiddle(text, 30)` (`head 15...tail 10`), allow clicking the chip body to restore its text into `.search-input` and focus, and convert empty-input `Backspace`/`Delete` into removing the last chip while restoring its text into `.search-input` and selecting all (`select()`) (or exiting `list` mode when in `list` mode with empty `subInput`).
       - `Cmd/Ctrl+F` on non-empty `.search-input` MUST convert the query text into a secondary filter chip and clear the input; `Cmd/Ctrl+V` (`onpaste`) on an empty input MUST attach pasted text/image/files as chips rather than dumping into the search box.
       - Dragging files over `.search-bar` (`dragenter`/`dragover`/`dragleave`/`drop`) MUST render `.drag-overlay` and attach dropped files on drop.
     - **Right `.search-actions` & `AToolsMark` AI Ripple/Pulse (`SearchBox.vue`, `AToolsMark.svelte`):**
       - `.search-actions` MUST display the subtle `.tab-target-hint` (`AI` + `<kbd>Tab</kbd>`, `>= 4.5:1` contrast) and `.avatar-wrapper` (`36px×36px` circular mark):
         - During `listLoading`, `.avatar-wrapper` MUST display the 3-dot `.action-indicator` + plugin logo + `.avatar-spinner`.
         - During Agent streaming / HITL, `AToolsMark` MUST render ZTools's concentric `.ai-animation-layer` (`.ai-mask`, `.ai-text` `"AI"`, `.ai-ripple-container` expanding rings on `sending`, and `.ai-pulse-container` contracting rings on `receiving`).
     - When `window.location.hash === "#dsh"`, `packages/renderer-svelte` MUST render `DshPanel` as a standalone full-viewport diagnostic workbench rather than the launcher bar.

## 2. Combobox Accessibility, 2D `NavigationGrid`, Dual Search Views & ZTools Interaction Parity

- **SPEC-017-R2 (ARIA Semantics, Hover != Selected Decoupling, 2D `NavigationGrid` Wrap-Around, Multi-Step `Escape` & CJK IME Guard):**
  1. **ARIA Semantics:** The search input MUST declare `role="combobox"`, `aria-autocomplete="list"`, `aria-expanded`, `aria-controls`, `aria-activedescendant`, and an accessible label; hit/list/home containers MUST declare `role="listbox"` with `role="option"` + `aria-selected` (`id="hit-option-{idx}"`); right-click context menus MUST declare `role="menu"` / `role="menuitem"`; HITL confirmation cards MUST declare `role="alertdialog"`.
  2. **Decoupled Pointer `:hover` vs `.selected` & Tactile Press (`CommandList.vue`, `VerticalList.vue`):**
     - Pointer movement (`onmouseenter`) MUST NOT mutate active keyboard selection (`selectedIndex` / `(selectedRow, selectedCol)`) on `.app-item`, `.list-item`, or `.push-item`.
     - Pointer hover MUST be gated via `@media (hover: hover)` (`150ms ease-out`) rendering `--hover-bg` (`:hover:not(.selected)`), distinct from `.selected` (`--active-bg` on `.app-item.selected`, `--primary-light-bg` on `.list-item.selected`).
     - `.app-item.selected` MUST render background tint ONLY with **ZERO** `1px` accent outline/inset border box (`CommandList.vue:285-291`). Interactive items MUST provide subtle press feedback (`:active { transform: scale(0.97); }`).
  3. **Default `(0,0)` Selection & 2D `NavigationGrid` Wrap-Around (`navigationGrid.ts`, `useNavigation.ts`):**
     - Whenever visible sections/items exist on wake, search change, or mode load, selection MUST default to `(selectedRow = 0, selectedCol = 0)` (`idx = 0`), automatically clamping `(selectedRow, selectedCol)` whenever dynamic grid contents shrink.
     - `ArrowDown` / `ArrowUp` MUST step vertically across grid rows (with column clamping `Math.min(col, targetRow.length - 1)` in `aggregate` mode) and wrap seamlessly from last row to first row and first row to last row.
     - `ArrowRight` / `ArrowLeft` in `"aggregate"` mode MUST step horizontally across cells and wrap between rows/sections (`(0, 0) <-> (lastRow, lastCol)`) when the search input has no text selection (`selectionStart === selectionEnd`), while leaving text caret navigation intact in `"list"` mode.
     - `Enter` MUST launch the selected hit (`launch_item`), confirm the selected list item (`select_list_item`), or submit the Agent prompt (`submit_agent_prompt`).
     - `Cmd/Ctrl + 1..9` MUST quick-launch the corresponding top visible hit; `Cmd/Ctrl + Shift + D` MUST toggle DSH; `Cmd/Ctrl + Shift + L` MUST toggle `searchViewMode` (`"aggregate"` <-> `"list"`).
     - `Tab` scoped strictly to the search input promotes to Agent mode (without hijacking keyboard focus from chip buttons or interactive controls); `Shift+Tab` / grid navigation moves cleanly without trapped focus.
     - **Multi-Step `Escape`:** `Escape` MUST unwind deterministically: dismiss open context menu -> close open DSH drawer -> clear `subInput` -> clear chips -> exit `list` or `agent` mode back to `search` -> clear `statusBanner` -> clear `query` -> invoke `hide_main_window`.
  4. **CJK IME Composition Guard:** Input and keydown handlers MUST track `compositionstart` / `compositionend` and ignore `e.isComposing || e.keyCode === 229` so Pinyin/IME composition strokes never trigger premature IPC queries or misfire `Enter`, `Tab`, `Arrow*`, or `Escape`.

- **SPEC-017-R3 (Zero-Query Home Sections, Dual `aggregate`/`list` Search Views, Drag-and-Drop Reorder, Context Menu, Adaptive Icons & HITL):**
  1. **Zero-Query ZTools Home View (`mode === "search"` and no query/chips):**
     - Fetch sections via `launcher_home_sections(27)` (`SPEC-016-R2`):
       - When `suggestions.length > 0`, render `智能推荐` `.main-push-section` with concise titles and action hints (omitting raw internal `act.signature` strings).
       - Render collapsible 9-column `.app-grid` sections (`最近使用` and `已固定`, falling back to `推荐应用` when both are empty), each showing up to 2 rows (`18` items) by default with a **full-row clickable** `.section-header.clickable` (`展开 (N)` / `收起`).
       - `已固定` items MUST support HTML5 drag-and-drop reordering (`.chosen` / `.ghost` visual classes) persisting immediately via `launcher_mutate_state({ op: "reorder_pinned", items })`.
       - Render crisp `32×32` Retina icons (`128×128` via `aitools-plugin://__app_icon__/<hash>.png` or `data:`) with deterministic squircle monogram fallback and 2-line centered `12px` `.app-name` labels.
  2. **Dual Search Views (`searchViewMode: "aggregate" | "list"`, `localStorage` `"aitools:search-mode"`, default `"aggregate"`):**
     - **`"aggregate"` Mode (`AggregateView.vue`):** Partition search hits via `partitionSearchHits` into collapsible 9-column `.app-grid` sections: `最佳搜索结果` (`exact`/`prefix`/`pinyin`/`contains`/`fuzzy`), `最佳匹配` (`regex`/`img`/`files`), and `匹配推荐` (`over`, displaying `.app-pin-badge` when `is_recommendation_pinned` is true), each with full-row clickable `.section-header.clickable` (`展开 (N)` / `收起`), XSS-safe Svelte array-segment `<mark class="highlight">` inside `.app-name` (NO `{@html}`), and `id="hit-option-{idx}"`, `role="option"`, `aria-selected`.
     - **`"list"` Mode (`VerticalList.vue`):** Merge and rank via `buildListModeResults` (exact > prefix > app > `use_count` > `score`) into `.vertical-list` `.list-item` rows (`padding: 8px 12px; gap: 12px; border-radius: 6px`): crisp `32×32` icon, `14px` `.item-title` with XSS-safe `<mark class="highlight">`, `12px` `.item-subtitle`, clean `.item-badge` (`应用`/`插件`/`列表`/`Wasm`/`宏`) + `<kbd>⌘N</kbd>`, and NO raw debug tokens (`pinyin · 98`).
  3. **ZTools Right-Click Context Menu (`SearchResults.vue`):**
     - Right-clicking an item (`contextmenu`) MUST open an accessible custom ZTools menu (`role="menu"` / `role="menuitem"`, keyboard navigation, Escape/outside-click dismiss, clamped within `800×594` and included in `syncWindowSize()`):
       - Recent items: `从使用记录删除` (`remove_recent`)
       - Local `.app` / plugin items with `target_path`: `打开文件位置` (`reveal_in_finder`)
       - Search/home items: `固定到搜索框` / `从搜索框取消固定` (`pin` / `unpin`)
       - Recommendation (`over`) items: `置顶` / `取消置顶` / `排到最前` (`pin_recommendation` / `unpin_recommendation` / `move_recommendation_to_front`)
       - Global view/debug actions: `切换为列表视图 (⌘⇧L)` / `切换为聚合视图 (⌘⇧L)` and `打开诊断面板 (⌘⇧D)`.
  4. **Monochrome `.adaptive-invert` Icon Adaptation (`AdaptiveIcon.vue`):**
     - `shouldInvertIcon(iconIsDark, resolvedTheme)` MUST apply `.adaptive-invert { filter: invert(1) brightness(1.1); }` strictly when `icon_is_dark !== null && icon_is_dark === (resolvedTheme === "dark")`, leaving full-color icons and `system-apps` `.app` icons (`icon_is_dark == null`) completely uninverted.
  5. **Headless ZTools List Mode & Interactive Agent/HITL:**
     - `mode === "list"` binds the single top `.search-input` to `search_list_mode`, renders live-streamed `ListItem` rows (`title`, `description`, `icon`), and dispatches `Enter`/click to `select_list_item`.
     - Agent mode (`mode === "agent"`) binds `submit_agent_prompt` with loading/empty/error states, live streaming tokens, and DAG node timeline. `pendingHitl` cards MUST bound `<pre>` height (`max-height: 160px; overflow: auto`), display distinct risk-tier badges, and **hide or disable `"Approve Session"` when `riskTier === "Tier2DestructiveOrExternal"`** (`INV-SEC-04`).

## 3. WCAG 2.2 AA Design System, Theme Cascade & DSH Polish

- **SPEC-017-R4 (WCAG 2.2 AA Contrast, ZTools `color-mix` & Emerald/Orange Tokens, Scrollbar Ramp, CJK Stack & DSH Polish):**
  1. **WCAG 2.2 AA Text & Placeholder Contrast (`>= 4.5:1`):** All text/background and `::placeholder`/background pairs across light and dark modes MUST satisfy `>= 4.5:1` contrast (preserving all 19 audited hex tokens `#1f2328`, `#47505c`, `#525963`, `#047857`, `#065f46`, `#c2410c`, `#9a3412`, `#b42318`, `#854d0e`, `#f3f4f6`, `#c6cbd2`, `#aeb5bf`, `#34d399`, `#6ee7b7`, `#fb923c`, `#fdba74`, `#fecaca`, `#fbbf24`, `#052e16`), pairing ZTools `color-mix(in srgb, ...)` semantic tokens (`--hover-bg`, `--active-bg`, `--primary-light-bg`), themed `::selection`/`::-moz-selection`, and ZTools's 6px `::-webkit-scrollbar` opacity ramp (`0.08`/`0.15`/`0.25` light, `0.10`/`0.20`/`0.35` dark).
  2. **Complete `[data-atools-theme]` Cascade:** `:root[data-atools-theme="light"]` and `:root[data-atools-theme="dark"]` MUST override the full semantic token palette (`--bg-*`, `--text-*`, `--border*`, `--accent*`, `--hover-bg`, `--active-bg`, `--primary-light-bg`, `--highlight*`, `--danger`, `--success`, `--warning`), and `@media (prefers-color-scheme: dark)` MUST scope via `:root:not([data-atools-theme="light"])` so manual theme selection never mixes opposing light/dark semantic tokens.
  3. **Typography, Focus, Motion & Brand Mark:** `--font-sans` MUST include CJK system fallbacks (`"PingFang SC"`, `"Hiragino Sans GB"`, `"Microsoft YaHei"`), `tabular-nums` for logs/scores, tokenized `4px` spacing and radius scales (`10px` shell radius), visible `:focus-visible` outlines, `@media (prefers-reduced-motion: reduce)` rules, and strictly monotonic `AToolsMark` sizing (`small` `20px` < avatar/default `36px` < `large` `72px`).
  4. **DSH Workbench Polish:** `DshPanel` MUST enforce `overflow-x: auto` / `min-height: 0` on `.waterfall`, severity badge coloring plus non-color anomaly markers, fault toggle pending/error feedback, theme switcher controls, `searchViewMode` (`aggregate` / `list`) switcher controls, `"Close"` button label, and clean user-facing copy free of internal spec tags (`SPEC-011`, `SPEC-013`, raw `_rev`).

## 4. Strict CSP Compliance & Production Built-in Plugins

- **SPEC-017-R5 (Zero Inline Scripts/Styles & Production Built-in Plugins):**
  1. **Strict `default-src 'self'` CSP Compliance:** Every HTML entry under `plugins/builtin/*/index.html` (`clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`, `todo-legacy`, `snipaste-legacy`) MUST contain **zero inline `<script>` blocks, zero inline `<style>` blocks, and zero inline `on*` event handler attributes**. All logic MUST load from external `main.js` and all styles from external `style.css`.
  2. **Cohesive Styling & Real Plugin Functionality:** Every built-in UI plugin MUST ship light/dark CSS (`prefers-color-scheme`), empty/loading/error states, and aligned SDK/shim contracts:
     - `clipboard-history`, `translation`, `terminal-launchpad`, `dev-docs-reference`: aligned with `@aitools/sdk` (`ContextEnvelope` fields `app_name`, `window_title`, `selected_text`, `clipboard_preview`, `captured_at_ms`, `bundle_id`; `AiChatRequest` profiles `"fast" | "balanced" | "deep"`; reconciled `Document` shapes) and working interactive UIs.
     - `todo-legacy` & `snipaste-legacy`: full Tier 1 `window.utools` / `window.ztools` flows (CRUD todo checklist via `db` / `dbStorage`; working Snipaste UI + `preload.js` list/select parity).
     - `json-codec` & `timestamp-calc` (Tier 3 `rust-wasm`): valid `aitools.manifest.json`, declared `features`, `SKILL.md`, and passing `aitools plugin audit`.
     - All 8 `plugins/builtin/*` directories MUST declare searchable `features` and pass `aitools plugin audit`.

## 5. Clause-to-Test Traceability

| Clause | Requirement Summary | Linked Test / Check (`crates/*/src/`, `crates/*/tests/`, or Differential Parity Suite) | Owner Crate / Package |
| :--- | :--- | :--- | :--- |
| `SPEC-017-R1` | `800×58..594` top-anchored `resize_main_window`, unconstrained `.dropdown-panel`, `FullScreenUI` vibrancy (`radius: 10.0`), transparent `html, body`, unified single-input `SearchBox` (`.measure-text`, `.placeholder-text`, `.pasted-text`, `.drag-overlay`, `.plugin-tag.has-cmd`, in-place `SubInput`, `AToolsMark` `.ai-animation-layer`) & `/#dsh` route | `spec_017_r1_window_sizing_drag_capabilities_and_dsh_route`, `spec_017_r1_vibrancy_symbol_compiles_safely` | `src-tauri`, `aitools-native`, `renderer-svelte` |
| `SPEC-017-R2` | Combobox/listbox/menu/alertdialog ARIA, decoupled `:hover` vs `.selected` (zero `onmouseenter` hijacking, zero `1px` outline on `.app-item.selected`), default `(0,0)` & 2D `NavigationGrid` wrap-around, multi-step `Escape`, `Cmd+Shift+L`/`Cmd+Shift+D`, CJK IME guard | `spec_017_r2_launcher_aria_keymap_and_ime_composition_guard`, `ztools_navigation_rs` (`rust-anything` differential suite: 2,278/2,278 cases incl. 1,764 sequence checks & `tests/renderer/navigationGrid.test.ts`) | `renderer-svelte`, `src-tauri`, `ztools_navigation_rs` |
| `SPEC-017-R3` | Zero-query `launcher_home_sections` (`最近使用`/`已固定` DnD + `智能推荐`), dual `aggregate`/`list` search views, full-row `.section-header.clickable`, right-click context menu, `.adaptive-invert` monochrome adaptation, XSS-safe `<mark class="highlight">`, headless list mode & Tier-2 HITL `alertdialog` | `spec_017_r3_enriched_launcher_hit_list_mode_and_tier2_hitl_contract`, `spec_017_r3_feature_entry_carries_optional_icon_into_feature_meta`, `ztools_icon_adapt_rs` (1,427/1,427 cases), `ztools_command_rank_rs` (2,627/2,627 cases incl. `useSearchResults.test.ts` & `recommendationPins.test.ts`), `ztools_highlight_rs` (1,178/1,178 cases) | `src-tauri`, `aitools-ai`, `aitools-core`, `renderer-svelte` |
| `SPEC-017-R4` | WCAG 2.2 AA `>= 4.5:1` contrast (19 audited light/dark hex tokens), ZTools `color-mix` `--hover-bg`/`--active-bg`/`--primary-light-bg`, 6px scrollbar ramp, `[data-atools-theme]` cascade, CJK font stack, `AToolsMark` scale (`20 < 36 < 72`) & DSH view-mode switcher | `spec_017_r4_design_tokens_contrast_theme_cascade_and_mark_scale` | `renderer-svelte`, `src-tauri` |
| `SPEC-017-R5` | Zero inline `<script>`/`<style>` in `plugins/builtin/*/index.html`, external `.js`/`.css`, SDK alignment & audit | `spec_017_r5_builtin_plugins_csp_clean_styled_and_auditable`, `char_transform_rs` (`rust-anything` differential suite: 4,092/4,092 cases) | `aitools-cli`, `plugins/builtin/*`, `aitools-sdk` |

## 6. Implementation & Verification Status

- **Status:** Active (`v2.1.0` ZTools interaction & style alignment — decoupled `:hover` vs `.selected`, 2D `NavigationGrid`, dual `aggregate`/`list` views, unified single-input `58px` `SearchBox` with in-place `SubInput`, DnD `已固定`, right-click context menu, `.adaptive-invert`, and `AToolsMark` AI ripple/pulse — specified & implemented in Wave 1; 5 ZTools reference modules verified differentially via `rust-anything` `v1.5.1` at 11,602/11,602 passing checks with 0 mismatches; pending Wave 2 independent verification & Retina 2x E2E run)
- **Wave 1 Frontend & Differential Verification (`v2.1.0`, 2026-11-18):**
  - `pnpm --filter @aitools/renderer-svelte check` (`0` errors/warnings), `pnpm --filter @aitools/renderer-svelte build`, and `cargo test -p aitools-desktop` (9/9 passed) reported passing by `@ztools-interaction-ui`.
  - Isolated `rust-anything` v1.5.1 parity across 5 ported ZTools units (`ztools_highlight`, `char_transform`, `ztools_navigation`, `ztools_icon_adapt`, `ztools_command_rank`): **11,602 / 11,602** differential checks passed (`0` mismatches, all gates passed).
