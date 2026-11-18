//! `aitools-telemetry` — Implements SPEC-010 (Chronological & Continuous Log Management).
//!
//! Guarantees:
//! - Strictly monotonic `seq` + UTC microsecond `ts` across async tasks, threads, and webviews
//! - Automatic `SPEC-000` latency budget anomaly detection on span close
//! - Secret & optional clipboard redaction prior to persistence or export
//! - Rolling JSONL persistence (20 MB per file, 7-day / 150 MB retention) + synchronous panic flush
//! - Chronological `.log` timeline and `.zip` diagnostic bundle export

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::fmt::Write as FmtWrite;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use thiserror::Error;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Record};
use tracing::{Event, Id, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB per JSONL file (SPEC-010-R3)
const MAX_TOTAL_LOG_BYTES: u64 = 150 * 1024 * 1024; // 150 MB total retention cap
const MAX_RETENTION_DAYS: i64 = 7;
const RING_BUFFER_CAPACITY: usize = 10_000;

static GLOBAL_SEQ: AtomicU64 = AtomicU64::new(0);
static GLOBAL_TELEMETRY: OnceLock<Arc<TelemetryHandle>> = OnceLock::new();

/// Errors returned by telemetry initialization, querying, and bundle export.
#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("telemetry subsystem is already initialized")]
    AlreadyInitialized,
    #[error("telemetry subsystem has not been initialized")]
    NotInitialized,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

/// Structured chronological log record persisted to rolling JSONL files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogRecord {
    pub seq: u64,
    pub ts: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_us: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub perf_anomaly: bool,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

/// Configuration for initializing the telemetry subsystem.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub log_dir: PathBuf,
    pub default_filter: String,
    pub redact_clipboard: bool,
    pub install_panic_hook: bool,
}

impl TelemetryConfig {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_dir: log_dir.into(),
            default_filter: "info,aitools=debug".to_string(),
            redact_clipboard: true,
            install_panic_hook: true,
        }
    }
}

/// Filter criteria for querying or exporting persisted [`LogRecord`] entries.
#[derive(Debug, Clone, Default)]
pub struct ExportFilter {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub trace_id: Option<String>,
    pub plugin_id: Option<String>,
    pub anomalies_only: bool,
}

/// Aggregate counters returned by diagnostic export operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticSummary {
    pub exported_at: String,
    pub total_records: usize,
    pub error_count: usize,
    pub warn_count: usize,
    pub anomaly_count: usize,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
}

/// Per-span latency percentile summary (`SPEC-014-R2`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpanPercentileStats {
    pub span: String,
    pub count: usize,
    pub anomaly_count: usize,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
}

/// Per-plugin health & latency summary (`SPEC-014-R2`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginHealthStats {
    pub plugin_id: String,
    pub event_count: usize,
    pub error_count: usize,
    pub anomaly_count: usize,
}

/// Structured output from `aitools doctor` / `aitools logs analyze` (`SPEC-014-R2`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorReport {
    pub total_records: usize,
    pub error_count: usize,
    pub anomaly_count: usize,
    pub spans: Vec<SpanPercentileStats>,
    pub plugins: Vec<PluginHealthStats>,
    pub recommendations: Vec<String>,
}

/// Analyzes chronological [`LogRecord`]s to compute P50/P95/P99 and plugin health (`SPEC-014-R2`).
pub fn analyze_records(records: &[LogRecord]) -> DoctorReport {
    use std::collections::BTreeMap;

    let mut span_durations: BTreeMap<String, (usize, Vec<u64>)> = BTreeMap::new();
    let mut plugin_stats: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut total_errors = 0usize;
    let mut total_anomalies = 0usize;

    for rec in records {
        let is_err = matches!(rec.level.as_str(), "ERROR" | "FATAL");
        if is_err {
            total_errors += 1;
        }
        if rec.perf_anomaly {
            total_anomalies += 1;
        }
        if let Some(span_name) = &rec.span {
            if let Some(dur) = rec.duration_us {
                let entry = span_durations
                    .entry(span_name.clone())
                    .or_insert_with(|| (0, Vec::new()));
                if rec.perf_anomaly {
                    entry.0 += 1;
                }
                entry.1.push(dur);
            }
        }
        if let Some(pid) = &rec.plugin_id {
            let entry = plugin_stats.entry(pid.clone()).or_insert((0, 0, 0));
            entry.0 += 1;
            if is_err {
                entry.1 += 1;
            }
            if rec.perf_anomaly {
                entry.2 += 1;
            }
        }
    }

    let mut spans = Vec::with_capacity(span_durations.len());
    for (span, (anomaly_count, mut durs)) in span_durations {
        durs.sort_unstable();
        let len = durs.len();
        let p50_us = durs[len / 2];
        let p95_us = durs[(len * 95) / 100];
        let p99_us = durs[(len * 99) / 100];
        spans.push(SpanPercentileStats {
            span,
            count: len,
            anomaly_count,
            p50_us,
            p95_us,
            p99_us,
        });
    }

    let mut plugins: Vec<PluginHealthStats> = plugin_stats
        .into_iter()
        .map(
            |(plugin_id, (event_count, error_count, anomaly_count))| PluginHealthStats {
                plugin_id,
                event_count,
                error_count,
                anomaly_count,
            },
        )
        .collect();
    plugins.sort_by(|a, b| {
        b.anomaly_count
            .cmp(&a.anomaly_count)
            .then_with(|| b.error_count.cmp(&a.error_count))
            .then_with(|| a.plugin_id.cmp(&b.plugin_id))
    });

    let mut recommendations = Vec::new();
    for s in &spans {
        if s.anomaly_count > 0 {
            recommendations.push(format!(
                "Span '{}' recorded {} SPEC-000 budget anomalies (P99={}us).",
                s.span, s.anomaly_count, s.p99_us
            ));
        }
    }
    for p in &plugins {
        if p.error_count > 0 || p.anomaly_count > 0 {
            recommendations.push(format!(
                "Plugin '{}' reported {} errors and {} latency anomalies.",
                p.plugin_id, p.error_count, p.anomaly_count
            ));
        }
    }

    DoctorReport {
        total_records: records.len(),
        error_count: total_errors,
        anomaly_count: total_anomalies,
        spans,
        plugins,
        recommendations,
    }
}

