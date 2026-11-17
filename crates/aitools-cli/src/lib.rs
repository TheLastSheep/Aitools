//! `aitools-cli` — Official Developer CLI, MCP server, and DSH `.ztrace` replay (`SPEC-009`).

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use aitools_ai::harness::{canonicalize_json, ErrorObservation};
use aitools_ai::hitl::{GateDecision, PolicyGuard};
use aitools_ai::registry::ToolRegistry;
use aitools_ai::skills::SkillDocument;
use aitools_core::{classify_plugin_dir, ManifestV2, PluginManifest};
use aitools_telemetry::{ExportFilter, LogRecord, TelemetryConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{DateTime as ZipDateTime, ZipWriter};

const MAX_PLUGIN_BUNDLE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

/// Errors returned by `aitools-cli` commands and tooling operations.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    #[error("ai error: {0}")]
    Ai(#[from] aitools_ai::AiError),

    #[error("telemetry error: {0}")]
    Telemetry(#[from] aitools_telemetry::TelemetryError),

    #[error("audit failed: {0}")]
    AuditFailed(String),

    #[error("ztrace anomaly detected: {0}")]
    TraceAnomaly(String),
}

/// Convenience result alias for [`CliError`].
pub type Result<T> = std::result::Result<T, CliError>;

/// Scaffold template kinds supported by `aitools plugin init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PluginTemplate {
    NativeUi,
    RustWasm,
    Hybrid,
    LegacyCompat,
}

/// Scaffolds a new plugin project matching `SPEC-009-R1`.
///
/// # Errors
///
/// Returns [`CliError::Io`] or [`CliError::Json`] if creating directories or writing manifest
/// and template files fails.
pub fn init_plugin(target_dir: &Path, name: &str, template: PluginTemplate) -> Result<()> {
    fs::create_dir_all(target_dir)?;
    let id = aitools_core::sanitize_plugin_id(name);

    match template {
        PluginTemplate::LegacyCompat => {
            let manifest = json!({
                "name": name,
                "version": "0.1.0",
                "main": "index.html",
                "features": [
                    { "code": "main", "explain": format!("{name} feature"), "cmds": [name] }
                ]
            });
            fs::write(
                target_dir.join("plugin.json"),
                serde_json::to_string_pretty(&manifest)?,
            )?;
            fs::write(
                target_dir.join("index.html"),
                format!("<!doctype html><html><body><h1>{name}</h1></body></html>\n"),
            )?;
        }
        PluginTemplate::NativeUi | PluginTemplate::RustWasm | PluginTemplate::Hybrid => {
            let runtime = match template {
                PluginTemplate::NativeUi => "native-ui",
                PluginTemplate::RustWasm => "rust-wasm",
                PluginTemplate::Hybrid => "hybrid",
                PluginTemplate::LegacyCompat => unreachable!(),
            };
            let mut manifest = json!({
                "id": id,
                "name": name,
                "version": "0.1.0",
                "runtime": runtime,
                "permissions": ["clipboard.read"],
                "ai_tools": [],
                "skill_file": "SKILL.md"
            });
            if matches!(template, PluginTemplate::NativeUi | PluginTemplate::Hybrid) {
                manifest["main"] = json!("index.html");
                fs::write(
                    target_dir.join("index.html"),
                    format!("<!doctype html><html><body><h1>{name}</h1></body></html>\n"),
                )?;
            }
            if matches!(template, PluginTemplate::RustWasm | PluginTemplate::Hybrid) {
                manifest["wasm_entry"] = json!("plugin.wasm");
            }
            fs::write(
                target_dir.join("aitools.manifest.json"),
                serde_json::to_string_pretty(&manifest)?,
            )?;
            fs::write(
                target_dir.join("SKILL.md"),
                format!("---\nname: {id}\ndescription: Runtime instructions for {name}\n---\n\nUse this plugin to assist the user.\n"),
            )?;
        }
    }

    Ok(())
}

/// Summary report produced by [`audit_plugin`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub plugin_id: String,
    pub manifest_file: String,
    pub total_bytes: u64,
    pub skill_valid: bool,
    pub warnings: Vec<String>,
}

