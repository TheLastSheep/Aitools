//! Cross-platform window vibrancy & translucency helpers (`SPEC-017-R1`).

use raw_window_handle::HasWindowHandle;

use crate::Result;

/// Applies a ZTools-matched translucent HUD vibrancy material (`FullScreenUI`, `10px` radius) to a native window on macOS.
///
/// On non-macOS platforms this is a safe no-op returning `Ok(())`.
///
/// # Errors
///
/// Returns [`crate::NativeError::Window`] if the macOS window handle is unavailable or the
/// vibrancy effect cannot be attached.
pub fn apply_hud_vibrancy<W: HasWindowHandle>(window: &W) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        window_vibrancy::apply_vibrancy(
            window,
            window_vibrancy::NSVisualEffectMaterial::FullScreenUI,
            Some(window_vibrancy::NSVisualEffectState::Active),
            Some(10.0),
        )
        .map_err(|err| crate::NativeError::Window(err.to_string()))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_017_r1_vibrancy_symbol_compiles_safely() {
        // Compile-time linkage & signature check; window-handle requires a live GUI window.
        let _fn_ptr: fn(&tauri_stub::StubWindow) -> Result<()> = apply_hud_vibrancy;
    }

    mod tauri_stub {
        use raw_window_handle::{HandleError, HasWindowHandle, WindowHandle};

        pub struct StubWindow;

        impl HasWindowHandle for StubWindow {
            fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
                Err(HandleError::NotSupported)
            }
        }
    }
}