/// Extracts `events.jsonl` from an `aitools-diag-*.zip` bundle into a `.ztrace` file (`SPEC-014-R3`).
///
/// # Errors
///
/// Returns [`TelemetryError::Io`] if reading the archive or writing `ztrace_out` fails,
/// [`TelemetryError::Zip`] if `events.jsonl` is missing or the zip is corrupt, or
/// [`TelemetryError::Json`] if re-serializing records fails.
pub fn convert_diag_zip_to_ztrace(
    zip_path: impl AsRef<Path>,
    ztrace_out: impl AsRef<Path>,
) -> Result<usize, TelemetryError> {
    convert_diag_zip_to_ztrace_filtered(zip_path, ztrace_out, None)
}

/// Extracts `events.jsonl` from an `aitools-diag-*.zip` bundle into a `.ztrace` file,
/// optionally filtering records to a single `trace_id` (`SPEC-014-R4`).
///
/// # Errors
///
/// Returns [`TelemetryError::Io`] if reading the archive or writing `ztrace_out` fails,
/// [`TelemetryError::Zip`] if `events.jsonl` is missing or the zip is corrupt, or
/// [`TelemetryError::Json`] if re-serializing records fails.
pub fn convert_diag_zip_to_ztrace_filtered(
    zip_path: impl AsRef<Path>,
    ztrace_out: impl AsRef<Path>,
    trace_id: Option<&str>,
) -> Result<usize, TelemetryError> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let events = archive.by_name("events.jsonl")?;
    let mut records = Vec::new();
    for line in BufReader::new(events).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(rec) = serde_json::from_str::<LogRecord>(&line) {
            if let Some(wanted) = trace_id {
                if rec.trace_id.as_deref() != Some(wanted) {
                    continue;
                }
            }
            records.push(rec);
        }
    }
    records.sort_by_key(|r| r.seq);
    records.dedup_by_key(|r| r.seq);

    let mut out = String::new();
    for rec in &records {
        out.push_str(&serde_json::to_string(rec)?);
        out.push('\n');
    }
    fs::write(ztrace_out, out)?;
    Ok(records.len())
}

struct Redactor {
    secret_patterns: Vec<Regex>,
    url_auth_pattern: Regex,
    redact_clipboard: AtomicBool,
}

impl Redactor {
    fn new(redact_clipboard: bool) -> Self {
        let secret_patterns = vec![
            // OpenAI / Anthropic / Stripe style keys
            Regex::new(r"\bsk-(?:proj-|ant-)?[A-Za-z0-9_\-]{16,}\b").expect("valid regex"),
            // Google API keys
            Regex::new(r"\bAIza[0-9A-Za-z\-_]{20,}\b").expect("valid regex"),
            // GitHub PATs
            Regex::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b").expect("valid regex"),
            // Slack tokens
            Regex::new(r"\bxox[baprs]-[0-9A-Za-z\-]{16,}\b").expect("valid regex"),
            // HTTP Bearer tokens
            Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9\-._~+/]+=*").expect("valid regex"),
        ];
        let url_auth_pattern =
            Regex::new(r"(https?://)([^:@/\s]+):([^@/\s]+)@").expect("valid regex");

