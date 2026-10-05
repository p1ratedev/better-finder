//! Text for the size and date columns.

use std::time::SystemTime;

/// Base-10 units, like Finder: `0 bytes`, `1 byte`, `532 bytes`, `1 KB`,
/// `12.4 MB`, `3.1 GB`, `1.2 TB`. One decimal for values under 100 in a unit
/// above bytes (`12.4 MB`), no decimal from 100 (`532 KB`). Drop a `.0`.
pub fn human_size(bytes: u64) -> String {
    let _ = bytes;
    todo!()
}

/// Formats `time` in the local time zone.
///
/// - `pattern` is a `strftime` pattern (chrono syntax), for example `%Y-%m-%d %H:%M`.
/// - The special pattern `relative` gives `Today 14:32`, `Yesterday 09:10`,
///   and `%Y-%m-%d` for older dates.
///
/// `now` is a parameter so that tests are deterministic.
pub fn format_time(time: SystemTime, pattern: &str, now: SystemTime) -> String {
    let _ = (time, pattern, now);
    todo!()
}
