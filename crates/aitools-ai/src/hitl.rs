//! 3-tier Human-in-the-Loop (HITL) PolicyGuard (`SPEC-007-R7`, `INV-SEC-04`).

use std::collections::HashSet;
use std::sync::Mutex;

use aitools_core::RiskTier;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AiError, Result};

/// Outcome of evaluating a tool call through [`PolicyGuard`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    RequireSessionApproval { summary: String },
    RequireExplicitApproval { summary: String, arguments: Value },
}

/// Enforces `SPEC-007-R7` and `INV-SEC-04` before any AI agent tool executes.
#[derive(Debug, Default)]
pub struct PolicyGuard {
    session_approved_tier1: Mutex<HashSet<String>>,
}

impl PolicyGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates whether `tool_name` at `risk_tier` can execute immediately or needs HITL UI approval.
    ///
    /// # Panics
    ///
    /// Panics if the internal session approval mutex is poisoned.
    pub fn evaluate(
        &self,
        tool_name: &str,
        risk_tier: RiskTier,
        resource_summary: &str,
        arguments: &Value,
        explicitly_approved: bool,
    ) -> GateDecision {
        match risk_tier {
            RiskTier::Tier0ReadOnly => GateDecision::Allow,
            RiskTier::Tier1StateMutating => {
                if explicitly_approved
                    || self
                        .session_approved_tier1
                        .lock()
                        .expect("session approved tier1 mutex poisoned")
                        .contains(tool_name)
                {
                    GateDecision::Allow
                } else {
                    GateDecision::RequireSessionApproval {
                        summary: format!("{tool_name}: {resource_summary}"),
                    }
                }
            }
            RiskTier::Tier2DestructiveOrExternal => {
                if explicitly_approved {
                    GateDecision::Allow
                } else {
                    GateDecision::RequireExplicitApproval {
                        summary: format!("{tool_name}: {resource_summary}"),
                        arguments: arguments.clone(),
                    }
                }
            }
        }
    }

    /// Records a once-per-session approval for a `Tier1StateMutating` tool.
    ///
    /// # Panics
    ///
    /// Panics if the internal session approval mutex is poisoned.
    pub fn approve_for_session(&self, tool_name: &str) {
        self.session_approved_tier1
            .lock()
            .expect("session approved tier1 mutex poisoned")
            .insert(tool_name.to_string());
    }

    /// Executes `action` strictly after passing the HITL gate (`INV-SEC-04`).
    ///
    /// # Errors
    ///
    /// Returns [`AiError::HitlConfirmationRequired`] if the tool call requires session or explicit
    /// confirmation, or propagates any error returned by `action`.
    pub fn execute_guarded<T>(
        &self,
        tool_name: &str,
        risk_tier: RiskTier,
        resource_summary: &str,
        arguments: &Value,
        explicitly_approved: bool,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        match self.evaluate(
            tool_name,
            risk_tier,
            resource_summary,
            arguments,
            explicitly_approved,
        ) {
            GateDecision::Allow => action(),
            GateDecision::RequireSessionApproval { summary }
            | GateDecision::RequireExplicitApproval { summary, .. } => {
                Err(AiError::HitlConfirmationRequired {
                    tool: tool_name.to_string(),
                    tier: risk_tier,
                    reason: summary,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_007_r7_three_tier_hitl_policy_guard() {
        let guard = PolicyGuard::new();

        // Tier 0 auto-approves without UI prompt.
        assert_eq!(
            guard.evaluate(
                "read_note",
                RiskTier::Tier0ReadOnly,
                "note-1",
                &json!({}),
                false
            ),
            GateDecision::Allow
        );

        // Tier 1 prompts once per session, then auto-allows after session approval.
        assert!(matches!(
            guard.evaluate(
                "write_note",
                RiskTier::Tier1StateMutating,
                "note-1",
                &json!({}),
                false
            ),
            GateDecision::RequireSessionApproval { .. }
        ));
        guard.approve_for_session("write_note");
        assert_eq!(
            guard.evaluate(
                "write_note",
                RiskTier::Tier1StateMutating,
                "note-1",
                &json!({}),
                false
            ),
            GateDecision::Allow
        );

        // Tier 2 ALWAYS requires explicit per-invocation approval even if Tier 1 was approved.
        assert!(matches!(
            guard.evaluate(
                "delete_file",
                RiskTier::Tier2DestructiveOrExternal,
                "/tmp/x",
                &json!({"path": "/tmp/x"}),
                false
            ),
            GateDecision::RequireExplicitApproval { .. }
        ));
        assert_eq!(
            guard.evaluate(
                "delete_file",
                RiskTier::Tier2DestructiveOrExternal,
                "/tmp/x",
                &json!({"path": "/tmp/x"}),
                true
            ),
            GateDecision::Allow
        );

        // execute_guarded prevents unapproved Tier 2 execution.
        let mut ran = false;
        let err = guard
            .execute_guarded(
                "exec_shell",
                RiskTier::Tier2DestructiveOrExternal,
                "ls",
                &json!({}),
                false,
                || {
                    ran = true;
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(!ran);
        assert!(matches!(err, AiError::HitlConfirmationRequired { .. }));
    }
}