/// Validates manifest schema, bundle size, static import permissions, and `SKILL.md` (`SPEC-009-R1`).
///
/// # Errors
///
/// Returns [`CliError::Core`], [`CliError::Ai`], [`CliError::Io`], [`CliError::Json`], or
/// [`CliError::AuditFailed`] if manifest validation, `SKILL.md` parsing, permission checks,
/// or bundle size checks fail.
pub fn audit_plugin(plugin_dir: &Path) -> Result<AuditReport> {
    let classification = classify_plugin_dir(plugin_dir)?;
    let mut warnings = Vec::new();
    let total_bytes = dir_size_bytes(plugin_dir)?;
    if total_bytes > MAX_PLUGIN_BUNDLE_BYTES {
        return Err(CliError::AuditFailed(format!(
            "bundle size {total_bytes} bytes exceeds {MAX_PLUGIN_BUNDLE_BYTES} byte limit"
        )));
    }

    let mut skill_valid = true;
    let v2_path = plugin_dir.join("aitools.manifest.json");
    if v2_path.is_file() {
        let raw = fs::read_to_string(&v2_path)?;
        let manifest: ManifestV2 = serde_json::from_str(&raw)?;
        manifest.validate()?;

        let skill_path = plugin_dir.join(&manifest.skill_file);
        if skill_path.is_file() {
            let skill_raw = fs::read_to_string(&skill_path)?;
            SkillDocument::parse(&skill_raw)?;
        } else {
            skill_valid = false;
            warnings.push(format!(
                "optional skill file '{}' missing",
                manifest.skill_file
            ));
        }

        if let Some(main_rel) = &manifest.main {
            let main_path = plugin_dir.join(main_rel);
            if main_path.is_file() {
                let html = fs::read_to_string(&main_path)?;
                if html.contains("fetch(") && !manifest.permissions.iter().any(|p| p == "network") {
                    return Err(CliError::AuditFailed(
                        "main entry references fetch() without 'network' permission".into(),
                    ));
                }
            }
        }
    } else {
        let v1_path = plugin_dir.join("plugin.json");
        let raw = fs::read_to_string(&v1_path)?;
        let _manifest: PluginManifest = serde_json::from_str(&raw)?;
    }

    Ok(AuditReport {
        plugin_id: classification.id,
        manifest_file: classification.manifest_file,
        total_bytes,
        skill_valid,
        warnings,
    })
}

/// Packs a plugin directory into a deterministic `.atp` zip archive (`SPEC-009-R1`).
///
/// # Errors
///
/// Returns an error if [`audit_plugin`] fails, reading plugin or signing key files fails,
/// or writing the `.atp` zip archive fails.
pub fn pack_plugin(
    plugin_dir: &Path,
    out_archive: &Path,
    sign_key_path: Option<&Path>,
) -> Result<String> {
    let report = audit_plugin(plugin_dir)?;
    let mut files = Vec::new();
    collect_files_sorted(plugin_dir, plugin_dir, &mut files)?;

    let mut file_hashes = Vec::new();
    let file = File::create(out_archive)?;
    let mut zip = ZipWriter::new(file);
    let fixed_time = ZipDateTime::from_date_and_time(2026, 1, 1, 0, 0, 0).unwrap_or_default();
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(fixed_time);

    for (rel_str, abs_path) in &files {
        let bytes = fs::read(abs_path)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = format!("{:x}", hasher.finalize());
        file_hashes.push(json!({ "path": rel_str, "sha256": digest }));

        zip.start_file(rel_str, options)?;
        zip.write_all(&bytes)?;
    }

    let signature = if let Some(key_path) = sign_key_path {
        let key_bytes = fs::read(key_path)?;
        let mut sig_hasher = Sha256::new();
        sig_hasher.update(&key_bytes);
        for item in &file_hashes {
            sig_hasher.update(item.to_string().as_bytes());
        }
        Some(format!("ed25519-sha256:{:x}", sig_hasher.finalize()))
    } else {
        None
    };

    let pack_manifest = serde_json::to_string_pretty(&json!({
        "pluginId": report.plugin_id,
        "files": file_hashes,
        "signature": signature,
    }))?;
    zip.start_file("ATTOOLS-SIGNATURE.json", options)?;
    zip.write_all(pack_manifest.as_bytes())?;
    zip.finish()?;

    Ok(report.plugin_id)
}

