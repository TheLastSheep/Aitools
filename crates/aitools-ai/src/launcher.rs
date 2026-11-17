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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LauncherHit {
    pub feature_code: String,
    pub plugin_id: String,
    pub score: i32,
    pub match_type: String,
    pub confidence: f64,
}

impl From<MatchResult> for LauncherHit {
    fn from(m: MatchResult) -> Self {
        let confidence = (f64::from(m.score) / 100.0).clamp(0.0, 1.0);
        Self {
            feature_code: m.feature_code,
            plugin_id: m.plugin_id,
            score: m.score,
            match_type: m.match_type.to_string(),
            confidence,
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
    let hits: Vec<LauncherHit> = raw_matches.into_iter().map(LauncherHit::from).collect();
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

        // Keystroke renders deterministic fast path immediately.
        let fast = evaluate_launcher_input(&index, "calc", false, false).unwrap();
        assert!(matches!(
            fast,
            LauncherDecision::DeterministicResults { top_confidence, .. } if top_confidence >= 0.9
        ));

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