        Self {
            secret_patterns,
            url_auth_pattern,
            redact_clipboard: AtomicBool::new(redact_clipboard),
        }
    }

    fn set_redact_clipboard(&self, enabled: bool) {
        self.redact_clipboard.store(enabled, Ordering::Relaxed);
    }

    fn scrub_str(&self, input: &str) -> String {
        let mut out = self
            .url_auth_pattern
            .replace_all(input, "${1}[REDACTED]:[REDACTED]@")
            .into_owned();
        for pattern in &self.secret_patterns {
            out = pattern.replace_all(&out, "[REDACTED]").into_owned();
        }
        out
    }

    fn scrub_field(&self, key: &str, value: Value) -> Value {
        if self.redact_clipboard.load(Ordering::Relaxed)
            && matches!(
                key,
                "clipboard" | "clipboard_text" | "selection" | "selected_text"
            )
        {
            return match value {
                Value::String(s) => Value::String(format!("<redacted:len={}>", s.len())),
                other => Value::String(format!("<redacted:type={}>", json_type_name(&other))),
            };
        }

        let lower_key = key.to_ascii_lowercase();
        if lower_key.contains("api_key")
            || lower_key.contains("secret")
            || lower_key.contains("password")
            || lower_key.contains("token")
            || lower_key == "authorization"
        {
            return Value::String("[REDACTED]".to_string());
        }

        match value {
            Value::String(s) => Value::String(self.scrub_str(&s)),
            Value::Array(arr) => Value::Array(
                arr.into_iter()
                    .map(|item| self.scrub_field(key, item))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| {
                        let clean = self.scrub_field(&k, v);
                        (k, clean)
                    })
                    .collect(),
            ),
            primitive => primitive,
        }
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

struct RollingJsonlWriter {
    log_dir: PathBuf,
    current_date: String,
    current_part: u32,
    current_bytes: u64,
    writer: BufWriter<File>,
}

impl RollingJsonlWriter {
    fn open(log_dir: &Path) -> Result<Self, TelemetryError> {
        fs::create_dir_all(log_dir)?;
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let (part, path, existing_bytes) = Self::resolve_active_file(log_dir, &today)?;
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut instance = Self {
            log_dir: log_dir.to_path_buf(),
            current_date: today,
            current_part: part,
            current_bytes: existing_bytes,
            writer: BufWriter::with_capacity(64 * 1024, file),
        };
        instance.prune_old_files()?;
        Ok(instance)
    }

    fn resolve_active_file(
        log_dir: &Path,
        date: &str,
    ) -> Result<(u32, PathBuf, u64), TelemetryError> {
        let mut part = 0u32;
        loop {
            let path = Self::file_path(log_dir, date, part);
            if !path.exists() {
                return Ok((part, path, 0));
            }
            let len = fs::metadata(&path)?.len();
            if len < MAX_FILE_BYTES {
                return Ok((part, path, len));
            }
            part += 1;
        }
    }

    fn file_path(log_dir: &Path, date: &str, part: u32) -> PathBuf {
        if part == 0 {
            log_dir.join(format!("aitools-{date}.jsonl"))
        } else {
            log_dir.join(format!("aitools-{date}.{part}.jsonl"))
        }
    }

    fn write_record(&mut self, record: &LogRecord) -> Result<(), TelemetryError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if today != self.current_date || self.current_bytes >= MAX_FILE_BYTES {
            self.writer.flush()?;
            if today != self.current_date {
                self.current_date = today;
                self.current_part = 0;
            } else {
                self.current_part += 1;
            }
            let next_path = Self::file_path(&self.log_dir, &self.current_date, self.current_part);
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&next_path)?;
            self.current_bytes = fs::metadata(&next_path).map(|m| m.len()).unwrap_or(0);
            self.writer = BufWriter::with_capacity(64 * 1024, file);
            let _ = self.prune_old_files();
        }

        let line = serde_json::to_string(record)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.current_bytes += (line.len() + 1) as u64;

        // Flush every record immediately so startup/INFO spans are durable before SIGTERM or doctor reads.
        self.writer.flush()?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TelemetryError> {
        self.writer.flush()?;
        Ok(())
    }

    fn prune_old_files(&mut self) -> Result<(), TelemetryError> {
        let cutoff = Utc::now() - Duration::days(MAX_RETENTION_DAYS);
        let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();

        for entry in fs::read_dir(&self.log_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = entry.metadata()?;
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let modified_dt: DateTime<Utc> = modified.into();
            if modified_dt < cutoff {
                let _ = fs::remove_file(&path);
                continue;
            }
            entries.push((path, meta.len(), modified));
        }

        // Enforce 150 MB total directory cap (oldest first)
        entries.sort_by_key(|(_, _, modified)| *modified);
        let mut total_bytes: u64 = entries.iter().map(|(_, len, _)| *len).sum();
        for (path, len, _) in entries {
            if total_bytes <= MAX_TOTAL_LOG_BYTES {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total_bytes = total_bytes.saturating_sub(len);
            }
        }
        Ok(())
    }
}

impl Drop for RollingJsonlWriter {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

/// Thread-safe handle to the telemetry writer, ring buffer, and log exporter.
pub struct TelemetryHandle {
    log_dir: PathBuf,
    writer: Mutex<RollingJsonlWriter>,
    ring_buffer: Mutex<VecDeque<LogRecord>>,
    redactor: Redactor,
    active_plugin_count: AtomicUsize,
}

impl TelemetryHandle {
    /// Records an external log event (e.g., bridged from a Svelte/Vue or plugin Webview console).
    pub fn ingest_external(
        &self,
        level: &str,
        source: &str,
        plugin_id: Option<String>,
        trace_id: Option<String>,
        msg: &str,
        fields: Map<String, Value>,
    ) {
        let scrubbed_fields = fields
            .into_iter()
            .map(|(k, v)| {
                let clean = self.redactor.scrub_field(&k, v);
                (k, clean)
            })
            .collect();

        let record = LogRecord {
            seq: next_seq(),
            ts: now_micros_utc(),
            level: level.to_ascii_uppercase(),
            trace_id,
            source: source.to_string(),
            plugin_id,
            span: None,
            duration_us: None,
            perf_anomaly: false,
            msg: self.redactor.scrub_str(msg),
            fields: scrubbed_fields,
        };
        self.record(record);
    }

