//! Visibility gates for periodic work that should sleep when nothing is on screen.

/// Cursor blink only when the pane is focused and the cursor is in blink mode.
pub fn should_run_cursor_blink(focused: bool, blinking: bool) -> bool {
    focused && blinking
}

/// CWD / agent identity polls only run for a pane that is currently painted.
pub fn should_poll_terminal_idle(surface_visible: bool) -> bool {
    surface_visible
}

/// Full Git snapshot poll only while the right sidebar is showing.
pub fn should_poll_git_snapshot(right_sidebar_visible: bool) -> bool {
    right_sidebar_visible
}

/// Sessions-sidebar branch metadata only while that surface is open.
pub fn should_poll_sidebar_git(left_sidebar_visible: bool, sessions_mode: bool) -> bool {
    left_sidebar_visible && sessions_mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_helpers_skip_hidden_surfaces() {
        assert!(!should_run_cursor_blink(false, true));
        assert!(!should_run_cursor_blink(true, false));
        assert!(should_run_cursor_blink(true, true));
        assert!(!should_poll_terminal_idle(false));
        assert!(should_poll_terminal_idle(true));
        assert!(!should_poll_git_snapshot(false));
        assert!(should_poll_git_snapshot(true));
        assert!(!should_poll_sidebar_git(false, true));
        assert!(!should_poll_sidebar_git(true, false));
        assert!(should_poll_sidebar_git(true, true));
    }
}
