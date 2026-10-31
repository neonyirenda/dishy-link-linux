//! When a rule lets a device online, as a weekly timetable.
//!
//! Port of upstream `core/schedule.ts`. A schedule holds several windows, because "4pm
//! to 8pm on weekdays, 9am to 9pm at the weekend" is one arrangement over one
//! set of devices. Everything is local time, and every boundary is built from
//! local date parts rather than by adding milliseconds, which keeps a window
//! set for 4pm at 4pm across a daylight-saving shift.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};

const MINUTES_PER_DAY: u32 = 1_440;
const DAY_MS: i64 = 86_400_000;
/// Long enough that a weekly window is always found, and a one-off date
/// already past never is.
const HORIZON_DAYS: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleWindow {
    /// Local weekdays, 0–6 from Sunday.
    pub weekdays: Vec<u8>,
    /// Minutes from local midnight.
    pub start_minute: u32,
    /// Minutes from local midnight. At or before `start_minute` runs past
    /// midnight into the next day.
    pub end_minute: u32,
    /// Local dates (YYYY-MM-DD) the window is confined to, inclusive. Empty
    /// `weekdays` runs every day between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_date: Option<String>,
}

/// `Allow` opens the device inside its windows and holds it shut for the rest
/// of the days the timetable covers. `Block` is the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleMode {
    Allow,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub mode: ScheduleMode,
    pub windows: Vec<ScheduleWindow>,
}

/// The stretch a schedule is in: whether it holds the device shut, and when
/// that stops being true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleSegment {
    pub blocked: bool,
    pub end_ms: i64,
}

/// Whether the schedule holds the device shut at this instant. A day the
/// timetable does not cover is unrestricted, not shut. Containment is
/// answered before the day, so a window running past midnight still finishes
/// on a day nothing else covers.
pub fn blocked_at(schedule: &Schedule, at_ms: i64) -> bool {
    let inside = schedule
        .windows
        .iter()
        .any(|window| window_contains(window, at_ms));
    match schedule.mode {
        ScheduleMode::Block => inside,
        ScheduleMode::Allow => !inside && day_is_scheduled(schedule, at_ms),
    }
}

/// Whether the timetable has any bearing at this instant. False on a day it
/// never names, where the rule is sitting the day out.
pub fn schedule_governs(schedule: &Schedule, at_ms: i64) -> bool {
    schedule
        .windows
        .iter()
        .any(|window| window_contains(window, at_ms))
        || day_is_scheduled(schedule, at_ms)
}

/// The current stretch and when it ends: the next instant `blocked_at`
/// changes, or a day ahead if nothing ahead changes. Always finite: an
/// infinite end would reach the store as JSON `null` and roll every poll.
pub fn schedule_segment(schedule: &Schedule, at_ms: i64) -> ScheduleSegment {
    let blocked = blocked_at(schedule, at_ms);
    let end_ms = candidate_boundaries(schedule, at_ms)
        .into_iter()
        .find(|boundary| blocked_at(schedule, *boundary) != blocked)
        .unwrap_or(at_ms + DAY_MS);
    ScheduleSegment { blocked, end_ms }
}

/// An empty timetable is inert.
pub fn schedule_active(schedule: Option<&Schedule>) -> bool {
    schedule.is_some_and(|schedule| !schedule.windows.is_empty())
}

/// `allow;12345@960-1200;06@540-1260`, with a run of dates appended to the
/// weekdays as `~2026-01-01..2026-02-14`.
pub fn format_schedule_param(schedule: &Schedule) -> String {
    let mode = match schedule.mode {
        ScheduleMode::Allow => "allow",
        ScheduleMode::Block => "block",
    };
    let mut parts = vec![mode.to_string()];
    for window in &schedule.windows {
        let mut weekdays = window.weekdays.clone();
        weekdays.sort_unstable();
        let weekdays: String = weekdays.iter().map(u8::to_string).collect();
        let run = if window.from_date.is_none() && window.to_date.is_none() {
            String::new()
        } else {
            format!(
                "~{}..{}",
                window.from_date.as_deref().unwrap_or(""),
                window.to_date.as_deref().unwrap_or("")
            )
        };
        parts.push(format!(
            "{weekdays}{run}@{}-{}",
            window.start_minute, window.end_minute
        ));
    }
    parts.join(";")
}

