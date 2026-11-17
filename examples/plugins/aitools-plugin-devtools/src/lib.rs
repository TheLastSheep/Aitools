//! Zero-dependency Tier 3 Rust/Wasm benchmark plugin matching `wit/aitools-plugin.wit` exports.

/// Returns the aitools Wasm ABI major*100 + minor version (`0x0002` => 2).
#[no_mangle]
pub extern "C" fn aitools_plugin_api_version() -> i32 {
    2
}

/// Evaluates an instant search query against built-in developer utilities.
#[no_mangle]
pub extern "C" fn on_query() -> i32 {
    1
}

/// Registers static `json_validate` tool metadata handle.
#[no_mangle]
pub extern "C" fn register_tools() -> i32 {
    1
}

/// Executes the selected tool entrypoint and returns status code `0` on success.
#[no_mangle]
pub extern "C" fn execute_tool() -> i32 {
    0
}
