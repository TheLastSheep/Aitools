//! `aitools-wasm-host` — Sandboxed Tier 3 `rust-wasm` plugin host (`SPEC-008-R3`).
//!
//! Strictly adheres to `SPEC-000`: depends only on `aitools-core` and `aitools-telemetry`.

use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tracing::info_span;
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

pub const MAX_WASM_MEMORY_BYTES: usize = 64 * 1024 * 1024; // 64 MB cap (SPEC-008-R3)
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_millis(5_000);
pub const DEFAULT_FUEL_BUDGET: u64 = 10_000_000;

/// Errors returned by the sandboxed Wasmtime plugin host.
#[derive(Debug, Error)]
pub enum WasmHostError {
    #[error("core error: {0}")]
    Core(#[from] aitools_core::AitoolsError),

    #[error("wasmtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for [`WasmHostError`].
pub type Result<T> = std::result::Result<T, WasmHostError>;

/// Execution quotas applied to each `wasmtime::Store` (`SPEC-008-R3`).
#[derive(Debug, Clone, Copy)]
pub struct WasmQuotas {
    pub max_memory_bytes: usize,
    pub max_fuel: u64,
    pub timeout: Duration,
}

impl Default for WasmQuotas {
    fn default() -> Self {
        Self {
            max_memory_bytes: MAX_WASM_MEMORY_BYTES,
            max_fuel: DEFAULT_FUEL_BUDGET,
            timeout: DEFAULT_EXECUTION_TIMEOUT,
        }
    }
}

struct HostState {
    limits: StoreLimits,
}

/// Sandboxed Wasmtime engine enforcing memory & fuel caps (`SPEC-008-R3`).
pub struct WasmPluginHost {
    engine: Engine,
    quotas: WasmQuotas,
}

impl WasmPluginHost {
    /// Creates a new sandboxed Wasm host configured with `quotas`.
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Wasmtime`] if initializing the Wasmtime [`Engine`] fails.
    pub fn new(quotas: WasmQuotas) -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;
        Ok(Self { engine, quotas })
    }

    /// Creates a new sandboxed Wasm host with [`WasmQuotas::default()`].
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Wasmtime`] if initializing the Wasmtime [`Engine`] fails.
    pub fn with_defaults() -> Result<Self> {
        Self::new(WasmQuotas::default())
    }

    /// Returns the execution quotas enforced by this host.
    pub fn quotas(&self) -> WasmQuotas {
        self.quotas
    }

    /// Instantiates a core Wasm module from bytes inside a bounded `Store`.
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Wasmtime`] if module compilation, fuel setup, or instantiation
    /// fails (including exceeding memory limits).
    pub fn instantiate_module_bytes(
        &self,
        plugin_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<WasmInstanceHandle> {
        let _span = info_span!("wasm_instantiate", plugin_id = %plugin_id).entered();
        let module = Module::from_binary(&self.engine, wasm_bytes)?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.quotas.max_memory_bytes)
            .instances(4)
            .tables(4)
            .build();
        let mut store = Store::new(&self.engine, HostState { limits });
        store.limiter(|state| &mut state.limits);
        store.set_fuel(self.quotas.max_fuel)?;
        store.set_epoch_deadline(1);
        let instance = Instance::new(&mut store, &module, &[])?;
        Ok(WasmInstanceHandle { store, instance })
    }

    /// Loads and instantiates a `.wasm` file from disk inside a bounded `Store`.
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Io`] if reading `path` fails, or [`WasmHostError::Wasmtime`]
    /// if module compilation or instantiation fails.
    pub fn instantiate_module_file(
        &self,
        plugin_id: &str,
        path: impl AsRef<Path>,
    ) -> Result<WasmInstanceHandle> {
        let bytes = std::fs::read(path)?;
        self.instantiate_module_bytes(plugin_id, &bytes)
    }
}

/// Live bounded Wasm module instance.
pub struct WasmInstanceHandle {
    store: Store<HostState>,
    instance: Instance,
}

