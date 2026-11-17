//! Salvaged AI-Native Harness primitives (`ErrorObservation`, `DangerousCommandGuard`,
//! `PromptSanitizer`, `prune_tool_output`, `canonicalize_json`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Structured self-healing error observation returned to an Agent when a tool call fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorObservation {
    pub error_code: String,
    pub message: String,
    pub recoverable: bool,
    pub why_failed: String,
    pub suggested_fixes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_capability_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example_arguments: Option<Value>,
}

impl ErrorObservation {
    pub fn from_tool_error(tool_name: &str, message: &str) -> Self {
        let lower = message.to_ascii_lowercase();

        if lower.contains("confirmation required") || lower.contains("confirm") {
            return Self {
                error_code: "confirmation_required".to_string(),
                message: message.to_string(),
                recoverable: true,
                why_failed: format!(
                    "Tool '{tool_name}' mutates local state and requires explicit HITL approval or dry_run preview first."
                ),
                suggested_fixes: vec![
                    "Call the tool first with `{\"dry_run\": true}` if supported.".to_string(),
                    "Request user confirmation via PolicyGuard before re-invoking.".to_string(),
                ],
                recovery_capability_id: Some(tool_name.to_string()),
                example_arguments: Some(json!({ "dry_run": true })),
            };
        }

        if lower.contains("permission") || lower.contains("denied") || lower.contains("forbidden") {
            return Self {
                error_code: "permission_denied".to_string(),
                message: message.to_string(),
                recoverable: true,
                why_failed: format!("Tool '{tool_name}' lacks a capability grant."),
                suggested_fixes: vec![
                    "Inspect enabled capabilities before retrying.".to_string(),
                    "Ask the user to grant the required permission.".to_string(),
                ],
                recovery_capability_id: None,
                example_arguments: None,
            };
        }

        if lower.contains("not found") || lower.contains("no such file") {
            return Self {
                error_code: "resource_not_found".to_string(),
                message: message.to_string(),
                recoverable: true,
                why_failed: "Target path, document, or parameter does not exist.".to_string(),
                suggested_fixes: vec![
                    "Locate the exact path using a read-only search tool first.".to_string()
                ],
                recovery_capability_id: None,
                example_arguments: None,
            };
        }

        Self {
            error_code: "tool_execution_error".to_string(),
            message: message.to_string(),
            recoverable: true,
            why_failed: format!("Tool '{tool_name}' failed while processing arguments."),
            suggested_fixes: vec![
                "Validate argument types against the tool's input schema.".to_string()
            ],
            recovery_capability_id: Some(tool_name.to_string()),
            example_arguments: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyVerdict {
    pub allowed: bool,
    pub risk_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl SafetyVerdict {
    pub fn allow() -> Self {
        Self {
            allowed: true,
            risk_level: "low".to_string(),
            reason: None,
            remediation: None,
        }
    }

    pub fn block(
        risk_level: &str,
        reason: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            allowed: false,
            risk_level: risk_level.to_string(),
            reason: Some(reason.into()),
            remediation: Some(remediation.into()),
        }
    }
}

/// Defense-in-depth guardrail for sensitive host paths.
pub struct DangerousCommandGuard;

impl DangerousCommandGuard {
    const PROTECTED_PATH_SEGMENTS: &'static [&'static str] = &[
        "/.ssh",
        "/.gnupg",
        "/.aws",
        "/.kube",
        "/etc/sudoers",
        "/etc/passwd",
        "/etc/shadow",
        "/System/",
        "/Library/Keychains",
    ];

    pub fn check_path(path: &str, is_write: bool) -> SafetyVerdict {
        let normalized = path.trim().replace('\\', "/");
        if normalized.is_empty() {
            return SafetyVerdict::block(
                "medium",
                "Path must not be empty",
                "Provide a valid path within the workspace or home directory.",
            );
        }
        if normalized == "/" || normalized == "~" {
            return SafetyVerdict::block(
                "critical",
                "Root or home root operation is blocked",
                "Target a specific subdirectory or file path.",
            );
        }
        for segment in Self::PROTECTED_PATH_SEGMENTS {
            if normalized.contains(segment) {
                return SafetyVerdict::block(
                    "critical",
                    format!("Path '{path}' touches protected directory '{segment}'"),
                    "Do not read or modify credential or OS system directories.",
                );
            }
        }
        if is_write
            && (normalized.starts_with("/usr/")
                || normalized.starts_with("/bin/")
                || normalized.starts_with("/sbin/"))
        {
            return SafetyVerdict::block(
                "critical",
                format!("Write access to system directory '{path}' is forbidden"),
                "Write outputs only inside user-owned directories.",
            );
        }
        SafetyVerdict::allow()
    }
}

/// Sanitizes untrusted observations (clipboard, OCR) against indirect prompt injections.
pub struct PromptSanitizer;

impl PromptSanitizer {
    const INJECTION_PATTERNS: &'static [&'static str] = &[
        "ignore previous instructions",
        "ignore all previous instructions",
        "disregard previous instructions",
        "system prompt:",
        "<|im_start|>system",
        "<|system|>",
        "you are now in developer mode",
    ];

    pub fn sanitize_observation(input: &str) -> (String, bool) {
        let lower = input.to_ascii_lowercase();
        let detected = Self::INJECTION_PATTERNS.iter().any(|p| lower.contains(p));
        if !detected {
            return (input.to_string(), false);
        }
        (
            format!("<untrusted_data>\n{input}\n</untrusted_data>"),
            true,
        )
    }
}

/// Head (60%) + Tail (40%) context pruning for oversized tool outputs.
pub fn prune_tool_output(output: &str, max_chars: usize) -> String {
    let total_chars = output.chars().count();
    if total_chars <= max_chars || max_chars < 64 {
        return output.to_string();
    }
    let head_budget = (max_chars * 6) / 10;
    let tail_budget = max_chars.saturating_sub(head_budget);
    let head: String = output.chars().take(head_budget).collect();
    let tail: String = output
        .chars()
        .skip(total_chars.saturating_sub(tail_budget))
        .collect();
    let omitted = total_chars.saturating_sub(head_budget + tail_budget);
    format!("{head}\n... [pruned {omitted} chars] ...\n{tail}")
}

/// Recursively sorts JSON object keys for deterministic KV-cache prefix stability.
pub fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize_json(v));
            }
            let mut out = Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}
