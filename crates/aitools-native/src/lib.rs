//! `aitools-native` — Native OS hooks, hotkey edge detector, bounded app catalog,
//! pre-focus context capture, and OS Keychain credential storage (`SPEC-001`).
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-telemetry` within the workspace.

pub mod app_catalog;
pub mod context;
pub mod hotkey;
pub mod icon_analysis;
pub mod secrets;
pub mod vibrancy;

use thiserror::Error;

/// Errors returned by native OS integration primitives.
#[derive(Debug, Error)]
pub enum NativeError {
    #[error("hotkey rejected: {0}")]
    HotkeyRejected(String),

    #[error("clipboard error: {0}")]
    Clipboard(String),

    #[error("catalog error: {0}")]
    Catalog(String),

    #[error("secret store error: {0}")]
    Secret(String),

    #[error("window error: {0}")]
    Window(String),
}

/// Convenience result alias for [`NativeError`].
pub type Result<T> = std::result::Result<T, NativeError>;
