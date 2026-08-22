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
}
