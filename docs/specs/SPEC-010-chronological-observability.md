# SPEC-010: Chronological & Continuous Log Management

- **Status:** Active
- **Version:** 1.0.0
- **Owner Crate:** `crates/aitools-telemetry`

## 1. Strict Chronological Ordering & Schema

- **SPEC-010-R1 (Monotonic Sequence + Microsecond Timestamp):** Every log record emitted by Rust crates, Tauri IPC handlers, QuickJS workers, Wasm plugins, or bridged Webview consoles MUST receive:
  - `seq`: Process-wide strictly increasing `u64` (`AtomicU64`) assigned at emission time.
  - `ts`: UTC RFC 3339 timestamp with microsecond precision (`YYYY-MM-DDTHH:MM:SS.ffffffZ`).
  - `level`: `TRACE` | `DEBUG` | `INFO` | `WARN` | `ERROR` | `FATAL`.
  - `trace_id`: Inherited from active `tracing` span (e.g., `wake-<ulid>` or `agent-<ulid>`).
  - `source`: Rust module path or webview label (`main`, `plugin-<id>`).
  - `plugin_id`: Optional plugin identifier.
  - `duration_us` & `perf_anomaly`: Set automatically when a measured span closes and exceeds `SPEC-000` P99 budgets.

## 2. Continuous Persistence, Rotation & Crash Safety

- **SPEC-010-R2 (Continuous JSONL Append):** Records MUST stream asynchronously to `<app_data_dir>/logs/aitools-YYYY-MM-DD.jsonl` in JSON Lines format without blocking UI or hotkey threads.
- **SPEC-010-R3 (Rolling Retention):** Active log files MUST rotate immediately upon reaching **20 MB**. Background cleanup MUST prune rotated files older than **7 days** or whenever total log directory size exceeds **150 MB** (deleting oldest files first).
- **SPEC-010-R4 (Panic Hook Flush):** `aitools-telemetry` MUST install a `std::panic::set_hook` wrapper that records a `FATAL` entry (including panic payload and location) and flushes the file writer synchronously before process abort.

## 3. Secret Redaction & Chronological Export

- **SPEC-010-R5 (Mandatory Secret Scrubbing):** Before persisting to disk or exporting, the redactor MUST mask API keys (`sk-*`, `AIza*`, `ghp_*`, `xoxb-*`), HTTP `Bearer` tokens, and URL userinfo (`https://user:pass@host`) with `[REDACTED]`. When `redact_clipboard` is enabled (default `true`), clipboard text fields are replaced with `<redacted:len=N>`.
- **SPEC-010-R6 (Chronological Timeline & Diagnostic Zip Export):** `aitools-telemetry` MUST support exporting a user-selected time range or `trace_id` into:
  1. `timeline.log`: Human-readable single-file chronological stream sorted ascending by `seq`.
  2. `aitools-diag-<timestamp>.zip`: Archive containing `events.jsonl`, `timeline.log`, and `summary.json` (record counts, error counts, and latency anomaly summary).

## 4. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented in `crates/aitools-telemetry/src/lib.rs` (fixed `E0716` borrow lifetime in `ChronologicalJsonlLayer::on_close`):
  - `SPEC-010-R1`: Verified by `spec_010_r1_strictly_increasing_seq_order`.
  - `SPEC-010-R5`: Verified by `spec_010_r5_redacts_secrets_and_clipboard`.
  - `SPEC-010-R6`: Verified by `spec_010_r6_exports_chronological_timeline_and_zip_bundle`.
  - `SPEC-010-R2` / `SPEC-010-R3` / `SPEC-010-R4`: Implemented (`RollingJsonlWriter`, 20 MB rotation / 7-day / 150 MB retention, `install_panic_hook`).