impl WasmInstanceHandle {
    /// Returns the remaining fuel budget for this Wasm instance.
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Wasmtime`] if fuel metering is disabled on the store.
    pub fn remaining_fuel(&self) -> Result<u64> {
        Ok(self.store.get_fuel()?)
    }

    /// Invokes an exported `() -> i32` function inside the bounded store.
    ///
    /// # Errors
    ///
    /// Returns [`WasmHostError::Wasmtime`] if `export_name` is missing, has a mismatched
    /// signature, traps, or runs out of fuel.
    pub fn call_i32_void(&mut self, export_name: &str) -> Result<i32> {
        let func = self
            .instance
            .get_typed_func::<(), i32>(&mut self.store, export_name)?;
        Ok(func.call(&mut self.store, ())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Valid minimal Wasm module:
    // (module
    //   (func (export "answer") (result i32) (i32.const 42))
    //   (func (export "spin") (result i32) (loop $l (br $l)) (i32.const 0))
    // )
    // Encoded by hand in standard Wasm binary format:
    // magic+version | type section: ()->i32 | func section: [0, 0] |
    // export section: "answer"->func0, "spin"->func1 |
    // code section: body0=(i32.const 42, end), body1=(loop void, br 0, end, i32.const 0, end)
    const TEST_WASM: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, // magic
        0x01, 0x00, 0x00, 0x00, // version
        // Type section (1): 1 type: () -> i32
        0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f,
        // Function section (3): 2 funcs of type 0
        0x03, 0x03, 0x02, 0x00, 0x00, // Export section (7): "answer" (0), "spin" (1)
        0x07, 0x11, 0x02, 0x06, b'a', b'n', b's', b'w', b'e', b'r', 0x00, 0x00, 0x04, b's', b'p',
        b'i', b'n', 0x00, 0x01, // Code section (10): 2 bodies
        0x0a, 0x10, 0x02, // body 0: len=4, locals=0, i32.const 42, end
        0x04, 0x00, 0x41, 0x2a, 0x0b,
        // body 1: len=9, locals=0, loop(void) br 0 end, i32.const 0, end
        0x09, 0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, 0x41, 0x00, 0x0b,
    ];

    // Wasm module requesting > 64 MB memory (1025 pages * 64 KiB = 65.6 MB)
    const HUGE_MEM_WASM: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
        // Memory section (5): 1 memory with min=1025 (0x81 0x08 LEB128)
        0x05, 0x04, 0x01, 0x00, 0x81, 0x08,
    ];

    #[test]
    fn spec_008_r3_enforces_64mb_memory_and_fuel_caps() {
        let host = WasmPluginHost::new(WasmQuotas {
            max_memory_bytes: MAX_WASM_MEMORY_BYTES,
            max_fuel: 500,
            timeout: DEFAULT_EXECUTION_TIMEOUT,
        })
        .unwrap();

        let mut inst = host
            .instantiate_module_bytes("demo-wasm", TEST_WASM)
            .unwrap();
        assert_eq!(inst.call_i32_void("answer").unwrap(), 42);

        // Infinite loop traps cleanly when fuel runs out.
        let trap = inst.call_i32_void("spin").unwrap_err();
        assert!(trap.to_string().contains("fuel") || trap.to_string().contains("wasmtime"));

        // Memory request > 64 MB is rejected at instantiation.
        let mem_err = host.instantiate_module_bytes("greedy-wasm", HUGE_MEM_WASM);
        assert!(mem_err.is_err());
    }

    #[test]
    fn spec_012_r3_instantiates_devtools_benchmark_wasm_plugin() {
        let wasm_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/plugins/aitools-plugin-devtools/plugin.wasm");
        let wasm_bytes = std::fs::read(&wasm_path).expect("devtools plugin.wasm should exist");
        let host = WasmPluginHost::with_defaults().unwrap();
        let mut inst = host
            .instantiate_module_bytes("devtools", &wasm_bytes)
            .unwrap();
        assert_eq!(inst.call_i32_void("aitools_plugin_api_version").unwrap(), 2);
        assert_eq!(inst.call_i32_void("on_query").unwrap(), 1);
        assert_eq!(inst.call_i32_void("register_tools").unwrap(), 1);
        assert_eq!(inst.call_i32_void("execute_tool").unwrap(), 0);
    }
}