    /// Toggles redaction of clipboard text fields before persistence.
    pub fn set_redact_clipboard(&self, enabled: bool) {
        self.redactor.set_redact_clipboard(enabled);
    }

    /// Updates the active plugin count attached to performance anomaly snapshots (`SPEC-014-R2`).
    pub fn set_active_plugin_count(&self, count: usize) {
        self.active_plugin_count.store(count, Ordering::Relaxed);
    }

    /// Flushes buffered log records to the underlying JSONL file.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`] if flushing the buffered writer fails.
    ///
    /// # Panics
    ///
    /// Panics if the internal writer mutex is poisoned.
    pub fn flush(&self) -> Result<(), TelemetryError> {
        let mut guard = self.writer.lock().expect("telemetry writer mutex poisoned");
        guard.flush()
    }

    /// Returns up to `limit` most recent [`LogRecord`] entries from the in-memory ring buffer.
    ///
    /// # Panics
    ///
    /// Panics if the internal ring-buffer mutex is poisoned.
    pub fn recent_records(&self, limit: usize) -> Vec<LogRecord> {
        let guard = self.ring_buffer.lock().expect("ring buffer mutex poisoned");
        let skip = guard.len().saturating_sub(limit);
        guard.iter().skip(skip).cloned().collect()
    }

    /// Reads persisted JSONL files in ascending `seq` order matching `filter` (SPEC-010-R6).
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`] if reading the log directory or JSONL files fails.
    pub fn query_records(&self, filter: &ExportFilter) -> Result<Vec<LogRecord>, TelemetryError> {
        let _ = self.flush();
        let mut files: Vec<PathBuf> = fs::read_dir(&self.log_dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .collect();
        files.sort();

        let mut records = Vec::new();
        for path in files {
            let file = File::open(&path)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(rec) = serde_json::from_str::<LogRecord>(&line) else {
                    continue;
                };
                if !matches_filter(&rec, filter) {
                    continue;
                }
                records.push(rec);
            }
        }

        records.sort_by_key(|r| r.seq);
        records.dedup_by_key(|r| r.seq);
        Ok(records)
    }

    /// Exports an aligned human-readable chronological `.log` file (SPEC-010-R6).
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`] if querying persisted logs or writing `output_path` fails.
    pub fn export_chronological_log(
        &self,
        output_path: impl AsRef<Path>,
        filter: &ExportFilter,
    ) -> Result<DiagnosticSummary, TelemetryError> {
        let records = self.query_records(filter)?;
        let (timeline, summary) = render_timeline_and_summary(&records);
        fs::write(output_path, timeline)?;
        Ok(summary)
    }

    /// Exports a `.zip` diagnostic bundle containing `events.jsonl`, `timeline.log`, and `summary.json`.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`], [`TelemetryError::Json`], or [`TelemetryError::Zip`] if
    /// querying logs, serializing entries, or writing the `.zip` archive fails.
    pub fn export_diagnostic_bundle(
        &self,
        zip_path: impl AsRef<Path>,
        filter: &ExportFilter,
    ) -> Result<DiagnosticSummary, TelemetryError> {
        let records = self.query_records(filter)?;
        let (timeline, summary) = render_timeline_and_summary(&records);

        let mut jsonl_buf = String::new();
        for rec in &records {
            jsonl_buf.push_str(&serde_json::to_string(rec)?);
            jsonl_buf.push('\n');
        }
        let summary_json = serde_json::to_string_pretty(&summary)?;

        let file = File::create(zip_path)?;
        let mut zip = ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("events.jsonl", options)?;
        zip.write_all(jsonl_buf.as_bytes())?;

        zip.start_file("timeline.log", options)?;
        zip.write_all(timeline.as_bytes())?;

        zip.start_file("summary.json", options)?;
        zip.write_all(summary_json.as_bytes())?;

        zip.finish()?;
        Ok(summary)
    }

    /// Opens a standalone handle without installing the global `tracing` subscriber (`SPEC-014`).
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`] if the log directory or rolling JSONL file cannot be opened.
    pub fn open_standalone(config: TelemetryConfig) -> Result<Arc<Self>, TelemetryError> {
        let writer = RollingJsonlWriter::open(&config.log_dir)?;
        Ok(Arc::new(Self {
            log_dir: config.log_dir,
            writer: Mutex::new(writer),
            ring_buffer: Mutex::new(VecDeque::with_capacity(RING_BUFFER_CAPACITY)),
            redactor: Redactor::new(config.redact_clipboard),
            active_plugin_count: AtomicUsize::new(0),
        }))
    }

    /// Aggregates span percentiles and per-plugin health diagnostics (`SPEC-014-R2`).
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryError::Io`] if querying persisted logs fails.
    pub fn analyze_logs(&self, filter: &ExportFilter) -> Result<DoctorReport, TelemetryError> {
        let records = self.query_records(filter)?;
        Ok(analyze_records(&records))
    }

    fn record(&self, record: LogRecord) {
        {
            let mut ring = self.ring_buffer.lock().expect("ring buffer mutex poisoned");
            if ring.len() >= RING_BUFFER_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(record.clone());
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_record(&record);
        }
    }
}

