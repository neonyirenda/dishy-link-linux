//! Rolling traffic totals over per-minute buckets, grouped into bars.
//!
//! The port of `core/energySummary.ts`: the same seven ranges, the same slot
//! boundaries, and the same rule that a slot with no recorded minutes is absent
//! rather than zero. Energy rides the same buckets, but this app only shows the
//! traffic half for now.

use serde::{Deserialize, Serialize};

use crate::calendar::Calendar;
use crate::energy::{Coverage, MinuteBucket, coverage};

/// One gigabyte is 8×10⁹ bits, matching the TypeScript summary.
pub const BITS_PER_GB: f64 = 8e9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageRange {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "6h")]
    SixHours,
    #[serde(rename = "12h")]
    TwelveHours,
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "day")]
    Day,
    #[serde(rename = "week")]
    Week,
    #[serde(rename = "month")]
    Month,
}

impl UsageRange {
    pub const ALL: [Self; 7] = [
        Self::OneHour,
        Self::SixHours,
        Self::TwelveHours,
        Self::Today,
        Self::Day,
        Self::Week,
        Self::Month,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::TwelveHours => "12h",
            Self::Today => "today",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// The catalogue key for the range's tab label.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::OneHour => "ranges.1h",
            Self::SixHours => "ranges.6h",
            Self::TwelveHours => "ranges.12h",
            Self::Today => "ranges.today",
            Self::Day => "ranges.day",
            Self::Week => "ranges.week",
            Self::Month => "ranges.month",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    /// Epoch seconds at the slot's start.
    pub t: i64,
    #[serde(rename = "kWh")]
    pub kwh: Option<f64>,
    /// Null when nothing was recorded for the slot — absence, not zero use.
    #[serde(rename = "downGB")]
    pub down_gb: Option<f64>,
    #[serde(rename = "upGB")]
    pub up_gb: Option<f64>,
    pub sampled_seconds: i64,
    /// Seconds of this slot the range covers; the newest slot is still filling.
    pub expected_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub range: UsageRange,
    #[serde(rename = "totalKWh")]
    pub total_kwh: f64,
    #[serde(rename = "totalDownGB")]
    pub total_down_gb: f64,
    #[serde(rename = "totalUpGB")]
    pub total_up_gb: f64,
    pub coverage: Coverage,
    pub buckets: Vec<UsageBucket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupUnit {
    Fixed(i64),
    CalendarDay,
    CalendarWeek,
    CalendarMonth,
}

impl GroupUnit {
    fn key(self, minute_sec: i64, calendar: &dyn Calendar) -> i64 {
        match self {
            Self::Fixed(width) => minute_sec.div_euclid(width) * width,
            Self::CalendarDay => calendar.day_key_sec(minute_sec),
            Self::CalendarWeek => calendar.week_key_sec(minute_sec),
            Self::CalendarMonth => calendar.month_key_sec(minute_sec),
        }
    }

    fn next(self, key_sec: i64, calendar: &dyn Calendar) -> i64 {
        match self {
            Self::Fixed(width) => key_sec + width,
            Self::CalendarDay => calendar.next_day_sec(key_sec),
            Self::CalendarWeek => calendar.next_week_sec(key_sec),
            Self::CalendarMonth => calendar.next_month_sec(key_sec),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RangeSpec {
    start_sec: i64,
    group: GroupUnit,
}

fn spec(range: UsageRange, now_ms: i64, calendar: &dyn Calendar) -> RangeSpec {
    let now_sec = now_ms.div_euclid(1_000);
    let (start_sec, group) = match range {
        UsageRange::OneHour => (now_sec - 3_600, GroupUnit::Fixed(300)),
        UsageRange::SixHours => (now_sec - 6 * 3_600, GroupUnit::Fixed(1_800)),
        UsageRange::TwelveHours => (now_sec - 12 * 3_600, GroupUnit::Fixed(3_600)),
        UsageRange::Today => (calendar.day_start_sec(now_ms), GroupUnit::Fixed(3_600)),
        UsageRange::Day => (
            calendar.day_start_sec_days_ago(now_ms, 13),
            GroupUnit::CalendarDay,
        ),
        UsageRange::Week => (
            calendar.week_start_sec_weeks_ago(now_ms, 11),
            GroupUnit::CalendarWeek,
        ),
        UsageRange::Month => (calendar.year_start_sec(now_ms), GroupUnit::CalendarMonth),
    };
    RangeSpec { start_sec, group }
}

/// The range's `[startSec, endSec)` window, so a caller can fetch the minutes
/// to fold.
pub fn usage_range_bounds(range: UsageRange, now_ms: i64, calendar: &dyn Calendar) -> (i64, i64) {
    let spec = spec(range, now_ms, calendar);
    (spec.start_sec, now_ms.div_euclid(1_000))
}

/// Every bar slot in the range, including ones nothing was recorded for.
fn group_keys_in_range(spec: RangeSpec, end_sec: i64, calendar: &dyn Calendar) -> Vec<i64> {
    let mut keys = Vec::new();
    let mut key = match spec.group {
        GroupUnit::Fixed(width) => spec.start_sec.div_euclid(width) * width,
        GroupUnit::CalendarDay => calendar.day_key_sec(spec.start_sec),
        GroupUnit::CalendarWeek => calendar.week_key_sec(spec.start_sec),
        GroupUnit::CalendarMonth => calendar.month_key_sec(spec.start_sec),
    };
    while key <= end_sec {
        keys.push(key);
        let next = spec.group.next(key, calendar);
        if next <= key {
            break;
        }
        key = next;
    }
    keys
}

/// A range's bar slots: each slot's start and the seconds of it the range
/// covers, plus the key a minute folds into. Shared with the latency summary
/// so every range-based view bars time the same way.
pub(crate) struct RangeSlots<'a> {
    spec: RangeSpec,
    calendar: &'a dyn Calendar,
    pub start_sec: i64,
    pub end_sec: i64,
}

impl<'a> RangeSlots<'a> {
    pub(crate) fn new(range: UsageRange, now_ms: i64, calendar: &'a dyn Calendar) -> Self {
        let spec = spec(range, now_ms, calendar);
        Self {
            spec,
            calendar,
            start_sec: spec.start_sec,
            end_sec: now_ms.div_euclid(1_000),
        }
    }

    pub(crate) fn key(&self, minute_sec: i64) -> i64 {
        self.spec.group.key(minute_sec, self.calendar)
    }

    pub(crate) fn contains(&self, minute_sec: i64) -> bool {
        (self.start_sec..self.end_sec).contains(&minute_sec)
    }

    /// Every slot, `(start, expected seconds)`, including empty ones.
    pub(crate) fn slots(&self) -> Vec<(i64, i64)> {
        group_keys_in_range(self.spec, self.end_sec, self.calendar)
            .into_iter()
            .map(|t| {
                let next = self.spec.group.next(t, self.calendar);
                (t, (next.min(self.end_sec) - t.max(self.start_sec)).max(0))
            })
            .collect()
    }
}

/// Fold the range's minute buckets into per-bar and whole-range totals.
///
/// Buckets outside the request are ignored. The caller normally fetches only
/// the range, and the recorder adds its in-progress minute, so this is a guard
/// rather than a filter that changes a correct fetch.
pub fn summarize_usage(
    buckets: &[MinuteBucket],
    range: UsageRange,
    now_ms: i64,
    calendar: &dyn Calendar,
) -> UsageSummary {
    let spec = spec(range, now_ms, calendar);
    let end_sec = now_ms.div_euclid(1_000);
    let mut groups: std::collections::BTreeMap<i64, GroupTotals> =
        std::collections::BTreeMap::new();
    let mut sampled_seconds = 0_i64;
    let mut total_watt_seconds = 0.0;
    let mut total_downlink_bits = 0.0;
    let mut total_uplink_bits = 0.0;

    for bucket in buckets {
        if bucket.minute < spec.start_sec || bucket.minute >= end_sec {
            continue;
        }
        let key = spec.group.key(bucket.minute, calendar);
        let group = groups.entry(key).or_default();
        group.watt_seconds += finite(bucket.watt_seconds);
        group.samples += i64::from(bucket.samples);
        group.downlink_bits += finite(bucket.downlink_bits);
        group.uplink_bits += finite(bucket.uplink_bits);
        sampled_seconds += i64::from(bucket.samples);
        total_watt_seconds += finite(bucket.watt_seconds);
        total_downlink_bits += finite(bucket.downlink_bits);
        total_uplink_bits += finite(bucket.uplink_bits);
    }

    let buckets = group_keys_in_range(spec, end_sec, calendar)
        .into_iter()
        .map(|t| {
            let expected_seconds =
                (spec.group.next(t, calendar).min(end_sec) - t.max(spec.start_sec)).max(0);
            match groups.get(&t) {
                None => UsageBucket {
                    t,
                    kwh: None,
                    down_gb: None,
                    up_gb: None,
                    sampled_seconds: 0,
                    expected_seconds,
                },
                Some(group) => UsageBucket {
                    t,
                    kwh: Some(group.watt_seconds / 3_600_000.0),
                    down_gb: Some(group.downlink_bits / BITS_PER_GB),
                    up_gb: Some(group.uplink_bits / BITS_PER_GB),
                    sampled_seconds: group.samples,
                    expected_seconds,
                },
            }
        })
        .collect();

    UsageSummary {
        range,
        total_kwh: total_watt_seconds / 3_600_000.0,
        total_down_gb: total_downlink_bits / BITS_PER_GB,
        total_up_gb: total_uplink_bits / BITS_PER_GB,
        coverage: coverage(sampled_seconds, end_sec - spec.start_sec),
        buckets,
    }
}

#[derive(Debug, Default)]
struct GroupTotals {
    watt_seconds: f64,
    samples: i64,
    downlink_bits: f64,
    uplink_bits: f64,
}

fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::UtcCalendar;

    // 2026-06-15T12:00:00Z, a Monday.
    const NOW_MS: i64 = 1_781_524_800_000;
    const NOW_SEC: i64 = 1_781_524_800;

    fn minute(minute: i64, down_bits: f64, up_bits: f64) -> MinuteBucket {
        MinuteBucket {
            minute,
            samples: 60,
            downlink_bits: down_bits,
            uplink_bits: up_bits,
            ..MinuteBucket::default()
        }
    }

    // SPEC-USAGE-002: the range windows match core/energySummary.ts.
    #[test]
    fn ranges_use_the_shared_windows_and_bar_widths() {
        let calendar = UtcCalendar;
        assert_eq!(
            usage_range_bounds(UsageRange::OneHour, NOW_MS, &calendar),
            (NOW_SEC - 3_600, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::SixHours, NOW_MS, &calendar),
            (NOW_SEC - 6 * 3_600, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::TwelveHours, NOW_MS, &calendar),
            (NOW_SEC - 12 * 3_600, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::Today, NOW_MS, &calendar),
            (1_781_481_600, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::Day, NOW_MS, &calendar),
            (1_781_481_600 - 13 * 86_400, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::Week, NOW_MS, &calendar),
            (1_781_481_600 - 11 * 7 * 86_400, NOW_SEC)
        );
        assert_eq!(
            usage_range_bounds(UsageRange::Month, NOW_MS, &calendar),
            (1_767_225_600, NOW_SEC)
        );

        let summary = summarize_usage(&[], UsageRange::OneHour, NOW_MS, &calendar);
        assert_eq!(summary.buckets.len(), 13); // 5-minute slots
        assert_eq!(summary.buckets[0].t, (NOW_SEC - 3_600) / 300 * 300);
        let today = summarize_usage(&[], UsageRange::Today, NOW_MS, &calendar);
        assert_eq!(today.buckets.len(), 13); // hourly slots, midnight through noon
        let month = summarize_usage(&[], UsageRange::Month, NOW_MS, &calendar);
        assert_eq!(month.buckets.len(), 6); // January through June
    }

    // SPEC-USAGE-001: totals convert bits to GB and coverage is honest.
    #[test]
    fn totals_fold_bits_and_coverage() {
        let calendar = UtcCalendar;
        let buckets = [
            minute(NOW_SEC - 120, 8e9, 1e9),
            minute(NOW_SEC - 60, 4e9, 2e9),
        ];
        let summary = summarize_usage(&buckets, UsageRange::OneHour, NOW_MS, &calendar);
        assert_eq!(summary.total_down_gb, 1.5);
        assert_eq!(summary.total_up_gb, 0.375);
        assert_eq!(summary.coverage.sampled_seconds, 120);
        assert_eq!(summary.coverage.expected_seconds, 3_600);
        assert!((summary.coverage.fraction - (120.0 / 3_600.0)).abs() < 1e-12);
    }

    // SPEC-USAGE-003: a slot with no minutes is null; edges are clipped.
    #[test]
    fn empty_slots_are_null_and_edge_slots_are_clipped() {
        let calendar = UtcCalendar;
        let now_ms = NOW_MS + 17_000;
        let buckets = [minute(NOW_SEC - 60, 8e9, 0.0)];
        let summary = summarize_usage(&buckets, UsageRange::OneHour, now_ms, &calendar);
        let with_data: Vec<_> = summary
            .buckets
            .iter()
            .filter(|bucket| bucket.down_gb.is_some())
            .collect();
        assert_eq!(with_data.len(), 1);
        assert_eq!(with_data[0].down_gb, Some(1.0));
        assert_eq!(with_data[0].sampled_seconds, 60);
        assert_eq!(with_data[0].expected_seconds, 300);
        assert!(
            summary
                .buckets
                .iter()
                .filter(|bucket| bucket.down_gb.is_none())
                .all(|bucket| bucket.sampled_seconds == 0)
        );
        let last = summary.buckets.last().expect("last slot");
        assert_eq!(last.expected_seconds, 17);
    }

    // SPEC-USAGE-002: calendar slots follow local day and Monday boundaries.
    #[test]
    fn day_and_week_slots_start_at_local_boundaries() {
        let calendar = UtcCalendar;
        let day = summarize_usage(&[], UsageRange::Day, NOW_MS, &calendar);
        assert_eq!(day.buckets.len(), 14);
        assert_eq!(
            day.buckets.first().map(|bucket| bucket.t),
            Some(1_780_358_400)
        );
        assert_eq!(
            day.buckets.last().map(|bucket| bucket.t),
            Some(1_781_481_600)
        );

        let week = summarize_usage(&[], UsageRange::Week, NOW_MS, &calendar);
        assert_eq!(week.buckets.len(), 12);
        assert_eq!(
            week.buckets.last().map(|bucket| bucket.t),
            Some(1_781_481_600)
        );
    }

    // SPEC-USAGE-002: the month range is the calendar year, not a rolling year.
    #[test]
    fn month_slots_are_calendar_months() {
        let calendar = UtcCalendar;
        let summary = summarize_usage(&[], UsageRange::Month, NOW_MS, &calendar);
        let keys: Vec<_> = summary.buckets.iter().map(|bucket| bucket.t).collect();
        assert_eq!(
            keys,
            vec![
                1_767_225_600, // Jan
                1_769_904_000, // Feb
                1_772_323_200, // Mar
                1_775_001_600, // Apr
                1_777_593_600, // May
                1_780_272_000, // Jun
            ]
        );
    }
}
