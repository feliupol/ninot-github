//! Timestamps formatted as GitHub formats them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current UTC time as GitHub formats it, such as `2026-09-29T20:38:27Z`.
pub fn now() -> String {
    format_timestamp(unix_now())
}

/// Seconds since the Unix epoch.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn format_timestamp(seconds: u64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps_like_github() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_790_714_307), "2026-09-29T20:38:27Z");
        assert_eq!(format_timestamp(951_782_400), "2000-02-29T00:00:00Z");
    }
}
