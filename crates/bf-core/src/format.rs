//! Text for the size and date columns.

use std::time::SystemTime;

/// Base-10 units, like Finder: `0 bytes`, `1 byte`, `532 bytes`, `1 KB`,
/// `12.4 MB`, `3.1 GB`, `1.2 TB`. One decimal for values under 100 in a unit
/// above bytes (`12.4 MB`), no decimal from 100 (`532 KB`). Drop a `.0`.
pub fn human_size(bytes: u64) -> String {
    if bytes == 0 {
        return "0 bytes".to_string();
    }
    if bytes == 1 {
        return "1 byte".to_string();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        let text = format!("{value:.1}");
        let text = text.strip_suffix(".0").unwrap_or(&text);
        format!("{text} {}", UNITS[unit])
    }
}

/// Formats `time` in the local time zone.
///
/// - `pattern` is a `strftime` pattern (chrono syntax), for example `%Y-%m-%d %H:%M`.
/// - The special pattern `finder` gives `Today at 14:32`, `Yesterday at 09:10`,
///   and `28 Sep 2026 at 13:02` for older dates. The app draws `finder` dates
///   with the system date formatter (it follows the locale); this text is
///   the fallback for tests and the CLI.
/// - The special pattern `relative` gives `Today 14:32`, `Yesterday 09:10`,
///   and `%Y-%m-%d` for older dates.
///
/// `now` is a parameter so that tests are deterministic.
pub fn format_time(time: SystemTime, pattern: &str, now: SystemTime) -> String {
    use chrono::{DateTime, Local};
    let time: DateTime<Local> = time.into();
    if pattern == "finder" {
        let now: DateTime<Local> = now.into();
        let day = time.date_naive();
        let today = now.date_naive();
        if day == today {
            return time.format("Today at %H:%M").to_string();
        }
        if today.pred_opt() == Some(day) {
            return time.format("Yesterday at %H:%M").to_string();
        }
        return time.format("%-d %b %Y at %H:%M").to_string();
    }
    if pattern == "relative" {
        let now: DateTime<Local> = now.into();
        let day = time.date_naive();
        let today = now.date_naive();
        if day == today {
            return time.format("Today %H:%M").to_string();
        }
        if today.pred_opt() == Some(day) {
            return time.format("Yesterday %H:%M").to_string();
        }
        return time.format("%Y-%m-%d").to_string();
    }
    let items: Vec<_> = chrono::format::StrftimeItems::new(pattern).collect();
    if items
        .iter()
        .any(|i| matches!(i, chrono::format::Item::Error))
    {
        return time.format(DEFAULT_PATTERN).to_string();
    }
    time.format_with_items(items.into_iter()).to_string()
}

/// The pattern for an invalid `pattern` argument.
const DEFAULT_PATTERN: &str = "%Y-%m-%d %H:%M";

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use std::time::SystemTime;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .into()
    }

    #[test]
    fn size_table() {
        let cases: &[(u64, &str)] = &[
            (0, "0 bytes"),
            (1, "1 byte"),
            (2, "2 bytes"),
            (999, "999 bytes"),
            (1000, "1 KB"),
            (1500, "1.5 KB"),
            (12_400_000, "12.4 MB"),
            (532_000, "532 KB"),
            (1_000_000_000, "1 GB"),
            (3_100_000_000, "3.1 GB"),
            (999_999, "1 MB"),
            (1_200_000_000_000, "1.2 TB"),
            (u64::MAX, "18447 PB"),
        ];
        for (n, text) in cases {
            assert_eq!(human_size(*n), *text, "{n}");
        }
    }

    #[test]
    fn explicit_pattern() {
        let t = at(2026, 3, 4, 14, 5);
        assert_eq!(format_time(t, "%Y-%m-%d %H:%M", t), "2026-03-04 14:05");
        assert_eq!(format_time(t, "%d/%m", t), "04/03");
    }

    #[test]
    fn relative_dates() {
        let now = at(2026, 3, 4, 20, 0);
        assert_eq!(
            format_time(at(2026, 3, 4, 14, 32), "relative", now),
            "Today 14:32"
        );
        assert_eq!(
            format_time(at(2026, 3, 3, 9, 10), "relative", now),
            "Yesterday 09:10"
        );
        assert_eq!(
            format_time(at(2026, 3, 2, 23, 59), "relative", now),
            "2026-03-02"
        );
        assert_eq!(
            format_time(at(2025, 12, 31, 1, 0), "relative", now),
            "2025-12-31"
        );
    }

    #[test]
    fn finder_dates() {
        let now = at(2026, 3, 4, 20, 0);
        assert_eq!(
            format_time(at(2026, 3, 4, 14, 32), "finder", now),
            "Today at 14:32"
        );
        assert_eq!(
            format_time(at(2026, 3, 3, 9, 10), "finder", now),
            "Yesterday at 09:10"
        );
        assert_eq!(
            format_time(at(2025, 9, 28, 13, 2), "finder", now),
            "28 Sep 2025 at 13:02"
        );
    }

    #[test]
    fn relative_uses_calendar_dates() {
        let now = at(2026, 3, 4, 0, 5);
        assert_eq!(
            format_time(at(2026, 3, 3, 23, 55), "relative", now),
            "Yesterday 23:55"
        );
    }

    #[test]
    fn invalid_pattern_falls_back() {
        let t = at(2026, 3, 4, 14, 5);
        assert_eq!(format_time(t, "%Q%", t), "2026-03-04 14:05");
        assert_eq!(format_time(t, "%", t), "2026-03-04 14:05");
    }
}