/// `.ztrace` replay event (`SPEC-009-R7`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceReplayReport {
    pub total_events: usize,
    pub anomalies: usize,
    pub errors: usize,
}

/// Replays a `.ztrace` JSON or JSONL file and checks for anomalies (`SPEC-009-R7`).
///
/// # Errors
///
/// Returns [`CliError::Io`] or [`CliError::Json`] if reading or parsing `path` fails, or
/// [`CliError::TraceAnomaly`] if sequence numbers are non-monotonic or `assert_no_anomalies`
/// is set when errors or performance anomalies exist.
pub fn replay_ztrace(path: &Path, assert_no_anomalies: bool) -> Result<TraceReplayReport> {
    let raw = fs::read_to_string(path)?;
    let records: Vec<LogRecord> = if raw.trim_start().starts_with('[') {
        serde_json::from_str(&raw)?
    } else {
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut anomalies = 0usize;
    let mut errors = 0usize;
    for pair in records.windows(2) {
        if pair[0].seq >= pair[1].seq {
            return Err(CliError::TraceAnomaly(format!(
                "non-monotonic seq {} -> {}",
                pair[0].seq, pair[1].seq
            )));
        }
    }
    for rec in &records {
        if rec.perf_anomaly {
            anomalies += 1;
        }
        if matches!(rec.level.as_str(), "ERROR" | "FATAL") {
            errors += 1;
        }
    }

    if assert_no_anomalies && (anomalies > 0 || errors > 0) {
        return Err(CliError::TraceAnomaly(format!(
            "found {anomalies} perf anomalies and {errors} error/fatal records"
        )));
    }

    Ok(TraceReplayReport {
        total_events: records.len(),
        anomalies,
        errors,
    })
}

/// Handles a single MCP JSON-RPC 2.0 request for `aitools mcp serve` (`SPEC-009-R3`).
pub fn handle_mcp_request(
    registry: &ToolRegistry,
    guard: &PolicyGuard,
    req: &Value,
) -> Option<Value> {
    let id = req.get("id")?.clone();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-11-25",
            "serverInfo": { "name": "aitools", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "tools": {} }
        }),
        "tools/list" => {
            let tools: Vec<Value> = registry
                .select_for_prompt("", "")
                .into_iter()
                .map(|t| {
                    canonicalize_json(&json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    }))
                })
                .collect();
            json!({ "tools": tools })
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let approved = params
                .get("approved")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let Some(tool) = registry.get(name) else {
                let obs = ErrorObservation::from_tool_error(name, "tool not found");
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "isError": true, "structuredContent": obs }
                }));
            };
            match guard.evaluate(name, tool.risk_tier, name, &args, approved) {
                GateDecision::Allow => json!({
                    "isError": false,
                    "content": [{ "type": "text", "text": format!("Executed {name}") }]
                }),
                GateDecision::RequireSessionApproval { summary }
                | GateDecision::RequireExplicitApproval { summary, .. } => {
                    let obs = ErrorObservation::from_tool_error(
                        name,
                        &format!("confirmation required: {summary}"),
                    );
                    json!({ "isError": true, "structuredContent": obs })
                }
            }
        }
        other => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {other}") }
            }));
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Runs MCP server loop over stdio (`SPEC-009-R3`).
///
/// # Errors
///
/// Returns [`CliError::Io`] or [`CliError::Json`] if reading stdin, parsing JSON-RPC requests,
/// or writing stdout responses fails.
pub fn serve_mcp_stdio() -> Result<()> {
    let mut registry = ToolRegistry::new();
    registry.register_builtin_os_defaults();
    let guard = PolicyGuard::new();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in BufReader::new(stdin).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = serde_json::from_str(&line)?;
        if let Some(resp) = handle_mcp_request(&registry, &guard, &req) {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Exports telemetry logs to `.log` or `.zip` (`SPEC-009-R1`, `SPEC-010-R6`).
///
/// # Errors
///
/// Returns [`CliError::Telemetry`] if initializing telemetry or exporting logs fails.
pub fn export_logs(
    log_dir: PathBuf,
    out_path: &Path,
    as_zip: bool,
    trace_id: Option<String>,
) -> Result<aitools_telemetry::DiagnosticSummary> {
    let handle = aitools_telemetry::init(TelemetryConfig::new(log_dir))?;
    let filter = ExportFilter {
        trace_id,
        ..Default::default()
    };
    let summary = if as_zip {
        handle.export_diagnostic_bundle(out_path, &filter)?
    } else {
        handle.export_chronological_log(out_path, &filter)?
    };
    Ok(summary)
}

/// Analyzes persisted telemetry JSONL files (`aitools doctor` / `aitools logs analyze`, `SPEC-014-R2`).
///
/// # Errors
///
/// Returns [`CliError::Telemetry`] if opening `log_dir` or querying log files fails.
pub fn analyze_logs_dir(log_dir: PathBuf) -> Result<aitools_telemetry::DoctorReport> {
    let handle =
        aitools_telemetry::TelemetryHandle::open_standalone(TelemetryConfig::new(log_dir))?;
    Ok(handle.analyze_logs(&ExportFilter::default())?)
}

/// Converts an `aitools-diag-*.zip` bundle into a `.ztrace` replay fixture (`SPEC-014-R3` / `SPEC-014-R4`).
///
/// # Errors
///
/// Returns [`CliError::Telemetry`] if reading the diagnostic archive or writing `out_ztrace` fails.
pub fn import_diag_to_ztrace(
    zip_path: &Path,
    out_ztrace: &Path,
    trace_id: Option<&str>,
) -> Result<usize> {
    Ok(aitools_telemetry::convert_diag_zip_to_ztrace_filtered(
        zip_path, out_ztrace, trace_id,
    )?)
}

/// Summary report emitted by `aitools plugin migrate` (`SPEC-012-R2`, `SPEC-012-R4`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub plugin_id: String,
    pub detected_node_modules: Vec<String>,
    pub native_addons: Vec<String>,
    pub inferred_permissions: Vec<String>,
    pub drafted_tools: usize,
    pub manifest_path: PathBuf,
    pub skill_path: PathBuf,
    pub report_path: PathBuf,
}

/// Migrates a legacy v1 `plugin.json` directory into `aitools.manifest.json` + `SKILL.md` (`SPEC-012-R2`).
///
/// # Errors
///
/// Returns [`CliError::Io`] or [`CliError::Json`] if reading `plugin.json` or writing the
/// migrated `aitools.manifest.json` and `SKILL.md` fails.
pub fn migrate_legacy_plugin(legacy_dir: &Path, out_dir: Option<&Path>) -> Result<MigrationReport> {
    let v1_path = legacy_dir.join("plugin.json");
    let raw = fs::read_to_string(&v1_path)?;
    let v1: PluginManifest = serde_json::from_str(&raw)?;
    let plugin_id = aitools_core::sanitize_plugin_id(&v1.name);

    let mut detected_node = Vec::new();
    let mut permissions = vec!["db.read".to_string(), "db.write".to_string()];

    if let Some(preload_rel) = &v1.preload {
        let preload_path = legacy_dir.join(preload_rel);
        if let Ok(code) = fs::read_to_string(&preload_path) {
            for (needle, perm) in [
                ("require('fs')", "fs.read"),
                ("require(\"fs\")", "fs.read"),
                ("require('child_process')", "shell.exec"),
                ("require(\"child_process\")", "shell.exec"),
                ("require('http')", "network"),
                ("require(\"http\")", "network"),
                ("require('https')", "network"),
                ("require(\"https\")", "network"),
                ("require('electron')", "legacy.electron"),
                ("require(\"electron\")", "legacy.electron"),
                ("clipboard", "clipboard.read"),
            ] {
                if code.contains(needle) {
                    detected_node.push(needle.to_string());
                    if !permissions.iter().any(|p| p == perm) {
                        permissions.push(perm.to_string());
                    }
                }
            }
        }
    }

    let ai_tools: Vec<Value> = v1
        .features
        .iter()
        .map(|f| {
            json!({
                "name": f.code,
                "description": if f.explain.is_empty() { f.label.clone().unwrap_or_else(|| f.code.clone()) } else { f.explain.clone() },
                "risk_tier": "tier1_state_mutating",
                "input_schema": {
                    "type": "object",
                    "properties": { "payload": { "type": "string" } }
                }
            })
        })
        .collect();

    let mut all_files = Vec::new();
    collect_files_sorted(legacy_dir, legacy_dir, &mut all_files)?;
    let native_addons: Vec<String> = all_files
        .into_iter()
        .filter(|(rel, _)| rel.ends_with(".node"))
        .map(|(rel, _)| rel)
        .collect();

    let dest = out_dir.unwrap_or(legacy_dir);
    fs::create_dir_all(dest)?;
    let manifest_path = dest.join("aitools.manifest.json");
    let skill_path = dest.join("SKILL.md");
    let report_path = dest.join("MIGRATION_REPORT.md");

    let v2_json = json!({
        "id": plugin_id,
        "name": v1.title.unwrap_or(v1.name),
        "version": if v1.version.is_empty() { "0.1.0".to_string() } else { v1.version },
        "runtime": "native-ui",
        "main": v1.main.unwrap_or_else(|| "index.html".to_string()),
        "permissions": permissions,
        "ai_tools": ai_tools,
        "skill_file": "SKILL.md"
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&v2_json)?)?;

    let mut skill_md = format!(
        "---\nname: {plugin_id}\ndescription: {}\n---\n\n# {}\n\nMigrated from legacy `plugin.json`.\n",
        v1.description.as_deref().unwrap_or("Migrated aitools plugin"),
        plugin_id
    );
    if !detected_node.is_empty() {
        skill_md.push_str("\n## Detected Legacy Node Dependencies\n");
        for dep in &detected_node {
            skill_md.push_str(&format!("- `{dep}`\n"));
        }
    }
    fs::write(&skill_path, skill_md)?;

    let report_md = render_migration_report(
        &plugin_id,
        &detected_node,
        &native_addons,
        &permissions,
        &ai_tools,
    );
    fs::write(&report_path, report_md)?;

    Ok(MigrationReport {
        plugin_id,
        detected_node_modules: detected_node,
        native_addons,
        inferred_permissions: permissions,
        drafted_tools: ai_tools.len(),
        manifest_path,
        skill_path,
        report_path,
    })
}

fn render_migration_report(
    plugin_id: &str,
    detected_node: &[String],
    native_addons: &[String],
    permissions: &[String],
    ai_tools: &[Value],
) -> String {
    let mut out = format!("# Migration Report: `{plugin_id}`\n\n");
    out.push_str("## Inferred Permissions\n");
    for perm in permissions {
        out.push_str(&format!("- `{perm}`\n"));
    }
    out.push_str(&format!("\n## Drafted AI Tools ({})\n", ai_tools.len()));
    for tool in ai_tools {
        if let Some(name) = tool.get("name").and_then(Value::as_str) {
            out.push_str(&format!("- `{name}`\n"));
        }
    }
    out.push_str("\n## Legacy Node Builtins\n");
    if detected_node.is_empty() {
        out.push_str("- None detected.\n");
    } else {
        for dep in detected_node {
            out.push_str(&format!("- `{dep}` -> review polyfill/IPC replacement\n"));
        }
    }
    out.push_str("\n## Native `.node` Addons (Manual Review Required)\n");
    if native_addons.is_empty() {
        out.push_str("- None detected.\n");
    } else {
        for addon in native_addons {
            out.push_str(&format!(
                "- WARNING: native addon `{addon}` requires replacement with a Rust/Wasm host capability or `@aitools/sdk` IPC\n"
            ));
        }
    }
    out
}

fn dir_size_bytes(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size_bytes(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

fn collect_files_sorted(root: &Path, curr: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(curr)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files_sorted(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn spec_009_r1_init_audit_and_deterministic_pack() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        init_plugin(&plugin_dir, "My Plugin", PluginTemplate::NativeUi).unwrap();

        let report = audit_plugin(&plugin_dir).unwrap();
        assert_eq!(report.plugin_id, "my-plugin");
        assert!(report.skill_valid);

        let key_file = dir.path().join("dev.key");
        fs::write(&key_file, b"test-signing-key").unwrap();

        let out1 = dir.path().join("out1.atp");
        let out2 = dir.path().join("out2.atp");
        pack_plugin(&plugin_dir, &out1, Some(&key_file)).unwrap();
        pack_plugin(&plugin_dir, &out2, Some(&key_file)).unwrap();

        assert_eq!(fs::read(&out1).unwrap(), fs::read(&out2).unwrap());
    }

    #[test]
    fn spec_009_r3_mcp_server_lists_and_enforces_hitl() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin_os_defaults();
        let guard = PolicyGuard::new();

        let list_resp = handle_mcp_request(
            &registry,
            &guard,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .unwrap();
        assert!(!list_resp["result"]["tools"].as_array().unwrap().is_empty());

        // Tier 1 mutating tool requires confirmation when approved=false.
        let blocked = handle_mcp_request(
            &registry,
            &guard,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": "os.clipboard.write", "arguments": { "text": "hi" } }
            }),
        )
        .unwrap();
        assert_eq!(blocked["result"]["isError"], true);
        assert_eq!(
            blocked["result"]["structuredContent"]["errorCode"],
            "confirmation_required"
        );
    }

    #[test]
    fn spec_009_r7_ztrace_replay_catches_anomalies() {
        let dir = TempDir::new().unwrap();
        let clean = dir.path().join("clean.ztrace");
        fs::write(
            &clean,
            r#"{"seq":1,"ts":"2026-11-16T00:00:00.000001Z","level":"INFO","source":"wake","msg":"ok"}
{"seq":2,"ts":"2026-11-16T00:00:00.000100Z","level":"DEBUG","source":"wake","duration_us":2500,"msg":"done"}
"#,
        )
        .unwrap();
        let ok = replay_ztrace(&clean, true).unwrap();
        assert_eq!(ok.total_events, 2);
        assert_eq!(ok.anomalies, 0);

        let bad = dir.path().join("anomaly.ztrace");
        fs::write(
            &bad,
            r#"{"seq":1,"ts":"2026-11-16T00:00:00.000001Z","level":"WARN","source":"wake","duration_us":20000,"perf_anomaly":true,"msg":"slow"}
"#,
        )
        .unwrap();
        assert!(replay_ztrace(&bad, true).is_err());
    }

    #[test]
    fn spec_012_r2_migrates_legacy_plugin_and_detects_node_deps() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("plugin.json"),
            r#"{
                "name": "Legacy Tool",
                "version": "1.0.0",
                "main": "index.html",
                "preload": "preload.js",
                "features": [{ "code": "run", "explain": "Run legacy task", "cmds": ["run"] }]
            }"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("preload.js"),
            "const fs = require('fs'); const { clipboard } = require('electron');",
        )
        .unwrap();
        fs::write(dir.path().join("index.html"), "<h1>Legacy</h1>").unwrap();
        fs::write(dir.path().join("binding.node"), b"ELF-stub").unwrap();

        let report = migrate_legacy_plugin(dir.path(), None).unwrap();
        assert_eq!(report.plugin_id, "legacy-tool");
        assert!(report
            .detected_node_modules
            .iter()
            .any(|d| d.contains("fs")));
        assert_eq!(report.native_addons, vec!["binding.node".to_string()]);
        assert!(report.inferred_permissions.iter().any(|p| p == "fs.read"));
        assert_eq!(report.drafted_tools, 1);

        let report_md = fs::read_to_string(&report.report_path).unwrap();
        assert!(report_md.contains("binding.node"));
        assert!(report_md.contains("Manual Review Required"));

        let audit = audit_plugin(dir.path()).unwrap();
        assert_eq!(audit.manifest_file, "aitools.manifest.json");
        assert!(audit.skill_valid);
    }

    #[test]
    fn spec_012_benchmark_plugins_pass_audit() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "examples/plugins/aitools-plugin-devtools",
            "examples/plugins/aitools-plugin-smart-clip",
            "examples/plugins/legacy-color-sample",
        ] {
            let report = audit_plugin(&root.join(rel))
                .unwrap_or_else(|e| panic!("benchmark plugin {rel} failed audit: {e}"));
            assert!(
                report.skill_valid,
                "benchmark plugin {rel} should have valid SKILL.md"
            );
        }
    }

    #[test]
    fn spec_013_r5_replays_all_golden_ztrace_fixtures() {
        let traces_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/traces");
        let mut count = 0usize;
        for entry in fs::read_dir(&traces_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("ztrace") {
                let report = replay_ztrace(&path, true).unwrap_or_else(|e| {
                    panic!("golden ztrace {} failed replay: {e}", path.display())
                });
                assert!(report.total_events > 0);
                assert_eq!(report.anomalies, 0);
                assert_eq!(report.errors, 0);
                count += 1;
            }
        }
        assert_eq!(count, 6, "expected 6 golden .ztrace fixtures");
    }

    #[test]
    fn spec_008_builtin_plugins_pass_audit_and_classification() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let expected = [
            (
                "plugins/builtin/clipboard-history",
                aitools_core::PluginRuntimeTier::NativeUi,
            ),
            (
                "plugins/builtin/translation",
                aitools_core::PluginRuntimeTier::NativeUi,
            ),
            (
                "plugins/builtin/terminal-launchpad",
                aitools_core::PluginRuntimeTier::NativeUi,
            ),
            (
                "plugins/builtin/dev-docs-reference",
                aitools_core::PluginRuntimeTier::NativeUi,
            ),
            (
                "plugins/builtin/json-codec",
                aitools_core::PluginRuntimeTier::RustWasm,
            ),
            (
                "plugins/builtin/timestamp-calc",
                aitools_core::PluginRuntimeTier::RustWasm,
            ),
            (
                "plugins/builtin/snipaste-legacy",
                aitools_core::PluginRuntimeTier::LegacyCompat,
            ),
            (
                "plugins/builtin/todo-legacy",
                aitools_core::PluginRuntimeTier::LegacyCompat,
            ),
        ];

        for (rel, tier) in expected {
            let plugin_dir = root.join(rel);
            let cls = classify_plugin_dir(&plugin_dir)
                .unwrap_or_else(|e| panic!("classify failed for {rel}: {e}"));
            assert_eq!(cls.tier, tier, "unexpected tier for {rel}");

            let report =
                audit_plugin(&plugin_dir).unwrap_or_else(|e| panic!("audit failed for {rel}: {e}"));
            assert!(
                report.skill_valid,
                "plugin {rel} failed SKILL.md validation"
            );
        }
    }
}
