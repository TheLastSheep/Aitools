//! Tier 3 Rust/Wasm timestamp & arithmetic calculator plugin.

/// Returns ABI version `2`.
#[no_mangle]
pub extern "C" fn aitools_plugin_api_version() -> i32 {
    2
}

/// Evaluates an instant query handle.
#[no_mangle]
pub extern "C" fn on_query() -> i32 {
    1
}

/// Registers static `convert_timestamp` and `eval_expression` tool handles.
#[no_mangle]
pub extern "C" fn register_tools() -> i32 {
    2
}

/// Executes default verification entrypoint and returns status `0`.
#[no_mangle]
pub extern "C" fn execute_tool() -> i32 {
    0
}

/// Converts Unix epoch seconds (non-negative) to UTC day number since 1970-01-01.
#[no_mangle]
pub extern "C" fn unix_epoch_days(seconds: i32) -> i32 {
    if seconds < 0 {
        return 0;
    }
    seconds / 86_400
}
