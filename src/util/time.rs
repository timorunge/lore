//! ISO 8601 timestamp formatting.

use std::time::Duration;

const SECS_PER_DAY: u64 = 86400;

/// Current time as a `Duration` since the Unix epoch.
pub fn unix_now() -> Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|e| {
            tracing::warn!("system clock is before the Unix epoch: {e}");
            Duration::ZERO
        })
}

/// Return the current UTC time as an ISO 8601 string.
pub fn iso8601_now() -> String {
    let now = unix_now().as_secs();
    let days = now / SECS_PER_DAY;
    let time_of_day = now % SECS_PER_DAY;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Converts days since the Unix epoch to (year, month, day) using Hinnant's algorithm.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_to_ymd_cases() {
        for (secs, expected) in [
            (0u64, "1970-01-01T00:00:00Z"),
            (1_704_067_200, "2024-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_735_689_599, "2024-12-31T23:59:59Z"),
        ] {
            let (year, month, day) = days_to_ymd(secs / 86400);
            let h = (secs % 86400) / 3600;
            let m = (secs % 3600) / 60;
            let s = secs % 60;
            let result = format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z");
            assert_eq!(result, expected, "iso8601({secs}) should be {expected:?}");
        }

        // Year 2100 is NOT a leap year (divisible by 100 but not 400).
        // This exercises the leap-year boundary in Hinnant's algorithm.
        // Unix timestamp for 2100-02-28T00:00:00Z:
        //   days from epoch to 2100-01-01 = 47482, then + 31 (Jan) + 27 (Feb 1..28) = 47540
        let feb28_2100_secs: u64 = 47540 * 86400;
        let (y, m, d) = days_to_ymd(feb28_2100_secs / 86400);
        assert_eq!(
            (y, m, d),
            (2100, 2, 28),
            "2100-02-28 should be correctly decoded"
        );
        // The day after should be 2100-03-01, not 2100-02-29.
        let mar01_2100_secs = feb28_2100_secs + 86400;
        let (y, m, d) = days_to_ymd(mar01_2100_secs / 86400);
        assert_eq!(
            (y, m, d),
            (2100, 3, 1),
            "day after 2100-02-28 must be 2100-03-01 (2100 is not a leap year)"
        );

        // Late-epoch boundary: last second of year 2099.
        // days from epoch to 2099-01-01: 47117 days, + 364 days = 47481, time = 86399
        let secs: u64 = 47481 * 86400 + 86399;
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let mins = (secs % 3600) / 60;
        let sec = secs % 60;
        let (y, mo, day) = days_to_ymd(days);
        assert_eq!((y, mo, day), (2099, 12, 31));
        assert_eq!((hours, mins, sec), (23, 59, 59));

        // Unix epoch (t=0) decodes as 1970-01-01. Values before the epoch are not
        // representable by `unix_now()` (which returns `Duration` and clamps to 0),
        // so the minimum meaningful timestamp is the epoch itself.
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1), "day 0 must be 1970-01-01");
    }
}
