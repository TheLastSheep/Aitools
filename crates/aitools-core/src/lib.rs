//! `aitools-core` — Core manifest models, LMDB storage (`heed`), and zero-alloc hybrid search.
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-telemetry` within the workspace.

pub mod error;
pub mod matcher;
pub mod models;
pub mod storage;

pub use error::{AitoolsError, Result};
pub use matcher::{
    search_all, search_all_checked, to_pinyin_full, to_pinyin_initials, FeatureSearchIndex,
    MatchResult, MediaFile, MAX_MEDIA_FILES, MAX_MEDIA_PATH_BYTES, MAX_MEDIA_PATH_TOTAL_BYTES,
    MAX_MEDIA_RULE_NAME_BYTES, MAX_MEDIA_RULE_NAME_WORK, MAX_TYPED_TEXT_REGEX_WORK,
};
pub use models::*;
pub use storage::{
    validate_sync_caller, AttachmentRecord, MacroStep, PluginDataQuery, PluginDataRemove,
    PluginDataWriteResult, SessionScratchpad, StorageEngine, WorkflowMacro, MAX_DOCUMENT_BYTES,
    MAX_KEYS_SELECTOR, MAX_QUERY_DOCUMENTS, MAX_RESPONSE_BYTES,
};
