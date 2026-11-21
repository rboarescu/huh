//! Pure-`std` UTC civil calendar dates (`YYYY-MM-DD`) from Unix epoch timestamps.
//! Uses Howard Hinnant's proleptic Gregorian algorithm; zero external crates.

use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: u64 = 86_400;

/// A UTC calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// Converts days since 1970-01-01 to a UTC `Date`.
pub fn from_days(days_since_epoch: i64) -> Date {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    Date { year, month: m as u8, day: d as u8 }
}

/// Converts a UTC `Date` to days since 1970-01-01.
pub fn to_days(date: Date) -> i64 {
    let y = date.year as i64 - if date.month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = date.month as u64;
    let d = date.day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

/// Converts Unix epoch seconds to a UTC `Date`.
pub fn from_epoch(secs: u64) -> Date {
    from_days((secs / SECS_PER_DAY) as i64)
}

/// Current Unix timestamp in seconds (0 if system clock predates Unix epoch).
pub fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Today's UTC date.
#[cfg_attr(not(test), allow(dead_code))]
pub fn today() -> Date {
    from_epoch(now_epoch())
}

/// Signed day count from `earlier` to `later` (`later - earlier`).
pub fn days_between(earlier: Date, later: Date) -> i64 {
    to_days(later) - to_days(earlier)
}

/// Adds `delta` days (positive or negative) to `date`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn add_days(date: Date, delta: i64) -> Date {
    from_days(to_days(date) + delta)
}

/// Formats `date` as `YYYY-MM-DD`.
pub fn format(date: Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year, date.month, date.day)
}

/// Parses a strict `YYYY-MM-DD` calendar date.
pub fn parse(s: &str) -> Option<Date> {
    let mut parts = s.trim().split('-');
    let y_str = parts.next()?;
    let m_str = parts.next()?;
    let d_str = parts.next()?;
    if parts.next().is_some() || y_str.len() != 4 || m_str.len() != 2 || d_str.len() != 2 {
        return None;
    }
    if !y_str.bytes().all(|b| b.is_ascii_digit())
        || !m_str.bytes().all(|b| b.is_ascii_digit())
        || !d_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = y_str.parse().ok()?;
    let month: u8 = m_str.parse().ok()?;
    let day: u8 = d_str.parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let candidate = Date { year, month, day };
    (from_days(to_days(candidate)) == candidate).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(from_epoch(0), Date { year: 1970, month: 1, day: 1 });
        assert_eq!(to_days(Date { year: 1970, month: 1, day: 1 }), 0);
        let d = parse("2026-11-19").unwrap();
        assert_eq!(d, Date { year: 2026, month: 11, day: 19 });
        assert_eq!(from_days(to_days(d)), d);
        assert_eq!(format(d), "2026-11-19");
        assert!(today().year >= 1970);
    }

    #[test]
    fn leap_years_and_validation() {
        assert!(parse("2000-02-29").is_some());
        assert!(parse("2024-02-29").is_some());
        assert!(parse("1900-02-29").is_none());
        assert!(parse("2100-02-29").is_none());
        assert!(parse("2026-02-29").is_none());
        assert!(parse("2026-02-30").is_none());
        assert!(parse("2026-04-31").is_none());
        assert!(parse("2026-13-01").is_none());
        assert!(parse("2026-00-10").is_none());
        assert!(parse("2026-11-1").is_none());
        assert!(parse("26-11-19").is_none());
        assert!(parse("2026/11/19").is_none());
        assert!(parse("2026-11-19-01").is_none());
    }

    #[test]
    fn day_arithmetic_and_round_trips() {
        let jan1 = parse("2024-01-01").unwrap();
        let feb28 = parse("2024-02-28").unwrap();
        let mar1 = parse("2024-03-01").unwrap();
        assert_eq!(days_between(feb28, mar1), 2);
        assert_eq!(add_days(feb28, 1), parse("2024-02-29").unwrap());
        assert_eq!(add_days(feb28, 2), mar1);
        assert_eq!(days_between(mar1, feb28), -2);
        assert_eq!(days_between(jan1, add_days(jan1, 366)), 366);
        assert_eq!(format(add_days(parse("2025-12-31").unwrap(), 1)), "2026-01-01");
        assert_eq!(format(add_days(parse("2026-01-01").unwrap(), -1)), "2025-12-31");
    }
}