/// `None` when the parameter names no usable window, so a malformed timetable
/// leaves a rule unscheduled rather than shut on hours nobody set.
pub fn parse_schedule_param(value: &str) -> Option<Schedule> {
    let mut entries = value.split(';');
    let mode = match entries.next()? {
        "allow" => ScheduleMode::Allow,
        "block" => ScheduleMode::Block,
        _ => return None,
    };
    let windows: Vec<ScheduleWindow> = entries.filter_map(window_from_param).collect();
    (!windows.is_empty()).then_some(Schedule { mode, windows })
}

fn window_from_param(value: &str) -> Option<ScheduleWindow> {
    let (days, span) = value.split_once('@')?;
    let (start, end) = span.split_once('-')?;
    let start: f64 = start.parse().ok().filter(|value: &f64| value.is_finite())?;
    let end: f64 = end.parse().ok().filter(|value: &f64| value.is_finite())?;
    let (weekday_part, run_part) = days.split_once('~').unwrap_or((days, ""));
    let (from, to) = run_part.split_once("..").unwrap_or((run_part, ""));
    let date = |value: &str| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .map(|_| value.to_string())
    };
    let (from_date, to_date) = (date(from), date(to));
    let mut weekdays: Vec<u8> = Vec::new();
    for character in weekday_part.chars() {
        if let Some(weekday) = character.to_digit(10).filter(|day| *day <= 6)
            && !weekdays.contains(&(weekday as u8))
        {
            weekdays.push(weekday as u8);
        }
    }
    // A window naming neither weekdays nor a run of dates falls on no day.
    if weekdays.is_empty() && from_date.is_none() && to_date.is_none() {
        return None;
    }
    let minute = |value: f64| value.round().clamp(0.0, f64::from(MINUTES_PER_DAY)) as u32;
    Some(ScheduleWindow {
        weekdays,
        start_minute: minute(start),
        end_minute: minute(end),
        from_date,
        to_date,
    })
}

/// A local wall-clock time as epoch milliseconds. A time skipped by a
/// daylight-saving jump moves forward an hour, as the JavaScript `Date`
/// constructor does.
fn resolve_local(naive: NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            Local
                .from_local_datetime(&(naive + Duration::hours(1)))
                .earliest()
        })
        .map_or_else(
            || naive.and_utc().timestamp_millis(),
            |at| at.timestamp_millis(),
        )
}

pub(crate) fn local_date(at_ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(at_ms).single().map_or_else(
        || {
            chrono::DateTime::from_timestamp_millis(at_ms)
                .unwrap_or_default()
                .date_naive()
        },
        |at| at.date_naive(),
    )
}

/// Local midnight `day_offset` days from the day `at_ms` falls on.
fn local_day(at_ms: i64, day_offset: i64) -> NaiveDate {
    local_date(at_ms) + Duration::days(day_offset)
}

/// Built from the day's own date parts, so on the two days a year that are not
/// 24 hours long, minute 960 is still 4pm.
pub(crate) fn minute_of_day(day: NaiveDate, minute: u32) -> i64 {
    resolve_local(day.and_time(chrono::NaiveTime::MIN) + Duration::minutes(i64::from(minute)))
}

/// Counted from the midnight the window opened on, so 22:00 to 02:00 is four
/// hours and 9am to 9am is the whole day.
fn span_end_minute(window: &ScheduleWindow) -> u32 {
    if window.end_minute > window.start_minute {
        window.end_minute
    } else {
        window.end_minute + MINUTES_PER_DAY
    }
}

