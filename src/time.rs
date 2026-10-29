//! Timestamps as text. `ls` shows them now; comments and `view.json` will
//! use the same format.

use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: u64 = 86_400;

/// `time` as UTC RFC 3339 with second precision: `2026-10-28T19:02:11Z`.
/// Times before 1970 don't occur here; they print as the epoch.
pub fn rfc3339(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil_from_days(secs / SECS_PER_DAY);
    let secs_of_day = secs % SECS_PER_DAY;
    let (hour, minute, second) = (secs_of_day / 3600, secs_of_day / 60 % 60, secs_of_day % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since 1970-01-01 as `(year, month, day)`. This is Howard Hinnant's
/// `civil_from_days` (<https://howardhinnant.github.io/date_algorithms.html>),
/// which counts in 400-year eras starting on March 1, so leap days fall at
/// the end of a year. It's usually written with signed integers so it can
/// handle dates before 1970. Without those, every value here is
/// non-negative, so it's all `u64` and needs no casts.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468; // days since 0000-03-01
    let era = shifted / 146_097; // 146,097 days in 400 years
    let day_of_era = shifted - era * 146_097; // 0..=146_096
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153; // 0 is March
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    // January and February belong to the next calendar year.
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> String {
        rfc3339(UNIX_EPOCH + Duration::from_secs(secs))
    }

    // Expected strings come from `date`. To check one:
    //   macOS: date -u -r 1709210096 +%Y-%m-%dT%H:%M:%SZ
    //   Linux: date -u -d @1709210096 +%Y-%m-%dT%H:%M:%SZ
    #[test]
    fn formats_known_dates() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1_709_210_096), "2024-02-29T12:34:56Z"); // leap day
        assert_eq!(at(1_798_761_599), "2026-12-31T23:59:59Z"); // last second of a year
        assert_eq!(at(1_798_761_600), "2027-01-01T00:00:00Z"); // and the next one
    }

    #[test]
    fn times_before_1970_print_as_the_epoch() {
        let before = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(rfc3339(before), "1970-01-01T00:00:00Z");
    }
}
