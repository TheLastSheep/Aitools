# SPEC-014: End-to-End Trace Propagation, Auto-Profiling & Self-Evolution Loop

- **Status:** Active
- **Version:** 1.1.0
- **Owner Crates:** `crates/aitools-telemetry`, `crates/aitools-cli`, `crates/tauri-plugin-aitools`

## 1. End-to-End Cross-Boundary `trace_id` Propagation

- **SPEC-014-R1 (W3C/ULID Trace Continuity):** Every wake, search, plugin launch, or agent turn MUST propagate `trace_id` across Rust `tracing` spans, Tauri IPC headers (`x-aitools-trace-id`) / `Channel` envelopes, `aitools-plugin://` / `aitools-sync://` (`X-AITools-Trace-Id`), and frontend `ingest_external`.

## 2. Automated Slow-Span Profiling & `aitools doctor`

- **SPEC-014-R2 (Anomaly Snapshot Enrichment):** Whenever a span closes with `perf_anomaly: true`, `aitools-telemetry` attaches anomaly metadata (`active_plugin_count`, `rss_bytes` / `slowest_subspan` when captured) to `LogRecord.fields`.
- **SPEC-014-R3 (`aitools doctor` Diagnostic Analyzer):** `aitools doctor` / `aitools logs analyze` MUST aggregate `.jsonl` logs into per-span `P50`/`P95`/`P99` percentiles, per-plugin error/anomaly counts, and actionable remediation hints.

## 3. Diagnostic-to-Trace Closed Loop

- **SPEC-014-R4 (`import-diag` Reproducer Generator):** `aitools harness import-diag <diag.zip> [--trace-id <id>] --out <repro.ztrace>` MUST reconstruct a playable, `seq`-ordered `.ztrace` fixture from `events.jsonl`.

## 4. Implementation & Verification Status (Updated 2026-11-16)

- **Status:** Implemented and verified:
  - `SPEC-014-R1`: `trace_id` propagated across `handle_wake_hotkey`, `handle_sync_request` (`X-AITools-Trace-Id` + body `trace_id`), `invoke_preload_list_feature`, `execute_dag`, `telemetry_log` (`x-aitools-trace-id` header), and `ingestFrontendLog` -> `dsh_ingest_frontend_log`.
  - `SPEC-014-R2`: `ChronologicalJsonlLayer::on_close` attaches `active_plugin_count` and `slowest_subspan` when `perf_anomaly: true` — verified by `spec_014_r2_enriches_perf_anomaly_fields` (`crates/aitools-telemetry/src/lib.rs`).
  - `SPEC-014-R3`: `analyze_records` / `TelemetryHandle::analyze_logs` (`aitools doctor`, `aitools logs analyze`) — verified by `spec_014_r2_analyzes_span_percentiles_and_slow_plugins` (`crates/aitools-telemetry/src/lib.rs`).
  - `SPEC-014-R4`: `convert_diag_zip_to_ztrace` & `convert_diag_zip_to_ztrace_filtered` (`aitools harness import-diag --trace-id`) — verified by `spec_014_r3_imports_diag_zip_to_ztrace` (`crates/aitools-telemetry/src/lib.rs`).
