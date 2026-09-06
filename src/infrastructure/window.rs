/// Starts a native drag for the current window from the mouse-down event that
/// AppKit is dispatching. Window movement is opt-in on macOS so interactive
/// titlebar controls (notably terminal tabs) keep their own drag gestures.
pub fn start_drag() {
    #[cfg(target_os = "macos")]
    unsafe {
        vibra_start_window_drag();
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn vibra_start_window_drag();
    fn vibra_restore_window_frame(view: *mut std::ffi::c_void);
}

/// Restore the native frame and keep subsequent moves/resizes in AppKit's defaults.
pub fn restore_frame(window: &gpui::Window) {
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::AppKit(handle) = handle.as_raw()
        {
            // GPUI owns the view; it remains alive for the duration of this call.
            unsafe { vibra_restore_window_frame(handle.ns_view.as_ptr()) };
        }
    }
}
