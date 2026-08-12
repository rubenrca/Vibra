//! Thin Rust surface for the embedded Sparkle updater.
//!
//! The Objective-C bridge only starts when the process is a packaged `.app`
//! that carries `SUFeedURL` in its Info.plist. Development `cargo run` builds
//! stay inert.

use std::sync::Once;

static START: Once = Once::new();

unsafe extern "C" {
    fn vibra_sparkle_start();
    fn vibra_sparkle_check_for_updates();
}

/// Start automatic update checks if this is a packaged Vibra.app.
pub fn start() {
    START.call_once(|| {
        // Safety: bridge is re-entrant and main-thread aware.
        unsafe { vibra_sparkle_start() };
    });
}

/// Open Sparkle's check-for-updates UI (menu / palette).
pub fn check_for_updates() {
    start();
    unsafe { vibra_sparkle_check_for_updates() };
}
