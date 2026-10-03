//! Wall-clock time for timestamps the app shows or caches.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch; zero if the clock is before it.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
