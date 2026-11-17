//! `aitools-js-bridge` — Tier 1 `legacy-compat` JS bridge & lazy QuickJS preload runtime (`SPEC-008`).
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-core` and `aitools-telemetry`.

pub mod callback_proxy;
pub mod lazy_worker;
pub mod shim;

pub use callback_proxy::{
    invoke_preload_list_feature, CallbackHandle, CallbackTable, ListItem, ListPushEvent,
    ListPushSink, RecordingListSink,
};
pub use lazy_worker::{JsBridgeHost, PluginActivationState};
pub use shim::{build_initialization_script, build_triple_global_shim, TRIPLE_GLOBAL_SHIM_JS};

use thiserror::Error;

/// Errors returned by the QuickJS preload worker and callback proxy.
#[derive(Debug, Error)]
pub enum JsBridgeError {
    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    #[error("quickjs error: {0}")]
    QuickJs(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for [`JsBridgeError`].
pub type Result<T> = std::result::Result<T, JsBridgeError>;
