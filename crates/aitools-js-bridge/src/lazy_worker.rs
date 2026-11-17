//! Lazy `rquickjs` preload worker (`SPEC-008-R2`, `SPEC-000` `quickjs_cold_start`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{mpsc, Mutex};
use std::thread::{self, JoinHandle};

use aitools_core::PluginManifest;
use rquickjs::{Context, Runtime};
use tracing::info_span;

use crate::shim::build_initialization_script;
use crate::{JsBridgeError, Result};

/// Activation status for a `legacy-compat` plugin (`SPEC-008-R2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginActivationState {
    /// Plugin has no `"preload"` field; no QuickJS runtime was spawned.
    WebViewOnlyNoQuickJs,
    /// Plugin declared `"preload"` and has a live lazy QuickJS context.
    QuickJsPreloadActive,
}

enum WorkerCmd {
    Activate {
        plugin_id: String,
        preload_code: String,
        reply: mpsc::Sender<Result<()>>,
    },
    EvalString {
        plugin_id: String,
        script: String,
        reply: mpsc::Sender<Result<String>>,
    },
    Unload {
        plugin_id: String,
        reply: mpsc::Sender<()>,
    },
    Shutdown,
}

struct WorkerClient {
    tx: Mutex<mpsc::Sender<WorkerCmd>>,
    handle: Option<JoinHandle<()>>,
}

impl WorkerClient {
    fn spawn() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        let handle = thread::Builder::new()
            .name("aitools-quickjs".into())
            .spawn(move || worker_loop(rx))
            .map_err(JsBridgeError::Io)?;
        Ok(Self {
            tx: Mutex::new(tx),
            handle: Some(handle),
        })
    }

    fn send(&self, cmd: WorkerCmd) -> Result<()> {
        self.tx
            .lock()
            .expect("quickjs worker tx mutex poisoned")
            .send(cmd)
            .map_err(|_| JsBridgeError::QuickJs("quickjs worker thread exited".into()))
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(WorkerCmd::Shutdown);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct QuickJsPluginContext {
    _runtime: Runtime,
    context: Context,
}

fn worker_loop(rx: mpsc::Receiver<WorkerCmd>) {
    let mut contexts: HashMap<String, QuickJsPluginContext> = HashMap::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WorkerCmd::Activate {
                plugin_id,
                preload_code,
                reply,
            } => {
                let res = (|| -> Result<()> {
                    let runtime =
                        Runtime::new().map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
                    runtime.set_memory_limit(16 * 1024 * 1024);
                    let context = Context::full(&runtime)
                        .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
                    let bootstrap = build_initialization_script(&plugin_id);
                    context.with(|ctx| -> Result<()> {
                        ctx.eval::<(), _>(bootstrap)
                            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
                        ctx.eval::<(), _>(preload_code.as_str())
                            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))?;
                        Ok(())
                    })?;
                    contexts.insert(
                        plugin_id,
                        QuickJsPluginContext {
                            _runtime: runtime,
                            context,
                        },
                    );
                    Ok(())
                })();
                let _ = reply.send(res);
            }
            WorkerCmd::EvalString {
                plugin_id,
                script,
                reply,
            } => {
                let res = match contexts.get(&plugin_id) {
                    Some(worker) => worker.context.with(|ctx| {
                        ctx.eval::<String, _>(script.as_str())
                            .map_err(|e| JsBridgeError::QuickJs(e.to_string()))
                    }),
                    None => Err(JsBridgeError::QuickJs(format!(
                        "no active QuickJS worker for '{plugin_id}'"
                    ))),
                };
                let _ = reply.send(res);
            }
            WorkerCmd::Unload { plugin_id, reply } => {
                contexts.remove(&plugin_id);
                let _ = reply.send(());
            }
            WorkerCmd::Shutdown => break,
        }
    }
}

/// Hosts lazy per-plugin QuickJS runtimes only for plugins that declare `"preload"`.
#[derive(Default)]
pub struct JsBridgeHost {
    worker: Option<WorkerClient>,
    active_plugins: HashSet<String>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<JsBridgeHost>();
};

