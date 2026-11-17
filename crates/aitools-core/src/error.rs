use thiserror::Error;

/// Unified error type for `aitools-core`.
#[derive(Debug, Error)]
pub enum AitoolsError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("lmdb error: {0}")]
    Lmdb(#[from] heed::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("manifest validation error: {0}")]
    Manifest(String),

    #[error("forbidden (403): {0}")]
    Forbidden(String),

    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
}

pub type Result<T> = std::result::Result<T, AitoolsError>;
