//! Calendar boundaries used by the stores.
//!
//! Retention follows the calendar the way Starlink's own usage page does: the
//! current year keeps per-minute detail, older minutes fold into one row per
//! month. Production uses the system timezone; tests and cross-language fixtures
//! use UTC so a run in another timezone cannot change the expected files.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, LocalResult, TimeZone, Utc};

pub trait Calendar: Send + Sync {
    /// Local start of the current calendar year, epoch seconds.
    fn year_start_sec(&self, now_ms: i64) -> i64;
    /// Local midnight of the day `now_ms` falls in, epoch seconds.
    fn day_start_sec(&self, now_ms: i64) -> i64;
    /// Local midnight `days_back` calendar days before `now_ms`.
    fn day_start_sec_days_ago(&self, now_ms: i64, days_back: u32) -> i64;
    /// Local Monday 00:00 of the week `now_ms` falls in, epoch seconds.
    fn week_start_sec(&self, now_ms: i64) -> i64;
    /// Local Monday 00:00 `weeks_back` calendar weeks before `now_ms`.
    fn week_start_sec_weeks_ago(&self, now_ms: i64, weeks_back: u32) -> i64;
    /// Local start of the calendar day a minute falls in, epoch seconds.
    fn day_key_sec(&self, minute_sec: i64) -> i64;
    /// Local Monday 00:00 of the week a minute falls in, epoch seconds.
    fn week_key_sec(&self, minute_sec: i64) -> i64;
    /// Local start of the calendar month a minute falls in, epoch seconds.
    fn month_key_sec(&self, minute_sec: i64) -> i64;
    /// Local midnight after the day beginning at `key_sec`.
    fn next_day_sec(&self, key_sec: i64) -> i64;
    /// Local Monday after the week beginning at `key_sec`.
    fn next_week_sec(&self, key_sec: i64) -> i64;
    /// Local first of the month after the month beginning at `key_sec`.
    fn next_month_sec(&self, key_sec: i64) -> i64;
}

fn resolve<Tz: TimeZone>(
    result: LocalResult<chrono::DateTime<Tz>>,
) -> Option<chrono::DateTime<Tz>> {
    match result {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        LocalResult::None => None,
    }
}

