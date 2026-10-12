//! Pure helpers for turn headers and message timestamps: elapsed time and
//! local clock time.

use std::time::SystemTime;

/// Elapsed time for a turn header: "12s", "1m 53s", "1h 2m".
pub(in crate::ui::agent_chat) fn format_elapsed(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Local wall-clock time of `at` as "HH:MM", for message timestamps.
pub(in crate::ui::agent_chat) fn clock_time(at: SystemTime) -> Option<String> {
    let seconds = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let time = seconds.min(i64::MAX as u64) as libc::time_t;
    // SAFETY: `localtime_r` only writes the zeroed `tm` it is given.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut local) }.is_null() {
        return None;
    }
    Some(format!("{:02}:{:02}", local.tm_hour, local.tm_min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_drops_units_that_are_zero() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(12), "12s");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(113), "1m 53s");
        assert_eq!(format_elapsed(3599), "59m 59s");
        assert_eq!(format_elapsed(3600), "1h 0m");
        assert_eq!(format_elapsed(3720), "1h 2m");
    }

    #[test]
    fn clock_time_is_hours_and_minutes() {
        let text = clock_time(SystemTime::now()).expect("local time");
        assert_eq!(text.len(), 5);
        assert_eq!(&text[2..3], ":");
        assert!(clock_time(SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1)).is_none());
    }
}