fn next_seq() -> u64 {
    GLOBAL_SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

fn now_micros_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn matches_filter(rec: &LogRecord, filter: &ExportFilter) -> bool {
    if filter.anomalies_only && !rec.perf_anomaly {
        return false;
    }
    if let Some(ref wanted_trace) = filter.trace_id {
        if rec.trace_id.as_deref() != Some(wanted_trace.as_str()) {
            return false;
        }
    }
    if let Some(ref wanted_plugin) = filter.plugin_id {
        if rec.plugin_id.as_deref() != Some(wanted_plugin.as_str()) {
            return false;
        }
    }
    if filter.since.is_some() || filter.until.is_some() {
        let Ok(parsed) = DateTime::parse_from_rfc3339(&rec.ts) else {
            return false;
        };
        let ts_utc = parsed.with_timezone(&Utc);
        if let Some(since) = filter.since {
            if ts_utc < since {
                return false;
            }
        }
        if let Some(until) = filter.until {
            if ts_utc > until {
                return false;
            }
        }
    }
    true
}

fn render_timeline_and_summary(records: &[LogRecord]) -> (String, DiagnosticSummary) {
    let mut out = String::new();
    let mut error_count = 0usize;
    let mut warn_count = 0usize;
    let mut anomaly_count = 0usize;

    for rec in records {
        if matches!(rec.level.as_str(), "ERROR" | "FATAL") {
            error_count += 1;
        } else if rec.level == "WARN" {
            warn_count += 1;
        }
        if rec.perf_anomaly {
            anomaly_count += 1;
        }

        let trace_col = rec.trace_id.as_deref().unwrap_or("-");
        let plugin_col = rec.plugin_id.as_deref().unwrap_or("host");
        let dur_col = rec
            .duration_us
            .map(|d| {
                format!(
                    " ({d}us{})",
                    if rec.perf_anomaly { " !ANOMALY" } else { "" }
                )
            })
            .unwrap_or_default();

        let _ = writeln!(
            out,
            "[{:08}] {} {:<5} [trace={}] [plugin={}] {}{}: {} {}",
            rec.seq,
            rec.ts,
            rec.level,
            trace_col,
            plugin_col,
            rec.source,
            dur_col,
            rec.msg,
            if rec.fields.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&rec.fields).unwrap_or_default()
            }
        );
    }

    let summary = DiagnosticSummary {
        exported_at: now_micros_utc(),
        total_records: records.len(),
        error_count,
        warn_count,
        anomaly_count,
        first_seq: records.first().map(|r| r.seq),
        last_seq: records.last().map(|r| r.seq),
    };
    (out, summary)
}

/// Checks `SPEC-000` P99 latency ceilings (in microseconds) by span name.
fn span_budget_us(span_name: &str) -> Option<u64> {
    match span_name {
        "wake_to_paint" | "pre_focus_capture" => Some(15_000),
        "db_sync" => Some(5_000),
        "quickjs_cold_start" => Some(10_000),
        "wasm_instantiate" => Some(5_000),
        _ => None,
    }
}

struct SpanState {
    started_at: Instant,
    trace_id: Option<String>,
    plugin_id: Option<String>,
    slowest_subspan: Option<(String, u64)>,
}

#[derive(Default)]
struct JsonFieldVisitor {
    message: Option<String>,
    trace_id: Option<String>,
    plugin_id: Option<String>,
    fields: Map<String, Value>,
}

impl Visit for JsonFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "trace_id" => self.trace_id = Some(value.to_string()),
            "plugin_id" => self.plugin_id = Some(value.to_string()),
            name => {
                self.fields
                    .insert(name.to_string(), Value::String(value.to_string()));
            }
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{value:?}");
        let trimmed = formatted.trim_matches('"').to_string();
        match field.name() {
            "message" => self.message = Some(trimmed),
            "trace_id" => self.trace_id = Some(trimmed),
            "plugin_id" => self.plugin_id = Some(trimmed),
            name => {
                self.fields.insert(name.to_string(), Value::String(trimmed));
            }
        }
    }
}

struct ChronologicalJsonlLayer {
    handle: Arc<TelemetryHandle>,
}

