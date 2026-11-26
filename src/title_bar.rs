//! Light or dark window title bar, matching the editor's theme.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// winit switches the title bar with an undocumented call that Windows 11 applies only
/// partly (e.g. dark background with black text), so the documented attribute is set too.
pub fn set_dark(window: &impl HasWindowHandle, dark: bool) {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    let value = i32::from(dark);
    // SAFETY: valid window handle, and a pointer to a 4-byte BOOL as the attribute expects.
    unsafe {
        DwmSetWindowAttribute(h.hwnd.get(), DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const value).cast(), 4);
    }
}
