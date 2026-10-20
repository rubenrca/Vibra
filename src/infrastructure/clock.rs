//! Wall-clock time for timestamps the app shows or caches.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch; zero if the clock is before it.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// The system time zone's offset from UTC, in seconds, at the given instant.
/// Daylight saving makes this depend on the date; falls back to UTC if the
/// platform cannot resolve it.
#[cfg(unix)]
pub fn local_utc_offset(unix: i64) -> i32 {
    let time = unix as libc::time_t;
    // SAFETY: `localtime_r` only reads `time` and fills the zeroed `tm` we own.
    unsafe {
        let mut parts: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut parts).is_null() {
            return 0;
        }
        i32::try_from(parts.tm_gmtoff).unwrap_or(0)
    }
}

#[cfg(not(unix))]
pub fn local_utc_offset(_unix: i64) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_offset_stays_within_real_time_zones() {
        // UTC-12 to UTC+14 covers every zone in use.
        let offset = local_utc_offset(unix_now() as i64);
        assert!((-12 * 3600..=14 * 3600).contains(&offset));
    }
}
