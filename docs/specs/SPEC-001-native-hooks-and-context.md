# SPEC-001: Native Hooks, Pre-Focus Context & Credential Isolation

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-native`

## 1. Pre-Focus Context Capture

- **SPEC-001-R1 (Pre-Focus Context Snapshot):** On global hotkey wake, `aitools-native` MUST capture a bounded `PreFocusContext` snapshot (`selection`, `clipboard`, `active_app`, `active_path`) prior to transferring focus to the launcher window, attaching removable context chips for `SPEC-007-R2` without exceeding `SPEC-000` wake-to-paint budgets.
- **SPEC-001-R2 (Double-Modifier Tap Edge Detector):** `DoubleTapDetector` MUST implement a pure monotonic-clock state machine supporting `Command`, `Control`, `Alt`, and `Shift` double-taps, rejecting taps held > 300 ms, inter-tap gaps >= 400 ms, mixed non-modifier chords, and clock rollbacks (`now < last_observed`).
- **SPEC-001-R3 (Hotkey Validation):** `validate_hotkey_label` MUST reject empty, whitespace-padded, control-character, oversized (> 256 bytes), duplicate-modifier, or bare non-function-key labels.

## 2. Bounded Application Catalog & Credential Store

- **SPEC-001-R4 (Bounded App Catalog):** `AppCatalog` MUST scan platform application roots with hard bounds (`MAX_DEPTH = 4`, `MAX_ENTRIES = 4096`, `MAX_APPS = 1024`) and a 30-second snapshot cache TTL so catalog refreshes cannot stall the launcher thread.
- **SPEC-001-R5 (OS Keychain Credential Store):** `SecretStore` MUST isolate LLM provider keys and WebDAV secrets in the OS Keychain (`keyring`, per `INV-SEC-03`) and NEVER write secrets to LMDB, JSON configs, or telemetry logs.

## 3. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Core native primitives implemented and verified in `crates/aitools-native`:
  - `SPEC-001-R1`: `capture_pre_focus_context`, `ClipboardProvider`, `ContextChip` — verified by `spec_001_r1_captures_pre_focus_context_and_chips` (`crates/aitools-native/src/context.rs`) using `MemoryClipboard`.
  - `SPEC-001-R2`: `DoubleTapDetector` — verified by `spec_001_r2_double_tap_timing_and_rollback_safety` (`crates/aitools-native/src/hotkey.rs`).
  - `SPEC-001-R3`: `validate_hotkey_label` — verified by `spec_001_r3_validates_hotkey_labels` (`crates/aitools-native/src/hotkey.rs`).
  - `SPEC-001-R4`: `AppCatalog` — verified by `spec_001_r4_bounded_catalog_discovers_apps_and_respects_caps` (`crates/aitools-native/src/app_catalog.rs`).
  - `SPEC-001-R5`: `KeyringSecretStore` & `MemorySecretStore` — verified by `spec_001_r5_secret_store_isolates_and_deletes_credentials` (`crates/aitools-native/src/secrets.rs`).
- **Pending Scope:** OS-level global hotkey event tap & active window/selection accessibility hooks will bind into `tauri-plugin-aitools` / `src-tauri` in the desktop shell milestone.
