//! Dual-mode launcher decision engine (`SPEC-007-R1`, `SPEC-007-R2`).

use aitools_core::{FeatureEntry, FeatureSearchIndex, MatchResult};
use serde::{Deserialize, Serialize};

pub const DETERMINISTIC_CONFIDENCE_THRESHOLD: f64 = 0.45;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LauncherDecision {
    DeterministicResults {
        matches: Vec<LauncherHit>,
        top_confidence: f64,
    },
    PromoteToAgent {
        reason: PromotionReason,
        query: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionReason {
    TabPressed,
    AtMentionPrefix,
    LowDeterministicConfidence,
}

/// Enriched launcher search match returned to the Svelte shell (`SPEC-007-R1`, `SPEC-017-R3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LauncherHit {
    /// Target feature identifier within the plugin or app catalog.
    pub feature_code: String,
    /// Owning plugin or catalog identifier.
    pub plugin_id: String,
    /// Primary human-readable label for display.
    pub label: String,
    /// Owning plugin or application display name.
    pub plugin_name: String,
    /// Concise feature description for secondary row text.
    pub description: String,
    /// Optional resolved icon (`data:` URI or URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Dispatch kind hint (`"app"`, `"plugin"`, `"list"`, `"wasm"`, `"macro"`, `"feature"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Deterministic/fuzzy relevance score.
    pub score: i32,
    /// Match category (`"exact"`, `"prefix"`, `"pinyin"`, `"regex"`, `"over"`, `"contains"`, `"fuzzy"`, `"files"`, `"img"`).
    pub match_type: String,
    /// Normalized confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Human-readable explanation / subtitle matching `FeatureEntry::explain`.
    pub explain: String,
}

impl LauncherHit {
    /// Constructs an enriched [`LauncherHit`] using metadata retained in `index`.
    #[must_use]
    pub fn from_match_with_index(m: MatchResult, index: &FeatureSearchIndex) -> Self {
        let confidence = (f64::from(m.score) / 100.0).clamp(0.0, 1.0);
        let meta = index.feature_meta(&m.plugin_id, &m.feature_code);
        let kind = infer_hit_kind(&m.plugin_id, &m.feature_code);
        let (label, plugin_name, icon, explain) = match meta {
            Some(meta) => (meta.label, meta.plugin_name, meta.icon, meta.explain),
            None => (
                m.feature_code.clone(),
                m.plugin_id.clone(),
                None,
                String::new(),
            ),
        };
        Self {
            feature_code: m.feature_code,
            plugin_id: m.plugin_id,
            label,
            plugin_name,
            description: explain.clone(),
            icon,
            kind,
            score: m.score,
            match_type: m.match_type.to_string(),
            confidence,
            explain,
        }
    }
}

fn infer_hit_kind(plugin_id: &str, feature_code: &str) -> Option<String> {
    if plugin_id == "system-apps" || feature_code.starts_with("app:") {
        Some("app".to_string())
    } else if plugin_id == "aitools-macros" || feature_code.starts_with("macro:") {
        Some("macro".to_string())
    } else {
        None
    }
}

impl From<MatchResult> for LauncherHit {
    fn from(m: MatchResult) -> Self {
        let confidence = (f64::from(m.score) / 100.0).clamp(0.0, 1.0);
        let kind = infer_hit_kind(&m.plugin_id, &m.feature_code);
        Self {
            label: m.feature_code.clone(),
            plugin_name: m.plugin_id.clone(),
            description: String::new(),
            icon: None,
            kind,
            feature_code: m.feature_code,
            plugin_id: m.plugin_id,
            score: m.score,
            match_type: m.match_type.to_string(),
            confidence,
            explain: String::new(),
        }
    }
}

/// Evaluates user keystrokes synchronously first (`SPEC-007-R1`), and promotes to
/// an Agent session when `tab_pressed`, `@` prefix, or confidence < `0.45` (`SPEC-007-R2`).
///
/// # Errors
///
/// Returns [`aitools_core::AitoolsError::BudgetExceeded`] if searching `index` exceeds
/// `SPEC-005` query or regex budgets.
pub fn evaluate_launcher_input(
    index: &FeatureSearchIndex,
    input: &str,
    tab_pressed: bool,
    submitted: bool,
) -> aitools_core::Result<LauncherDecision> {
    let trimmed = input.trim();
    if tab_pressed {
        return Ok(LauncherDecision::PromoteToAgent {
            reason: PromotionReason::TabPressed,
            query: trimmed.to_string(),
        });
    }

    let raw_matches = index.search_checked(trimmed)?;
    let hits: Vec<LauncherHit> = raw_matches
        .into_iter()
        .map(|m| LauncherHit::from_match_with_index(m, index))
        .collect();
    let top_confidence = hits.first().map_or(0.0, |h| h.confidence);

    if submitted {
        if trimmed.starts_with('@') {
            return Ok(LauncherDecision::PromoteToAgent {
                reason: PromotionReason::AtMentionPrefix,
                query: trimmed.to_string(),
            });
        }
        if top_confidence < DETERMINISTIC_CONFIDENCE_THRESHOLD {
            return Ok(LauncherDecision::PromoteToAgent {
                reason: PromotionReason::LowDeterministicConfidence,
                query: trimmed.to_string(),
            });
        }
    }

    Ok(LauncherDecision::DeterministicResults {
        matches: hits,
        top_confidence,
    })
}

/// Builds a [`FeatureSearchIndex`] for the launcher decision engine.
///
/// # Errors
///
/// Returns [`aitools_core::AitoolsError::BudgetExceeded`] if indexing `features` exceeds
/// `SPEC-005` regex or memory budgets.
pub fn build_index(features: &[FeatureEntry]) -> aitools_core::Result<FeatureSearchIndex> {
    FeatureSearchIndex::try_build(features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aitools_core::Cmd;

    fn sample_features() -> Vec<FeatureEntry> {
        vec![FeatureEntry {
            code: "calc".into(),
            plugin_id: "builtin-calc".into(),
            plugin_name: "Calculator".into(),
            label: "Calculator".into(),
            icon: None,
            explain: "Evaluate math expressions".into(),
            cmds: vec![Cmd::Text("calculator".into())],
            main_push: false,
            priority: 10,
        }]
    }

    #[test]
    fn spec_007_r1_and_r2_fast_path_and_agent_promotion() {
        let index = build_index(&sample_features()).unwrap();

        // Keystroke renders deterministic fast path immediately with enriched metadata.
        let fast = evaluate_launcher_input(&index, "calc", false, false).unwrap();
        match &fast {
            LauncherDecision::DeterministicResults {
                matches,
                top_confidence,
            } => {
                assert!(*top_confidence >= 0.9);
                assert_eq!(matches[0].label, "Calculator");
                assert_eq!(matches[0].plugin_name, "Calculator");
                assert_eq!(matches[0].explain, "Evaluate math expressions");
                assert_eq!(matches[0].description, "Evaluate math expressions");
            }
            LauncherDecision::PromoteToAgent { .. } => panic!("expected deterministic results"),
        }

        // Tab promotes immediately.
        let tab = evaluate_launcher_input(&index, "calc", true, false).unwrap();
        assert!(matches!(
            tab,
            LauncherDecision::PromoteToAgent {
                reason: PromotionReason::TabPressed,
                ..
            }
        ));

        // Submitting @mention or low-confidence query promotes to Agent.
        let at = evaluate_launcher_input(&index, "@files find pdfs", false, true).unwrap();
        assert!(matches!(
            at,
            LauncherDecision::PromoteToAgent {
                reason: PromotionReason::AtMentionPrefix,
                ..
            }
        ));

        let low =
            evaluate_launcher_input(&index, "summarize my desktop files", false, true).unwrap();
        assert!(matches!(
            low,
            LauncherDecision::PromoteToAgent {
                reason: PromotionReason::LowDeterministicConfidence,
                ..
            }
        ));
    }
}
