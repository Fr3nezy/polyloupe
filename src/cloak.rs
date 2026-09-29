//! DWM "cloaking": a cloaked window counts as shown for Windows (it can be maximized and painted)
//! but isn't drawn on screen. At startup a window restored maximized stays cloaked until a frame
//! at its final size is painted, so it appears in one piece.

use eframe::egui_wgpu::wgpu::rwh::HasWindowHandle;

/// Cloaks or uncloaks the window. Returns false when that isn't possible (other platforms, or no
/// Win32 handle), in which case the window simply stays drawn.
pub fn set_cloaked(window: &impl HasWindowHandle, cloaked: bool) -> bool {
    #[cfg(windows)]
    {
        use eframe::egui_wgpu::wgpu::rwh::RawWindowHandle;
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
        let Ok(handle) = window.window_handle() else { return false };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else { return false };
        let value: i32 = cloaked.into();
        // SAFETY: a live window handle from eframe and a pointer to a 4-byte BOOL.
        let hr = unsafe {
            DwmSetWindowAttribute(
                win32.hwnd.get() as _,
                DWMWA_CLOAK as u32,
                (&value as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            )
        };
        hr >= 0
    }
    #[cfg(not(windows))]
    {
        let _ = (window, cloaked);
        false
    }
}
