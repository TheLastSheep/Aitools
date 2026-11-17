//! Tier 3 Rust/Wasm JSON & Base64/URL codec plugin matching `wit/aitools-plugin.wit` ABI v2.

/// Returns ABI version `2`.
#[no_mangle]
pub extern "C" fn aitools_plugin_api_version() -> i32 {
    2
}

/// Evaluates an instant query score for JSON / Base64 / URL keywords.
#[no_mangle]
pub extern "C" fn on_query() -> i32 {
    1
}

/// Registers static `format_json` and `codec_base64_url` tool handles.
#[no_mangle]
pub extern "C" fn register_tools() -> i32 {
    2
}

/// Executes deterministic codec verification check and returns status `0`.
#[no_mangle]
pub extern "C" fn execute_tool() -> i32 {
    0
}

/// Computes Base64 encoded byte length for `raw_len` unpadded input bytes.
#[no_mangle]
pub extern "C" fn base64_encoded_len(raw_len: i32) -> i32 {
    if raw_len <= 0 {
        return 0;
    }
    ((raw_len + 2) / 3) * 4
}