impl<S> Layer<S> for ChronologicalJsonlLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = JsonFieldVisitor::default();
        attrs.record(&mut visitor);

        // Inherit trace_id / plugin_id from parent span if omitted
        let mut inherited_trace = visitor.trace_id;
        let mut inherited_plugin = visitor.plugin_id;
        if let Some(span) = ctx.span(id) {
            if let Some(parent) = span.parent() {
                if let Some(parent_state) = parent.extensions().get::<SpanState>() {
                    if inherited_trace.is_none() {
                        inherited_trace = parent_state.trace_id.clone();
                    }
                    if inherited_plugin.is_none() {
                        inherited_plugin = parent_state.plugin_id.clone();
                    }
                }
            }
            span.extensions_mut().insert(SpanState {
                started_at: Instant::now(),
                trace_id: inherited_trace,
                plugin_id: inherited_plugin,
                slowest_subspan: None,
            });
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let mut visitor = JsonFieldVisitor::default();
        values.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            if let Some(state) = span.extensions_mut().get_mut::<SpanState>() {
                if visitor.trace_id.is_some() {
                    state.trace_id = visitor.trace_id;
                }
                if visitor.plugin_id.is_some() {
                    state.plugin_id = visitor.plugin_id;
                }
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = JsonFieldVisitor::default();
        event.record(&mut visitor);

        let mut trace_id = visitor.trace_id;
        let mut plugin_id = visitor.plugin_id;
        let mut active_span_name = None;

        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                if active_span_name.is_none() {
                    active_span_name = Some(span.name().to_string());
                }
                if let Some(state) = span.extensions().get::<SpanState>() {
                    if trace_id.is_none() {
                        trace_id = state.trace_id.clone();
                    }
                    if plugin_id.is_none() {
                        plugin_id = state.plugin_id.clone();
                    }
                }
                if trace_id.is_some() && plugin_id.is_some() {
                    break;
                }
            }
        }

        let meta: &Metadata<'_> = event.metadata();
        let scrubbed_fields = visitor
            .fields
            .into_iter()
            .map(|(k, v)| {
                let clean = self.handle.redactor.scrub_field(&k, v);
                (k, clean)
            })
            .collect();

        let raw_msg = visitor.message.unwrap_or_else(|| meta.name().to_string());
        let record = LogRecord {
            seq: next_seq(),
            ts: now_micros_utc(),
            level: level_str(*meta.level()).to_string(),
            trace_id,
            source: meta.target().to_string(),
            plugin_id,
            span: active_span_name,
            duration_us: None,
            perf_anomaly: false,
            msg: self.handle.redactor.scrub_str(&raw_msg),
            fields: scrubbed_fields,
        };

        self.handle.record(record);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let (duration_us, trace_id, plugin_id, child_slowest) = {
            let extensions = span.extensions();
            let Some(state) = extensions.get::<SpanState>() else {
                return;
            };
            (
                state.started_at.elapsed().as_micros() as u64,
                state.trace_id.clone(),
                state.plugin_id.clone(),
                state.slowest_subspan.clone(),
            )
        };
        let span_name = span.name();
        if let Some(parent) = span.parent() {
            if let Some(parent_state) = parent.extensions_mut().get_mut::<SpanState>() {
                let replace = parent_state
                    .slowest_subspan
                    .as_ref()
                    .is_none_or(|(_, prev_us)| duration_us > *prev_us);
                if replace {
                    parent_state.slowest_subspan = Some((span_name.to_string(), duration_us));
                }
            }
        }
        let budget = span_budget_us(span_name);
        let is_anomaly = budget.is_some_and(|max_us| duration_us > max_us);

        if is_anomaly || budget.is_some() {
            let mut fields = Map::new();
            if is_anomaly {
                fields.insert(
                    "active_plugin_count".to_string(),
                    Value::Number(
                        self.handle
                            .active_plugin_count
                            .load(Ordering::Relaxed)
                            .into(),
                    ),
                );
                let slowest_label = match child_slowest {
                    Some((sub_name, sub_us)) => format!("{sub_name}:{sub_us}us"),
                    None => format!("{span_name}:{duration_us}us"),
                };
                fields.insert("slowest_subspan".to_string(), Value::String(slowest_label));
            }

            let record = LogRecord {
                seq: next_seq(),
                ts: now_micros_utc(),
                level: if is_anomaly {
                    "WARN".to_string()
                } else {
                    "DEBUG".to_string()
                },
                trace_id,
                source: span.metadata().target().to_string(),
                plugin_id,
                span: Some(span_name.to_string()),
                duration_us: Some(duration_us),
                perf_anomaly: is_anomaly,
                msg: if is_anomaly {
                    format!(
                        "Span '{span_name}' exceeded SPEC-000 budget ({duration_us}us > {}us)",
                        budget.unwrap_or(0)
                    )
                } else {
                    format!("Span '{span_name}' completed in {duration_us}us")
                },
                fields,
            };
            self.handle.record(record);
        }
    }
}

fn level_str(level: Level) -> &'static str {
    match level {
        Level::TRACE => "TRACE",
        Level::DEBUG => "DEBUG",
        Level::INFO => "INFO",
        Level::WARN => "WARN",
        Level::ERROR => "ERROR",
    }
}