impl JsBridgeHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Activates a legacy plugin (`SPEC-008-R2`).
    ///
    /// Guarantees that plugins without a `"preload"` field NEVER spawn an `rquickjs` worker.
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::Io`] if reading the preload script fails, or
    /// [`JsBridgeError::QuickJs`] if spawning or initializing the QuickJS worker fails.
    pub fn activate_legacy_plugin(
        &mut self,
        plugin_id: &str,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> Result<PluginActivationState> {
        if !manifest.requires_quickjs_preload() {
            return Ok(PluginActivationState::WebViewOnlyNoQuickJs);
        }

        let preload_rel = manifest.preload.as_deref().unwrap_or_default();
        let preload_path = plugin_dir.join(preload_rel);
        let preload_code = std::fs::read_to_string(&preload_path)?;
        self.activate_preload_source(plugin_id, &preload_code)?;
        Ok(PluginActivationState::QuickJsPreloadActive)
    }

    /// Evaluates `TRIPLE_GLOBAL_SHIM_JS` + `preload_code` in an isolated QuickJS context.
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::Io`] if spawning the worker thread fails, or
    /// [`JsBridgeError::QuickJs`] if initializing or evaluating `preload_code` fails.
    pub fn activate_preload_source(&mut self, plugin_id: &str, preload_code: &str) -> Result<()> {
        let _span = info_span!("quickjs_cold_start", plugin_id = %plugin_id).entered();
        if self.worker.is_none() {
            self.worker = Some(WorkerClient::spawn()?);
        }
        let client = self
            .worker
            .as_ref()
            .expect("worker initialized immediately above");
        let (reply_tx, reply_rx) = mpsc::channel();
        client.send(WorkerCmd::Activate {
            plugin_id: plugin_id.to_string(),
            preload_code: preload_code.to_string(),
            reply: reply_tx,
        })?;
        let res = reply_rx
            .recv()
            .map_err(|_| JsBridgeError::QuickJs("quickjs worker dropped reply".into()))?;
        res?;
        self.active_plugins.insert(plugin_id.to_string());
        Ok(())
    }

    /// Evaluates an expression in an active plugin's QuickJS context and returns the string result.
    ///
    /// # Errors
    ///
    /// Returns [`JsBridgeError::QuickJs`] if no worker is active for `plugin_id`, the worker
    /// thread exits unexpectedly, or script evaluation throws a JS exception.
    pub fn eval_string(&self, plugin_id: &str, script: &str) -> Result<String> {
        let Some(ref client) = self.worker else {
            return Err(JsBridgeError::QuickJs(format!(
                "no active QuickJS worker for '{plugin_id}'"
            )));
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        client.send(WorkerCmd::EvalString {
            plugin_id: plugin_id.to_string(),
            script: script.to_string(),
            reply: reply_tx,
        })?;
        reply_rx
            .recv()
            .map_err(|_| JsBridgeError::QuickJs("quickjs worker dropped reply".into()))?
    }

    #[must_use]
    pub fn active_worker_count(&self) -> usize {
        self.active_plugins.len()
    }

    #[must_use]
    pub fn has_worker(&self, plugin_id: &str) -> bool {
        self.active_plugins.contains(plugin_id)
    }

    pub fn unload(&mut self, plugin_id: &str) {
        self.active_plugins.remove(plugin_id);
        if let Some(ref client) = self.worker {
            let (reply_tx, reply_rx) = mpsc::channel();
            if client
                .send(WorkerCmd::Unload {
                    plugin_id: plugin_id.to_string(),
                    reply: reply_tx,
                })
                .is_ok()
            {
                let _ = reply_rx.recv();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn manifest_with_preload(preload: Option<&str>) -> PluginManifest {
        PluginManifest {
            platform: vec![],
            name: "demo".into(),
            title: None,
            version: "1.0.0".into(),
            main: Some("index.html".into()),
            logo: None,
            preload: preload.map(str::to_string),
            description: None,
            author: None,
            homepage: None,
            plugin_setting: None,
            features: vec![],
            development: None,
            tools: HashMap::new(),
            providers: HashMap::new(),
            permissions: vec![],
        }
    }

    #[test]
    fn spec_008_r1_triple_global_aliasing_is_identical_and_frozen() {
        let mut host = JsBridgeHost::new();
        host.activate_preload_source("color-helper", "globalThis.__loaded = true;")
            .unwrap();

        let check = host
            .eval_string(
                "color-helper",
                r#"JSON.stringify({
                    sameAZ: window.aitools === window.ztools,
                    sameZU: window.ztools === window.utools,
                    frozen: Object.isFrozen(window.aitools),
                    dbFrozen: Object.isFrozen(window.ztools.db),
                    pluginId: window.utools.__pluginId,
                    loaded: globalThis.__loaded === true
                })"#,
            )
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&check).unwrap();
        assert_eq!(parsed["sameAZ"], true);
        assert_eq!(parsed["sameZU"], true);
        assert_eq!(parsed["frozen"], true);
        assert_eq!(parsed["dbFrozen"], true);
        assert_eq!(parsed["pluginId"], "color-helper");
        assert_eq!(parsed["loaded"], true);
    }

    #[test]
    fn spec_008_r2_skips_quickjs_without_preload() {
        let mut host = JsBridgeHost::new();
        let state = host
            .activate_legacy_plugin(
                "no-preload",
                &manifest_with_preload(None),
                &PathBuf::from("."),
            )
            .unwrap();
        assert_eq!(state, PluginActivationState::WebViewOnlyNoQuickJs);
        assert_eq!(host.active_worker_count(), 0);
        assert!(!host.has_worker("no-preload"));
    }
}
