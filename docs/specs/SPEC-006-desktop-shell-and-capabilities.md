# SPEC-006: Desktop Host Shell, Capability Isolation & Svelte Renderer

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crates / Packages:** `crates/tauri-plugin-aitools`, `src-tauri`, `packages/renderer-svelte`

## 1. Least-Privilege Tauri Capability Splitting

- **SPEC-006-R1 (`main-shell.json` Host Privileges):** `src-tauri/capabilities/main-shell.json` MUST scope full host window control, global hotkey binding, AI orchestration, and telemetry export strictly to `"windows": ["main", "dsh"]`.
- **SPEC-006-R2 (`plugin-sandbox.json` Webview Sandbox):** `src-tauri/capabilities/plugin-sandbox.json` MUST restrict `"webviews": ["plugin-*"]` strictly to `aitools:default`, `dialog`, `notification`, and `opener` permissions. Plugin webviews MUST NEVER receive filesystem write, shell spawn, or AI key access.

## 2. Custom Protocol Isolation & Wake Closure

- **SPEC-006-R3 (Path-Canonicalized Asset Serving):** `aitools-plugin://<plugin_id>/<asset_path>` MUST verify `UriSchemeContext::webview_label() == "plugin-<plugin_id>"` (`INV-SEC-01`) and canonicalize resolved paths under the plugin's root directory to reject `..` path traversal with HTTP `403`.
- **SPEC-006-R4 (Wake Hotkey Pre-Focus Channel):** On global hotkey trigger, `src-tauri` MUST synchronously capture `PreFocusContext` (`SPEC-001-R1`) before revealing `main`, and push context chips over `tauri::ipc::Channel` tagged with a fresh `wake-<seq>` `trace_id`.

## 3. Svelte 5 Dual-Mode Renderer & Embedded DSH

- **SPEC-006-R5 (Unified Dual-Mode & DSH Shell):** `packages/renderer-svelte` MUST route keystrokes to `aitools-core` hybrid search (< 5 ms), promote to `aitools-ai` Agent mode on `Tab` or `@` with removable context chips, and toggle the embedded DSH diagnostic drawer on `Cmd+Shift+D`.

## 4. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented across `crates/tauri-plugin-aitools`, `src-tauri`, and `packages/renderer-svelte`:
  - `SPEC-006-R1` & `SPEC-006-R2`: Least-privilege capability split in `src-tauri/capabilities/main-shell.json` & `plugin-sandbox.json` — verified by `spec_006_r1_and_r2_capabilities_isolate_plugins` (`src-tauri/src/lib.rs`).
  - `SPEC-006-R3`: `handle_plugin_asset_request` with `validate_sync_caller` + `std::fs::canonicalize` — verified by `inv_sec_01_blocks_cross_plugin_and_path_traversal` (`crates/tauri-plugin-aitools/src/lib.rs`).
  - `SPEC-006-R4`: Global wake hotkey (`Alt+Space`) synchronously captures `PreFocusContext` and streams `WakePayload` via `tauri::ipc::Channel` (`register_wake_channel`).
  - `SPEC-006-R5`: `packages/renderer-svelte` (`App.svelte`, `DshPanel.svelte`, `stores/launcher.svelte.ts`) verified cleanly with `svelte-check` (0 errors, 0 warnings) and `vite build`.