/// Initializes the global `tracing` subscriber and returns a [`TelemetryHandle`] for querying/exporting.
///
/// # Errors
///
/// Returns [`TelemetryError::Io`] if opening the log directory fails, or
/// [`TelemetryError::AlreadyInitialized`] if global telemetry or a `tracing` subscriber is
/// already installed.
pub fn init(config: TelemetryConfig) -> Result<Arc<TelemetryHandle>, TelemetryError> {
    let writer = RollingJsonlWriter::open(&config.log_dir)?;
    let handle = Arc::new(TelemetryHandle {
        log_dir: config.log_dir,
        writer: Mutex::new(writer),
        ring_buffer: Mutex::new(VecDeque::with_capacity(RING_BUFFER_CAPACITY)),
        redactor: Redactor::new(config.redact_clipboard),
        active_plugin_count: AtomicUsize::new(0),
    });

    if GLOBAL_TELEMETRY.set(Arc::clone(&handle)).is_err() {
        return Err(TelemetryError::AlreadyInitialized);
    }

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.default_filter));

    let layer = ChronologicalJsonlLayer {
        handle: Arc::clone(&handle),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(layer)
        .try_init()
        .map_err(|_| TelemetryError::AlreadyInitialized)?;

    if config.install_panic_hook {
        install_panic_hook();
    }

    Ok(handle)
}

/// Returns the global [`TelemetryHandle`] installed by [`init`].
///
/// # Errors
///
/// Returns [`TelemetryError::NotInitialized`] if [`init`] has not been called yet.
pub fn global() -> Result<Arc<TelemetryHandle>, TelemetryError> {
    GLOBAL_TELEMETRY
        .get()
        .cloned()
        .ok_or(TelemetryError::NotInitialized)
}

