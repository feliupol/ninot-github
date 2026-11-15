//! Timestamps formatted as GitHub formats them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};

/// Current UTC time as GitHub formats it, such as `2026-09-29T20:38:27Z`.
pub fn now() -> String {
    format_timestamp(unix_now())
}

/// Current UTC time to the second, the instant [`now`] formats.
pub(crate) fn now_utc() -> DateTime<Utc> {
    let seconds = i64::try_from(unix_now()).expect("the clock fits in i64");
    DateTime::from_timestamp(seconds, 0).expect("the clock is in chrono's range")
}

/// Parses an RFC 3339 timestamp, such as `2026-09-29T20:38:27Z` or one with an offset, into UTC.
pub(crate) fn parse(text: &str) -> chrono::ParseResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).map(|time| time.with_timezone(&Utc))
}

/// `time` as GitHub formats it, to the second.
pub(crate) fn format(time: &DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
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

    #[test]
    fn now_utc_is_whole_seconds_like_now() {
        let now = now_utc();
        assert_eq!(now.timestamp_subsec_nanos(), 0);
        assert_eq!(parse(&format_timestamp(now.timestamp() as u64)), Ok(now));
    }

    #[test]
    fn parses_offsets_into_utc() {
        assert_eq!(
            parse("2026-09-29T22:38:27+02:00").map(|time| time.timestamp()),
            Ok(1_790_714_307)
        );
        assert!(parse("yesterday").is_err());
    }
}