fn window_runs_on(window: &ScheduleWindow, day: NaiveDate) -> bool {
    let dated = window.from_date.is_some() || window.to_date.is_some();
    if dated {
        // ISO dates sort as strings.
        let key = day.format("%Y-%m-%d").to_string();
        if window.from_date.as_ref().is_some_and(|from| key < *from)
            || window.to_date.as_ref().is_some_and(|to| key > *to)
        {
            return false;
        }
    }
    if window.weekdays.is_empty() {
        return dated;
    }
    window
        .weekdays
        .contains(&(day.weekday().num_days_from_sunday() as u8))
}

/// Yesterday too: a window that crosses midnight is still the previous day's.
fn window_contains(window: &ScheduleWindow, at_ms: i64) -> bool {
    [0, -1].into_iter().any(|offset| {
        let day = local_day(at_ms, offset);
        window_runs_on(window, day)
            && (minute_of_day(day, window.start_minute)
                ..minute_of_day(day, span_end_minute(window)))
                .contains(&at_ms)
    })
}

fn day_is_scheduled(schedule: &Schedule, at_ms: i64) -> bool {
    let day = local_day(at_ms, 0);
    schedule
        .windows
        .iter()
        .any(|window| window_runs_on(window, day))
}

/// Both edges of every window in range, plus every local midnight, where a
/// covered day becomes one the timetable does not name.
fn candidate_boundaries(schedule: &Schedule, at_ms: i64) -> Vec<i64> {
    let mut found = Vec::new();
    for offset in -1..=HORIZON_DAYS {
        let day = local_day(at_ms, offset);
        found.push(minute_of_day(day, 0));
        for window in &schedule.windows {
            if window_runs_on(window, day) {
                found.push(minute_of_day(day, window.start_minute));
                found.push(minute_of_day(day, span_end_minute(window)));
            }
        }
    }
    found.retain(|boundary| *boundary > at_ms);
    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::{Datelike, Local, TimeZone, Timelike};

    use super::*;

    const WEEKDAYS: [u8; 5] = [1, 2, 3, 4, 5];
    const WEEKEND: [u8; 2] = [0, 6];

    /// Local wall-clock time in August 2026 (the 17th is a Monday).
    pub(crate) fn at(day: u32, hour: u32, minute: u32) -> i64 {
        let (month, day) = if day > 31 { (9, day - 31) } else { (8, day) };
        Local
            .with_ymd_and_hms(2026, month, day, hour, minute, 0)
            .earliest()
            .expect("local time")
            .timestamp_millis()
    }

    fn local(ms: i64) -> chrono::DateTime<Local> {
        Local.timestamp_millis_opt(ms).single().expect("local")
    }

    fn window(weekdays: &[u8], start: u32, end: u32) -> ScheduleWindow {
        ScheduleWindow {
            weekdays: weekdays.to_vec(),
            start_minute: start,
            end_minute: end,
            from_date: None,
            to_date: None,
        }
    }

    fn dated(weekdays: &[u8], start: u32, end: u32, from: &str, to: &str) -> ScheduleWindow {
        ScheduleWindow {
            from_date: Some(from.into()),
            to_date: Some(to.into()),
            ..window(weekdays, start, end)
        }
    }

    fn kids() -> Schedule {
        Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![
                window(&WEEKDAYS, 16 * 60, 20 * 60),
                window(&WEEKEND, 9 * 60, 21 * 60),
            ],
        }
    }

    fn weekdays_only() -> Schedule {
        Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![window(&WEEKDAYS, 16 * 60, 20 * 60)],
        }
    }

    // SPEC-RULE-004: allow windows open the device and shut it for the rest of
    // the days they cover; days never named are unrestricted.
    #[test]
    fn allow_windows_open_inside_and_shut_outside_on_covered_days() {
        let kids = kids();
        assert!(!blocked_at(&kids, at(17, 18, 0)));
        assert!(blocked_at(&kids, at(17, 10, 0)));
        assert!(blocked_at(&kids, at(17, 21, 0)));
        assert!(!blocked_at(&kids, at(22, 10, 0)));
        assert!(blocked_at(&kids, at(22, 22, 0)));
        assert!(!blocked_at(&weekdays_only(), at(22, 3, 0)));
        assert!(!blocked_at(&weekdays_only(), at(22, 12, 0)));
    }

    // SPEC-RULE-004: block windows, windows past midnight, and dated runs.
    #[test]
    fn block_windows_midnight_crossings_and_dated_runs() {
        let bedtime = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![window(&[0, 1, 2, 3, 4, 5, 6], 22 * 60, 7 * 60)],
        };
        assert!(blocked_at(&bedtime, at(17, 23, 0)));
        assert!(blocked_at(&bedtime, at(18, 3, 0)));
        assert!(!blocked_at(&bedtime, at(18, 9, 0)));

        let late_monday = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![window(&[1], 22 * 60, 2 * 60)],
        };
        assert!(blocked_at(&late_monday, at(18, 1, 0)));
        assert!(!blocked_at(&late_monday, at(18, 3, 0)));

        let exam_day = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![dated(&[], 9 * 60, 17 * 60, "2026-08-19", "2026-08-19")],
        };
        assert!(blocked_at(&exam_day, at(19, 12, 0)));
        assert!(!blocked_at(&exam_day, at(26, 12, 0)));

        let holidays = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![dated(&[], 9 * 60, 17 * 60, "2026-08-17", "2026-08-21")],
        };
        assert!(blocked_at(&holidays, at(17, 12, 0)));
        assert!(blocked_at(&holidays, at(21, 12, 0)));
        assert!(!blocked_at(&holidays, at(16, 12, 0)));
        assert!(!blocked_at(&holidays, at(22, 12, 0)));

        let term_time = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![dated(
                &WEEKDAYS,
                16 * 60,
                20 * 60,
                "2026-08-17",
                "2026-08-31",
            )],
        };
        assert!(blocked_at(&term_time, at(17, 10, 0)));
        assert!(!blocked_at(&term_time, at(22, 10, 0)));
        assert!(!blocked_at(&term_time, at(31 + 7, 10, 0)));

        let january = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![dated(&[], 16 * 60, 20 * 60, "2027-01-01", "2027-02-14")],
        };
        assert!(!blocked_at(&january, at(17, 10, 0)));
        assert!(schedule_segment(&january, at(17, 10, 0)).end_ms > at(17, 10, 0));
    }

    // SPEC-RULE-004: "not blocked" splits into sitting the day out and being
    // inside an open window.
    #[test]
    fn governing_distinguishes_days_the_timetable_never_names() {
        assert!(!schedule_governs(&weekdays_only(), at(22, 12, 0)));
        assert!(schedule_governs(&weekdays_only(), at(21, 18, 0)));
        assert!(schedule_governs(&weekdays_only(), at(21, 10, 0)));
        let overnight = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![window(&[5], 21 * 60, 2 * 60)],
        };
        assert!(schedule_governs(&overnight, at(22, 1, 0)));
        assert!(!schedule_governs(&overnight, at(22, 3, 0)));
    }

    // SPEC-RULE-004: the segment ends where the answer next changes, on the
    // wall clock, and is always finite.
    #[test]
    fn segments_end_where_the_answer_changes() {
        let kids = kids();
        let shut = schedule_segment(&kids, at(17, 10, 0));
        assert!(shut.blocked);
        assert_eq!(
            (local(shut.end_ms).day(), local(shut.end_ms).hour()),
            (17, 16)
        );

        let open = schedule_segment(&kids, at(17, 18, 0));
        assert!(!open.blocked);
        assert_eq!(local(open.end_ms).hour(), 20);

        let friday_night = schedule_segment(&kids, at(21, 21, 0));
        assert!(friday_night.blocked);
        assert_eq!(
            (
                local(friday_night.end_ms).day(),
                local(friday_night.end_ms).hour()
            ),
            (22, 9)
        );

        let released = schedule_segment(&weekdays_only(), at(21, 21, 0));
        assert!(released.blocked);
        assert_eq!(
            (local(released.end_ms).day(), local(released.end_ms).hour()),
            (22, 0)
        );

        let spent = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![dated(&[], 9 * 60, 17 * 60, "2026-08-01", "2026-08-01")],
        };
        let segment = schedule_segment(&spent, at(19, 12, 0));
        assert!(!segment.blocked);
        assert_eq!(segment.end_ms, at(19, 12, 0) + 86_400_000);

        let overlapping = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![
                window(&[1], 9 * 60, 18 * 60),
                window(&[1], 12 * 60, 14 * 60),
            ],
        };
        let segment = schedule_segment(&overlapping, at(17, 13, 0));
        assert!(!segment.blocked);
        assert_eq!(local(segment.end_ms).hour(), 18);

        for day in [17, 18, 19, 20, 21] {
            let segment = schedule_segment(&kids, at(day, 6, 0));
            assert_eq!(
                (local(segment.end_ms).hour(), local(segment.end_ms).minute()),
                (16, 0)
            );
        }
    }

    // SPEC-RULE-004: a boundary stays on the wall clock across the late-October
    // daylight-saving change (both hemispheres shift within a week of it).
    #[test]
    fn boundaries_keep_their_wall_clock_across_daylight_saving() {
        let kids = kids();
        for day in 22..=30 {
            let morning = Local
                .with_ymd_and_hms(2026, 10, day, 6, 0, 0)
                .earliest()
                .expect("local")
                .timestamp_millis();
            let segment = schedule_segment(&kids, morning);
            let end = local(segment.end_ms);
            let expected = if matches!(end.weekday().num_days_from_sunday(), 0 | 6) {
                9
            } else {
                16
            };
            assert_eq!(
                (end.day(), end.hour(), end.minute()),
                (day, expected, 0),
                "Oct {day}"
            );
        }
    }

    // SPEC-RULE-004: the wire form round-trips; malformed timetables are
    // refused and unreadable windows dropped.
    #[test]
    fn the_query_parameter_form_round_trips() {
        let kids = kids();
        assert_eq!(
            format_schedule_param(&kids),
            "allow;12345@960-1200;06@540-1260"
        );
        assert_eq!(
            parse_schedule_param(&format_schedule_param(&kids)),
            Some(kids)
        );

        let exam_day = Schedule {
            mode: ScheduleMode::Block,
            windows: vec![dated(&[], 540, 1020, "2026-08-19", "2026-08-19")],
        };
        assert_eq!(
            format_schedule_param(&exam_day),
            "block;~2026-08-19..2026-08-19@540-1020"
        );
        assert_eq!(
            parse_schedule_param(&format_schedule_param(&exam_day)),
            Some(exam_day)
        );

        let term_time = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![dated(&WEEKDAYS, 960, 1200, "2027-01-01", "2027-02-14")],
        };
        assert_eq!(
            format_schedule_param(&term_time),
            "allow;12345~2027-01-01..2027-02-14@960-1200"
        );
        assert_eq!(
            parse_schedule_param(&format_schedule_param(&term_time)),
            Some(term_time)
        );

        for malformed in [
            "",
            "allow",
            "sometimes;12345@960-1200",
            "allow;@960-1200",
            "allow;9@x",
        ] {
            assert_eq!(parse_schedule_param(malformed), None, "{malformed}");
        }
        assert_eq!(
            parse_schedule_param("allow;9@60-120;1@960-1200"),
            Some(Schedule {
                mode: ScheduleMode::Allow,
                windows: vec![window(&[1], 960, 1200)],
            })
        );
    }

    // SPEC-RULE-004: an empty timetable is inert.
    #[test]
    fn an_empty_timetable_is_inert() {
        let empty = Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![],
        };
        assert!(!schedule_active(None));
        assert!(!schedule_active(Some(&empty)));
        assert!(!blocked_at(&empty, at(17, 10, 0)));
        assert!(schedule_active(Some(&kids())));
        // The weekday helper is the same Sunday-zero convention chrono's
        // `num_days_from_sunday` uses.
        assert_eq!(local(at(16, 12, 0)).weekday().num_days_from_sunday(), 0);
    }
}