fn install_panic_hook() {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if let Some(telemetry) = GLOBAL_TELEMETRY.get() {
            let location = panic_info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown".to_string());
            let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_string()
            };

            let mut fields = Map::new();
            fields.insert("location".to_string(), Value::String(location));

            telemetry.record(LogRecord {
                seq: next_seq(),
                ts: now_micros_utc(),
                level: "FATAL".to_string(),
                trace_id: None,
                source: "panic_hook".to_string(),
                plugin_id: None,
                span: None,
                duration_us: None,
                perf_anomaly: false,
                msg: telemetry
                    .redactor
                    .scrub_str(&format!("Process panic: {payload}")),
                fields,
            });
            let _ = telemetry.flush();
        }
        prev_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_handle(dir: &TempDir) -> Arc<TelemetryHandle> {
        let writer = RollingJsonlWriter::open(dir.path()).expect("open rolling writer");
        Arc::new(TelemetryHandle {
            log_dir: dir.path().to_path_buf(),
            writer: Mutex::new(writer),
            ring_buffer: Mutex::new(VecDeque::with_capacity(64)),
            redactor: Redactor::new(true),
            active_plugin_count: AtomicUsize::new(0),
        })
    }

    #[test]
    fn spec_010_r1_strictly_increasing_seq_order() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);

        for i in 0..25 {
            handle.ingest_external(
                "info",
                "test::source",
                Some("demo-plugin".into()),
                Some("trace-1".into()),
                &format!("step {i}"),
                Map::new(),
            );
        }

        let records = handle.query_records(&ExportFilter::default()).unwrap();
        assert_eq!(records.len(), 25);
        for pair in records.windows(2) {
            assert!(pair[0].seq < pair[1].seq, "seq must be strictly monotonic");
        }
    }

    #[test]
    fn spec_010_r5_redacts_secrets_and_clipboard() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);

        let mut fields = Map::new();
        fields.insert(
            "clipboard_text".into(),
            Value::String("sensitive user text".into()),
        );
        fields.insert("api_key".into(), Value::String("raw-secret".into()));

        handle.ingest_external(
            "warn",
            "aitools_ai::router",
            None,
            Some("trace-secret".into()),
            "Failed auth with Bearer eyJhbGciOiJIUzI1NiJ9.abc.def and key sk-proj-1234567890abcdefghij",
            fields,
        );

        let records = handle.query_records(&ExportFilter::default()).unwrap();
        let rec = &records[0];
        assert!(!rec.msg.contains("sk-proj-1234567890"));
        assert!(!rec.msg.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(rec.msg.contains("[REDACTED]"));
        assert_eq!(
            rec.fields.get("clipboard_text").unwrap(),
            "<redacted:len=19>"
        );
        assert_eq!(rec.fields.get("api_key").unwrap(), "[REDACTED]");
    }

    #[test]
    fn spec_010_r6_exports_chronological_timeline_and_zip_bundle() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);

        handle.ingest_external(
            "info",
            "host::wake",
            None,
            Some("wake-01".into()),
            "Hotkey triggered",
            Map::new(),
        );
        handle.ingest_external(
            "error",
            "plugin::translate",
            Some("translate".into()),
            Some("wake-01".into()),
            "Upstream timeout",
            Map::new(),
        );

        let log_out = dir.path().join("timeline.log");
        let zip_out = dir.path().join("diag.zip");
        let filter = ExportFilter {
            trace_id: Some("wake-01".into()),
            ..Default::default()
        };

        let summary = handle.export_chronological_log(&log_out, &filter).unwrap();
        assert_eq!(summary.total_records, 2);
        assert_eq!(summary.error_count, 1);

        let timeline_txt = fs::read_to_string(&log_out).unwrap();
        assert!(timeline_txt.contains("Hotkey triggered"));
        assert!(timeline_txt.contains("Upstream timeout"));

        let zip_summary = handle.export_diagnostic_bundle(&zip_out, &filter).unwrap();
        assert_eq!(zip_summary.total_records, 2);

        let zip_file = File::open(&zip_out).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        assert!(archive.by_name("events.jsonl").is_ok());
        assert!(archive.by_name("timeline.log").is_ok());
        assert!(archive.by_name("summary.json").is_ok());
    }

    #[test]
    fn spec_014_r2_analyzes_span_percentiles_and_slow_plugins() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);
        for (dur, anomaly) in [(1000, false), (2000, false), (18000, true)] {
            handle.record(LogRecord {
                seq: next_seq(),
                ts: now_micros_utc(),
                level: if anomaly { "WARN" } else { "DEBUG" }.into(),
                trace_id: Some("wake-bench".into()),
                source: "host::wake".into(),
                plugin_id: Some("slow-plugin".into()),
                span: Some("wake_to_paint".into()),
                duration_us: Some(dur),
                perf_anomaly: anomaly,
                msg: "measured".into(),
                fields: Map::new(),
            });
        }
        let report = handle.analyze_logs(&ExportFilter::default()).unwrap();
        assert_eq!(report.total_records, 3);
        assert_eq!(report.anomaly_count, 1);
        assert_eq!(report.spans[0].p50_us, 2000);
        assert_eq!(report.spans[0].p99_us, 18000);
        assert_eq!(report.plugins[0].plugin_id, "slow-plugin");
        assert!(!report.recommendations.is_empty());
    }

    #[test]
    fn spec_014_r3_imports_diag_zip_to_ztrace() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);
        handle.ingest_external("info", "wake", None, Some("t1".into()), "start", Map::new());
        handle.ingest_external("info", "wake", None, Some("t2".into()), "other", Map::new());
        let zip_out = dir.path().join("aitools-diag-test.zip");
        handle
            .export_diagnostic_bundle(&zip_out, &ExportFilter::default())
            .unwrap();

        let ztrace_out = dir.path().join("repro.ztrace");
        let count = convert_diag_zip_to_ztrace(&zip_out, &ztrace_out).unwrap();
        assert_eq!(count, 2);
        let content = fs::read_to_string(&ztrace_out).unwrap();
        assert!(content.contains("start"));

        let filtered_out = dir.path().join("repro-t1.ztrace");
        let filtered_count =
            convert_diag_zip_to_ztrace_filtered(&zip_out, &filtered_out, Some("t1")).unwrap();
        assert_eq!(filtered_count, 1);
        let filtered_content = fs::read_to_string(&filtered_out).unwrap();
        assert!(filtered_content.contains("start"));
        assert!(!filtered_content.contains("other"));
    }

    #[test]
    fn spec_014_r2_enriches_perf_anomaly_fields() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);
        handle.set_active_plugin_count(4);

        let layer = ChronologicalJsonlLayer {
            handle: Arc::clone(&handle),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let parent = tracing::info_span!("db_sync", trace_id = "trace-slow");
            let _pg = parent.enter();
            {
                let child = tracing::info_span!("slow_child_op");
                let _cg = child.enter();
                std::thread::sleep(std::time::Duration::from_millis(7));
            }
        });

        let records = handle.recent_records(10);
        let anomaly = records
            .into_iter()
            .find(|r| r.span.as_deref() == Some("db_sync") && r.perf_anomaly)
            .expect("expected perf_anomaly record for db_sync > 5ms");
        assert_eq!(
            anomaly.fields.get("active_plugin_count"),
            Some(&Value::Number(4.into()))
        );
        let slowest = anomaly
            .fields
            .get("slowest_subspan")
            .and_then(Value::as_str)
            .expect("slowest_subspan must be populated");
        assert!(
            slowest.starts_with("slow_child_op:"),
            "unexpected slowest_subspan: {slowest}"
        );
    }

    #[test]
    fn spec_010_r1_info_records_flush_immediately_to_jsonl_without_error() {
        let dir = TempDir::new().unwrap();
        let handle = test_handle(&dir);

        let layer = ChronologicalJsonlLayer {
            handle: Arc::clone(&handle),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                apps_indexed = 12usize,
                plugins_indexed = 8usize,
                plugin_features_indexed = 16usize,
                "aitools desktop startup indexing complete"
            );
        });

        // Read directly from the JSONL file on disk without calling handle.flush() or emitting ERROR.
        let jsonl_files: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .collect();
        assert_eq!(jsonl_files.len(), 1);
        let raw = fs::read_to_string(&jsonl_files[0]).unwrap();
        assert!(
            !raw.is_empty(),
            "JSONL log file must be non-empty immediately after INFO event"
        );
        let parsed: LogRecord =
            serde_json::from_str(raw.lines().next().expect("at least one JSONL line")).unwrap();
        assert_eq!(parsed.level, "INFO");
        assert_eq!(parsed.msg, "aitools desktop startup indexing complete");
        assert_eq!(
            parsed.fields.get("apps_indexed"),
            Some(&Value::Number(12.into()))
        );
        assert_eq!(
            parsed.fields.get("plugins_indexed"),
            Some(&Value::Number(8.into()))
        );
    }
}
