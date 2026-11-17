//! Ambient context fusion and zero-query intent anchoring (`SPEC-011-R1`).

use serde::{Deserialize, Serialize};

/// Classified payload signature inferred from ambient desktop context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSignature {
    Json,
    StackTrace,
    Url,
    FilePaths,
    Image,
    PlainText,
    Empty,
}

/// Unified ambient context captured prior to launcher focus (`SPEC-011-R1`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEnvelope {
    pub trace_id: String,
    pub selection: Option<String>,
    pub clipboard: Option<String>,
    pub active_app: Option<String>,
    pub active_path: Option<String>,
    pub image_handle: Option<String>,
}

impl ContextEnvelope {
    /// Returns the highest-priority non-empty textual payload (`selection` before `clipboard`).
    #[must_use]
    pub fn primary_text(&self) -> Option<&str> {
        self.selection
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.clipboard.as_deref().filter(|s| !s.trim().is_empty()))
    }

    /// Classifies the ambient context without blocking on network or LLM inference.
    #[must_use]
    pub fn classify(&self) -> ContentSignature {
        if self.image_handle.is_some() {
            return ContentSignature::Image;
        }
        let Some(text) = self.primary_text().map(str::trim) else {
            return if self.active_path.is_some() {
                ContentSignature::FilePaths
            } else {
                ContentSignature::Empty
            };
        };

        if ((text.starts_with('{') && text.ends_with('}'))
            || (text.starts_with('[') && text.ends_with(']')))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
        {
            return ContentSignature::Json;
        }
        if text.starts_with("http://") || text.starts_with("https://") {
            return ContentSignature::Url;
        }
        if looks_like_stack_trace(text) {
            return ContentSignature::StackTrace;
        }
        if text.starts_with('/') || text.contains(":\\") {
            return ContentSignature::FilePaths;
        }
        ContentSignature::PlainText
    }
}

/// Zero-query suggested AI action or tool surfaced before the user types (`SPEC-011-R1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickAgentAction {
    pub id: String,
    pub title: String,
    pub prompt_template: String,
    pub target_tool: Option<String>,
    pub signature: ContentSignature,
}

/// Computes up to 3 top-ranked Quick Agent Actions from `envelope` in < 1 ms (`SPEC-011-R1`).
#[must_use]
pub fn suggest_zero_query_actions(envelope: &ContextEnvelope) -> Vec<QuickAgentAction> {
    let sig = envelope.classify();
    let mut actions = match sig {
        ContentSignature::Json => vec![
            action(
                "json-format",
                "Pretty-print & validate JSON",
                "Format and validate the JSON in context",
                Some("legacy-json.format"),
                sig,
            ),
            action(
                "json-to-rust",
                "Convert JSON to Rust structs",
                "Generate idiomatic Rust serde structs from this JSON",
                None,
                sig,
            ),
            action(
                "json-explain",
                "Summarize JSON schema",
                "Summarize keys and anomalies in this JSON payload",
                None,
                sig,
            ),
        ],
        ContentSignature::StackTrace => vec![
            action(
                "trace-diagnose",
                "Diagnose stack trace root cause",
                "Analyze this error stack trace and identify the root cause and fix",
                None,
                sig,
            ),
            action(
                "trace-search-files",
                "Locate referenced source files",
                "Find local files mentioned in this stack trace",
                Some("os.files.find"),
                sig,
            ),
            action(
                "trace-copy-summary",
                "Distill concise bug summary",
                "Summarize this error in 3 bullet points and copy to clipboard",
                Some("os.clipboard.write"),
                sig,
            ),
        ],
        ContentSignature::Url => vec![
            action(
                "url-summarize",
                "Summarize & extract key links",
                "Inspect this URL and summarize key details",
                None,
                sig,
            ),
            action(
                "url-clean",
                "Strip tracking parameters",
                "Remove UTM/tracking query parameters from this URL and copy",
                Some("os.clipboard.write"),
                sig,
            ),
            action(
                "url-qr",
                "Generate QR code for URL",
                "Create a scannable QR code for this URL",
                None,
                sig,
            ),
        ],
        ContentSignature::FilePaths => vec![
            action(
                "files-inspect",
                "Inspect selected path",
                "Summarize contents or metadata of the active path",
                Some("os.files.find"),
                sig,
            ),
            action(
                "files-rename-plan",
                "Plan safe batch rename (dry-run)",
                "Preview a clean batch rename plan for these files",
                None,
                sig,
            ),
            action(
                "files-archive",
                "Create deterministic archive",
                "Pack selected files into a clean archive",
                None,
                sig,
            ),
        ],
        ContentSignature::Image => vec![
            action(
                "img-ocr",
                "Extract text via OCR",
                "Extract all visible text from the captured image",
                None,
                sig,
            ),
            action(
                "img-explain",
                "Explain screenshot or UI",
                "Describe errors or UI details visible in the image",
                None,
                sig,
            ),
            action(
                "img-compress",
                "Compress image for web",
                "Optimize image dimensions and byte size",
                None,
                sig,
            ),
        ],
        ContentSignature::PlainText => vec![
            action(
                "text-translate",
                "Translate text (EN <-> ZH)",
                "Translate the selected or copied text cleanly",
                None,
                sig,
            ),
            action(
                "text-polish",
                "Proofread & tighten prose",
                "Rewrite this text clearly and concisely",
                None,
                sig,
            ),
            action(
                "text-extract-tasks",
                "Extract actionable checklist",
                "Extract action items from this text",
                None,
                sig,
            ),
        ],
        ContentSignature::Empty => vec![
            action(
                "empty-clipboard",
                "Search recent clipboard",
                "Inspect recent clipboard entries",
                Some("os.clipboard.read"),
                sig,
            ),
            action(
                "empty-files",
                "Find recent workspace files",
                "Search local files in the active workspace",
                Some("os.files.find"),
                sig,
            ),
            action(
                "empty-ask",
                "Ask AI Agent",
                "Start an open-ended desktop agent session",
                None,
                sig,
            ),
        ],
    };
    actions.truncate(3);
    actions
}

fn action(
    id: &str,
    title: &str,
    prompt_template: &str,
    target_tool: Option<&str>,
    signature: ContentSignature,
) -> QuickAgentAction {
    QuickAgentAction {
        id: id.to_string(),
        title: title.to_string(),
        prompt_template: prompt_template.to_string(),
        target_tool: target_tool.map(str::to_string),
        signature,
    }
}

fn looks_like_stack_trace(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("panicked at")
        || lower.contains("traceback (most recent call last)")
        || lower.contains("error[e")
        || (lower.contains("at ") && lower.contains(".rs:"))
        || (lower.contains("exception") && lower.contains("at "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_011_r1_classifies_context_and_returns_top_3_suggestions() {
        let json_env = ContextEnvelope {
            trace_id: "wake-1".into(),
            clipboard: Some(r#"{"name":"aitools","v":2}"#.into()),
            ..Default::default()
        };
        assert_eq!(json_env.classify(), ContentSignature::Json);
        let actions = suggest_zero_query_actions(&json_env);
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].id, "json-format");

        let trace_env = ContextEnvelope {
            trace_id: "wake-2".into(),
            selection: Some("thread 'main' panicked at src/main.rs:12:5".into()),
            ..Default::default()
        };
        assert_eq!(trace_env.classify(), ContentSignature::StackTrace);
        assert_eq!(
            suggest_zero_query_actions(&trace_env)[0].id,
            "trace-diagnose"
        );
    }
}