fn year_start<Tz: TimeZone>(timezone: &Tz, now_ms: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(now) = resolve(timezone.timestamp_millis_opt(now_ms)) else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn month_key<Tz: TimeZone>(timezone: &Tz, minute_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(minute_sec, 0)) else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(date.year(), date.month(), 1, 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn day_start<Tz: TimeZone>(timezone: &Tz, now_ms: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(now) = resolve(timezone.timestamp_millis_opt(now_ms)) else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn week_start<Tz: TimeZone>(timezone: &Tz, now_ms: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    week_start_weeks_ago(timezone, now_ms, 0)
}

fn day_start_days_ago<Tz: TimeZone>(timezone: &Tz, now_ms: i64, days_back: u32) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(now) = resolve(timezone.timestamp_millis_opt(now_ms)) else {
        return 0;
    };
    let Some(day) = now
        .date_naive()
        .checked_sub_days(chrono::Days::new(u64::from(days_back)))
    else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(day.year(), day.month(), day.day(), 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn week_start_weeks_ago<Tz: TimeZone>(timezone: &Tz, now_ms: i64, weeks_back: u32) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(now) = resolve(timezone.timestamp_millis_opt(now_ms)) else {
        return 0;
    };
    let Some(monday) = now.date_naive().checked_sub_days(chrono::Days::new(
        u64::from(now.weekday().num_days_from_monday()) + u64::from(weeks_back) * 7,
    )) else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(monday.year(), monday.month(), monday.day(), 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn day_key<Tz: TimeZone>(timezone: &Tz, minute_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(minute_sec, 0)) else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn week_key<Tz: TimeZone>(timezone: &Tz, minute_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(minute_sec, 0)) else {
        return 0;
    };
    let Some(monday) = date
        .date_naive()
        .checked_sub_days(chrono::Days::new(u64::from(
            date.weekday().num_days_from_monday(),
        )))
    else {
        return 0;
    };
    resolve(timezone.with_ymd_and_hms(monday.year(), monday.month(), monday.day(), 0, 0, 0))
        .map_or(0, |start| start.timestamp())
}

fn next_day<Tz: TimeZone>(timezone: &Tz, key_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(key_sec, 0)) else {
        return key_sec;
    };
    let Some(next) = date.date_naive().checked_add_days(chrono::Days::new(1)) else {
        return key_sec;
    };
    resolve(timezone.with_ymd_and_hms(next.year(), next.month(), next.day(), 0, 0, 0))
        .map_or(key_sec, |start| start.timestamp())
}

fn next_week<Tz: TimeZone>(timezone: &Tz, key_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(key_sec, 0)) else {
        return key_sec;
    };
    let Some(next) = date.date_naive().checked_add_days(chrono::Days::new(7)) else {
        return key_sec;
    };
    resolve(timezone.with_ymd_and_hms(next.year(), next.month(), next.day(), 0, 0, 0))
        .map_or(key_sec, |start| start.timestamp())
}

fn next_month<Tz: TimeZone>(timezone: &Tz, key_sec: i64) -> i64
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = resolve(timezone.timestamp_opt(key_sec, 0)) else {
        return key_sec;
    };
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    resolve(timezone.with_ymd_and_hms(year, month, 1, 0, 0, 0))
        .map_or(key_sec, |start| start.timestamp())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalCalendar;

impl Calendar for LocalCalendar {
    fn year_start_sec(&self, now_ms: i64) -> i64 {
        year_start(&Local, now_ms)
    }

    fn day_start_sec(&self, now_ms: i64) -> i64 {
        day_start(&Local, now_ms)
    }

    fn day_start_sec_days_ago(&self, now_ms: i64, days_back: u32) -> i64 {
        day_start_days_ago(&Local, now_ms, days_back)
    }

    fn week_start_sec(&self, now_ms: i64) -> i64 {
        week_start(&Local, now_ms)
    }

    fn week_start_sec_weeks_ago(&self, now_ms: i64, weeks_back: u32) -> i64 {
        week_start_weeks_ago(&Local, now_ms, weeks_back)
    }

    fn day_key_sec(&self, minute_sec: i64) -> i64 {
        day_key(&Local, minute_sec)
    }

    fn week_key_sec(&self, minute_sec: i64) -> i64 {
        week_key(&Local, minute_sec)
    }

    fn month_key_sec(&self, minute_sec: i64) -> i64 {
        month_key(&Local, minute_sec)
    }

    fn next_day_sec(&self, key_sec: i64) -> i64 {
        next_day(&Local, key_sec)
    }

    fn next_week_sec(&self, key_sec: i64) -> i64 {
        next_week(&Local, key_sec)
    }

    fn next_month_sec(&self, key_sec: i64) -> i64 {
        next_month(&Local, key_sec)
    }
}

/// Fixed calendar for tests and for fixtures shared with the TypeScript code,
/// whose generator pins `TZ=UTC`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UtcCalendar;

impl Calendar for UtcCalendar {
    fn year_start_sec(&self, now_ms: i64) -> i64 {
        year_start(&Utc, now_ms)
    }

    fn day_start_sec(&self, now_ms: i64) -> i64 {
        day_start(&Utc, now_ms)
    }

    fn day_start_sec_days_ago(&self, now_ms: i64, days_back: u32) -> i64 {
        day_start_days_ago(&Utc, now_ms, days_back)
    }

    fn week_start_sec(&self, now_ms: i64) -> i64 {
        week_start(&Utc, now_ms)
    }

    fn week_start_sec_weeks_ago(&self, now_ms: i64, weeks_back: u32) -> i64 {
        week_start_weeks_ago(&Utc, now_ms, weeks_back)
    }

    fn day_key_sec(&self, minute_sec: i64) -> i64 {
        day_key(&Utc, minute_sec)
    }

    fn week_key_sec(&self, minute_sec: i64) -> i64 {
        week_key(&Utc, minute_sec)
    }

    fn month_key_sec(&self, minute_sec: i64) -> i64 {
        month_key(&Utc, minute_sec)
    }

    fn next_day_sec(&self, key_sec: i64) -> i64 {
        next_day(&Utc, key_sec)
    }

    fn next_week_sec(&self, key_sec: i64) -> i64 {
        next_week(&Utc, key_sec)
    }

    fn next_month_sec(&self, key_sec: i64) -> i64 {
        next_month(&Utc, key_sec)
    }
}

/// Wall clock in Unix milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-06-15T12:00:00Z
    const NOW_MS: i64 = 1_781_524_800_000;

    #[test]
    fn utc_boundaries_are_stable() {
        let calendar = UtcCalendar;
        assert_eq!(calendar.year_start_sec(NOW_MS), 1_767_225_600); // 2026-01-01
        assert_eq!(
            calendar.month_key_sec(1_781_524_800), // 2026-06-15
            1_780_272_000                          // 2026-06-01
        );
        assert_eq!(calendar.day_start_sec(NOW_MS), 1_781_481_600); // 2026-06-15 00:00
        assert_eq!(calendar.week_start_sec(NOW_MS), 1_781_481_600); // Monday
        assert_eq!(
            calendar.day_start_sec_days_ago(NOW_MS, 13),
            1_781_481_600 - 13 * 86_400
        );
        assert_eq!(
            calendar.week_start_sec_weeks_ago(NOW_MS, 11),
            1_781_481_600 - 11 * 7 * 86_400
        );
        assert_eq!(calendar.next_day_sec(1_781_481_600), 1_781_568_000);
        assert_eq!(calendar.next_week_sec(1_781_481_600), 1_782_086_400);
        assert_eq!(calendar.next_month_sec(1_780_272_000), 1_782_864_000); // July
    }
}
