//! `aitools-js-bridge` — ZTools/uTools WebView shim, CommonJS `NodePreloadRuntime` sidecar,
//! and lazy QuickJS pure-JS fallback (`SPEC-004`, `SPEC-008`, `SPEC-015`).
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-core` and `aitools-telemetry`.

pub mod callback_proxy;
pub mod lazy_worker;
pub mod node_runtime;
pub mod rust_node_compat;
pub mod shim;

pub use callback_proxy::{
    invoke_preload_list_feature, CallbackHandle, CallbackTable, ListItem, ListPushEvent,
    ListPushSink, RecordingListSink,
};
pub use lazy_worker::{JsBridgeHost, PluginActivationState};
pub use node_runtime::{
    resolve_node_binary, ExportedFeatureInfo, ExportedWindowProp, HostBridgeHandler,
    NodePreloadRuntime, NodeRuntimeConfig, NoopHostBridgeHandler, PreloadLoadInfo,
};
pub use rust_node_compat::{
    PreloadCapabilities, PreloadExecutionMode, PreloadRuntime, PreloadTier,
};
pub use shim::{
    build_initialization_script, build_initialization_script_with_preload_props,
    build_triple_global_shim, TRIPLE_GLOBAL_SHIM_JS,
};

/// In-process `rquickjs` pure-JS fallback runtime preserved for environments without Node.js (`SPEC-015-R2`).
pub type LegacyPreloadRuntime = JsBridgeHost;

use thiserror::Error;

/// Errors returned by the Node.js preload sidecar, QuickJS worker, and callback proxy.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum JsBridgeError {
    /// Core manifest, matcher, or storage error.
    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    /// QuickJS runtime initialization or evaluation failure.
    #[error("quickjs error: {0}")]
    QuickJs(String),

    /// Node.js preload sidecar spawn, protocol, or JS execution failure.
    #[error("node runtime error: {0}")]
    NodeRuntime(String),

    /// Pure-Rust QuickJS Node compatibility gap triggering optional System Node fallback.
    #[error("node compatibility gap in plugin '{plugin_id}': {capability}")]
    NodeCompatGap {
        /// Plugin identifier that encountered the unsupported Node/native capability.
        plugin_id: String,
        /// Human-readable description of the missing Node capability or native module.
        capability: String,
    },

    /// Bidirectional host RPC failure dispatched from preload code.
    #[error("host rpc error: {0}")]
    HostRpc(String),

    /// Preload operation exceeded its bounded execution deadline.
    #[error("timeout error: {0}")]
    Timeout(String),

    /// JSON serialization or deserialization failure on stdio RPC frames.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Filesystem or stdio pipe I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for [`JsBridgeError`].
pub type Result<T> = std::result::Result<T, JsBridgeError>;
