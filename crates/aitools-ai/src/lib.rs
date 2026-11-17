//! `aitools-ai` — Native AI & Agent-Based Tool Platform (`SPEC-007`, `SPEC-009-R2/R5`, `INV-SEC-04`).
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-core` and `aitools-telemetry`.

pub mod broker;
pub mod context_fusion;
pub mod harness;
pub mod hitl;
pub mod launcher;
pub mod mcp_client;
pub mod planner;
pub mod registry;
pub mod skills;
pub mod stream;

use thiserror::Error;

/// Errors returned by the AI broker, HITL policy guard, planner, and skill parser.
#[derive(Debug, Error)]
pub enum AiError {
    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    #[error("hitl confirmation required ({tier:?}) for tool '{tool}': {reason}")]
    HitlConfirmationRequired {
        tool: String,
        tier: aitools_core::RiskTier,
        reason: String,
    },

    #[error("hitl denied for tool '{tool}': {reason}")]
    HitlDenied { tool: String, reason: String },

    #[error("quota exceeded for plugin '{plugin_id}': {reason}")]
    QuotaExceeded { plugin_id: String, reason: String },

    #[error("provider error: {0}")]
    Provider(String),

    #[error("invalid skill document: {0}")]
    Skill(String),
}

/// Convenience result alias for [`AiError`].
pub type Result<T> = std::result::Result<T, AiError>;
