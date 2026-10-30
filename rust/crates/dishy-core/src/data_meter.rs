//! Per-device data allowances: when a device has spent its share of a cycle.
//!
//! Port of `core/dataMeter.ts`. The decision only: no clock, no storage, no
//! router. A rule measures the odometer's lifetime counter from an anchor of
//! its own, so a rule's usage and the month the usage list shows are two spans
//! of one counter. Nothing here holds state between calls; a rule carries
//! everything that has to survive.
//!
//! In this port rules are watch-only (spec 0169): reaching one is announced,
//! and `rule_holds_device` says whether a rule would hold its device, but
//! nothing here or in the recorder pauses a device on the strength of it.

use std::collections::HashMap;

use chrono::{Datelike, Duration, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::schedule::{Schedule, local_date, minute_of_day, schedule_active, schedule_segment};

const DAY_MS: i64 = 86_400_000;

/// How long a spent-allowance announcement stands. Reaching an allowance is
/// an event, not a condition to sit in.
pub const METER_ALERT_HOLD_MS: i64 = 60_000;

/// The longest countdown a rule can be set to.
pub const MAX_COUNTDOWN_MS: i64 = 24 * 3_600_000;

/// What one device's counter reads now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterReading {
    pub client_key: String,
    pub lifetime_rx: u64,
    pub lifetime_tx: u64,
}

/// When a rule's allowance starts over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeterCycle {
    Daily,
    /// `weekday` is 0–6 from Sunday.
    Weekly {
        weekday: u8,
    },
    /// Rolls on `day`, clamped to short months.
    Monthly {
        day: u8,
    },
    #[serde(rename_all = "camelCase")]
    Custom {
        days: u32,
        start_ms: i64,
    },
    /// The account's own cycle, copied from the signed-in account.
    Billing {
        day: u8,
    },
    /// A fixed allowance that never rolls.
    Once,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterRule {
    pub client_key: String,
    /// The cycle's budget. Zero means the rule measures no allowance.
    pub allocation_bytes: u64,
    /// Kept for the TypeScript historian. This host never enforces it.
    pub auto_pause: bool,
    pub cycle: MeterCycle,
    pub anchor_rx: u64,
    pub anchor_tx: u64,
    /// Counters as last seen, so a cycle can roll for an offline device.
    pub observed_rx: u64,
    pub observed_tx: u64,
    pub period_start_ms: i64,
    /// `None` for a cycle that never rolls (written as JSON `null`).
    pub period_end_ms: Option<i64>,
    /// Whether this cycle's allowance has been reached.
    pub acted_this_cycle: bool,
    /// When the allowance or countdown was reached, while the announcement
    /// stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_ms: Option<i64>,
    pub created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Charges this member the group's summed usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_allowance: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countdown_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countdown_start_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countdown_acted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_acted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterTransitionKind {
    /// A rule has run out of its allowance or countdown.
    Reached,
    /// That announcement has stood its minute and retires.
    Expired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeterTransition {
    pub kind: MeterTransitionKind,
    pub client_key: String,
    pub usage_bytes: u64,
    pub at_ms: i64,
    pub rule: MeterRule,
}

/// The terms a rule is written with.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterRuleTerms {
    pub client_key: String,
    pub allocation_bytes: u64,
    pub auto_pause: bool,
    pub cycle: MeterCycle,
    pub lifetime_rx: u64,
    pub lifetime_tx: u64,
    pub now_ms: i64,
    pub group_id: Option<String>,
    pub shared_allowance: bool,
    pub countdown_ms: Option<i64>,
    pub schedule: Option<Schedule>,
}

impl MeterRuleTerms {
    /// A device's own watch-only rule with no countdown or timetable.
    pub fn new(
        client_key: impl Into<String>,
        allocation_bytes: u64,
        cycle: MeterCycle,
        lifetime_rx: u64,
        lifetime_tx: u64,
        now_ms: i64,
    ) -> Self {
        Self {
            client_key: client_key.into(),
            allocation_bytes,
            auto_pause: false,
            cycle,
            lifetime_rx,
            lifetime_tx,
            now_ms,
            group_id: None,
            shared_allowance: false,
            countdown_ms: None,
            schedule: None,
        }
    }
}

/// Local midnight at the start of `day`.
fn midnight(day: NaiveDate) -> i64 {
    minute_of_day(day, 0)
}

/// Local midnight on `day` of the month `month_offset` from `at_ms`'s, with a
/// day past the month's length landing on its last day, so a rule rolling on
/// the 31st still rolls in June.
fn day_of_month(at_ms: i64, day: u8, month_offset: i32) -> i64 {
    let first = local_date(at_ms).with_day(1).expect("the 1st exists");
    let first = if month_offset >= 0 {
        first + Months::new(month_offset as u32)
    } else {
        first - Months::new(month_offset.unsigned_abs())
    };
    let next_first = first + Months::new(1);
    let last_day = (next_first - Duration::days(1)).day();
    let day = u32::from(day).clamp(1, last_day);
    midnight(first.with_day(day).expect("clamped day exists"))
}

/// The allowance as an announcement names it, so every host words it alike.
pub fn format_allowance(bytes: u64) -> String {
    if bytes >= 1_000_000_000_000 {
        format!("{:.2} TB", bytes as f64 / 1e12)
    } else {
        format!("{:.1} GB", bytes as f64 / 1e9)
    }
}

/// "1h 20m" / "45m" / "2h": a countdown at a glance.
pub fn format_duration(total_ms: i64) -> String {
    let whole = (total_ms.max(0) as f64 / 60_000.0).round() as i64;
    let (hours, minutes) = (whole / 60, whole % 60);
    match (hours, minutes) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// What a `dataLimit:` alert needs to be worded after its rule is gone: the
/// subject, the measure it reached, and whether it speaks for a group. Kept in
/// the alert's stored label, fields joined by the ASCII unit separator, which
/// no device or group name carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLimitLabel {
    pub subject: String,
    /// The allowance ("20.0 GB") or the countdown ("2h").
    pub detail: String,
    pub timing: bool,
    pub as_group: bool,
}

const LABEL_SEPARATOR: char = '\u{1f}';

impl DataLimitLabel {
    pub fn for_rule(rule: &MeterRule, subject: &str, as_group: bool) -> Self {
        let timing = rule.countdown_ms.is_some();
        Self {
            subject: subject.to_owned(),
            detail: match rule.countdown_ms {
                Some(countdown) => format_duration(countdown),
                None => format_allowance(rule.allocation_bytes),
            },
            timing,
            as_group,
        }
    }

    pub fn encode(&self) -> String {
        let flags = format!("{}{}", u8::from(self.timing), u8::from(self.as_group));
        [self.subject.as_str(), &self.detail, &flags].join(&LABEL_SEPARATOR.to_string())
    }

    pub fn parse(label: &str) -> Option<Self> {
        let mut parts = label.split(LABEL_SEPARATOR);
        let (subject, detail, flags) = (parts.next()?, parts.next()?, parts.next()?);
        let mut flags = flags.chars();
        Some(Self {
            subject: subject.to_owned(),
            detail: detail.to_owned(),
            timing: flags.next()? == '1',
            as_group: flags.next()? == '1',
        })
    }
}

/// The cycle `at_ms` falls in. `None` as the end means it never rolls.
pub fn period_bounds(cycle: &MeterCycle, at_ms: i64) -> (i64, Option<i64>) {
    match *cycle {
        MeterCycle::Daily => {
            let today = local_date(at_ms);
            (midnight(today), Some(midnight(today + Duration::days(1))))
        }
        MeterCycle::Weekly { weekday } => {
            let today = local_date(at_ms);
            let back = (i64::from(today.weekday().num_days_from_sunday()) - i64::from(weekday))
                .rem_euclid(7);
            let start = today - Duration::days(back);
            (midnight(start), Some(midnight(start + Duration::days(7))))
        }
        MeterCycle::Monthly { day } | MeterCycle::Billing { day } => {
            let this_month = day_of_month(at_ms, day, 0);
            if at_ms >= this_month {
                (this_month, Some(day_of_month(at_ms, day, 1)))
            } else {
                (day_of_month(at_ms, day, -1), Some(this_month))
            }
        }
        MeterCycle::Custom { days, start_ms } => {
            // Counted from the rule's own start date, so a cycle that began
            // before the rule was written lands on the same boundaries.
            let span = i64::from(days.max(1)) * DAY_MS;
            let start = start_ms + (at_ms - start_ms).div_euclid(span) * span;
            (start, Some(start + span))
        }
        MeterCycle::Once => (0, None),
    }
}

/// Where a just-opened cycle starts. A cycle that never rolls sits on no
/// calendar boundary, so it starts at the moment it was opened.
fn cycle_start_ms(cycle: &MeterCycle, bounds_start: i64, now_ms: i64) -> i64 {
    if *cycle == MeterCycle::Once {
        now_ms
    } else {
        bounds_start
    }
}

/// Bytes a rule has counted this cycle, floored at zero.
pub fn usage_bytes(rule: &MeterRule) -> u64 {
    rule.observed_rx.saturating_sub(rule.anchor_rx)
        + rule.observed_tx.saturating_sub(rule.anchor_tx)
}

/// What tells one rule from another: a device can hold a rule of its own and
/// one per group that names it.
pub fn rule_key(client_key: &str, group_id: Option<&str>) -> String {
    format!("{}:{client_key}", group_id.unwrap_or("device"))
}

/// Whether this rule's announcement belongs to its group: a shared allowance
/// crosses once for the group, and a group countdown ends on one clock.
pub fn announces_as_group(rule: &MeterRule) -> bool {
    rule.group_id.is_some() && (rule.shared_allowance == Some(true) || rule.countdown_ms.is_some())
}

/// What an announcement about this rule is keyed to.
pub fn announcement_subject(rule: &MeterRule) -> String {
    match (&rule.group_id, announces_as_group(rule)) {
        (Some(group), true) => format!("group:{group}"),
        _ => rule.client_key.clone(),
    }
}

/// What each shared allowance has spent, summed across its members.
pub fn shared_usage_by_group(rules: &[MeterRule]) -> HashMap<String, u64> {
    let mut totals = HashMap::new();
    for rule in rules {
        if let (Some(group), Some(true)) = (&rule.group_id, rule.shared_allowance) {
            *totals.entry(group.clone()).or_insert(0) += usage_bytes(rule);
        }
    }
    totals
}

/// Bytes charged against a rule's allowance: its own usage, or its group's
/// sum for a member sharing one allowance.
pub fn charged_bytes(rule: &MeterRule, shared_usage: &HashMap<String, u64>) -> u64 {
    match (&rule.group_id, rule.shared_allowance) {
        (Some(group), Some(true)) => shared_usage
            .get(group)
            .copied()
            .unwrap_or_else(|| usage_bytes(rule)),
        _ => usage_bytes(rule),
    }
}

/// No allowance is not an allowance of nothing.
pub fn allocation_spent(rule: &MeterRule, shared_usage: &HashMap<String, u64>) -> bool {
    rule.allocation_bytes > 0 && charged_bytes(rule, shared_usage) >= rule.allocation_bytes
}

pub fn countdown_start_ms(rule: &MeterRule) -> i64 {
    rule.countdown_start_ms.unwrap_or(rule.period_start_ms)
}

pub fn countdown_left_ms(rule: &MeterRule, now_ms: i64) -> Option<i64> {
    rule.countdown_ms
        .map(|countdown| (countdown_start_ms(rule) + countdown - now_ms).max(0))
}

pub fn countdown_spent(rule: &MeterRule, now_ms: i64) -> bool {
    countdown_left_ms(rule, now_ms).is_some_and(|left| left <= 0)
}

pub fn window_shut(rule: &MeterRule) -> bool {
    rule.window_blocked == Some(true)
}

/// Whether this rule would hold its device: off the latches, so a cycle that
/// rolled does not free a device whose countdown is still up.
pub fn rule_holds_device(rule: &MeterRule) -> bool {
    rule.acted_this_cycle || rule.countdown_acted == Some(true) || window_shut(rule)
}

/// A countdown as stored: whole milliseconds, at most a day.
fn clamp_countdown(countdown_ms: Option<i64>) -> Option<i64> {
    countdown_ms.map(|countdown| countdown.clamp(1, MAX_COUNTDOWN_MS))
}

/// The timetable's opening segment, seeded on the write rather than left to
/// the first turnover.
fn open_window(rule: &mut MeterRule, schedule: Option<Schedule>, now_ms: i64) {
    rule.schedule = None;
    rule.window_end_ms = None;
    rule.window_blocked = None;
    rule.window_acted = None;
    if let Some(schedule) = schedule.filter(|schedule| schedule_active(Some(schedule))) {
        let segment = schedule_segment(&schedule, now_ms);
        rule.schedule = Some(schedule);
        rule.window_end_ms = Some(segment.end_ms);
        rule.window_blocked = Some(segment.blocked);
        rule.window_acted = Some(false);
    }
}

pub fn create_rule(terms: MeterRuleTerms) -> MeterRule {
    let (start, end) = period_bounds(&terms.cycle, terms.now_ms);
    let countdown_ms = clamp_countdown(terms.countdown_ms);
    let mut rule = MeterRule {
        client_key: terms.client_key,
        allocation_bytes: terms.allocation_bytes,
        auto_pause: terms.auto_pause,
        period_start_ms: cycle_start_ms(&terms.cycle, start, terms.now_ms),
        period_end_ms: end,
        cycle: terms.cycle,
        anchor_rx: terms.lifetime_rx,
        anchor_tx: terms.lifetime_tx,
        observed_rx: terms.lifetime_rx,
        observed_tx: terms.lifetime_tx,
        acted_this_cycle: false,
        reached_at_ms: None,
        updated_ms: Some(terms.now_ms),
        created_ms: terms.now_ms,
        group_id: terms.group_id,
        shared_allowance: terms.shared_allowance.then_some(true),
        countdown_ms,
        countdown_start_ms: countdown_ms.map(|_| terms.now_ms),
        countdown_acted: countdown_ms.map(|_| false),
        schedule: None,
        window_end_ms: None,
        window_blocked: None,
        window_acted: None,
    };
    open_window(&mut rule, terms.schedule, terms.now_ms);
    rule
}

/// The rule an edit leaves behind: the anchors stand, a changed cycle moves
/// only its boundaries, a countdown that is set runs from now, and a changed
/// timetable re-seeds. `charged_bytes` is what a raised allowance is judged
/// against; `None` uses the rule's own usage.
pub fn upsert_rule(
    existing: Option<&MeterRule>,
    terms: MeterRuleTerms,
    charged_bytes: Option<u64>,
) -> MeterRule {
    let Some(existing) = existing else {
        return create_rule(terms);
    };
    let now_ms = terms.now_ms;
    let mut rule = existing.clone();
    if terms.cycle != existing.cycle {
        let (start, end) = period_bounds(&terms.cycle, now_ms);
        rule.period_start_ms = cycle_start_ms(&terms.cycle, start, now_ms);
        rule.period_end_ms = end;
    }
    rule.allocation_bytes = terms.allocation_bytes;
    rule.auto_pause = terms.auto_pause;
    rule.cycle = terms.cycle;
    rule.updated_ms = Some(now_ms);
    rule.group_id = terms.group_id;
    rule.shared_allowance = terms.shared_allowance.then_some(true);
    let countdown_ms = clamp_countdown(terms.countdown_ms);
    rule.countdown_ms = countdown_ms;
    rule.countdown_start_ms = countdown_ms.map(|_| now_ms);
    rule.countdown_acted = countdown_ms.map(|_| false);
    if terms.schedule != existing.schedule {
        open_window(&mut rule, terms.schedule, now_ms);
    }
    // A stamp carried across a change of subject would retire under a key it
    // was never raised on.
    if announcement_subject(&rule) != announcement_subject(existing) {
        rule.reached_at_ms = None;
    }
    if charged_bytes.unwrap_or_else(|| usage_bytes(&rule)) < rule.allocation_bytes {
        rule.acted_this_cycle = false;
    }
    rule
}

/// Start the allowance and countdown over from now. The timetable is not
/// restarted: its hours are the hours.
pub fn restart_cycle(rule: &MeterRule, now_ms: i64) -> MeterRule {
    let (start, end) = period_bounds(&rule.cycle, now_ms);
    let mut restarted = MeterRule {
        anchor_rx: rule.observed_rx,
        anchor_tx: rule.observed_tx,
        period_start_ms: cycle_start_ms(&rule.cycle, start, now_ms),
        period_end_ms: end,
        acted_this_cycle: false,
        reached_at_ms: None,
        ..rule.clone()
    };
    if rule.countdown_ms.is_some() {
        restarted.countdown_start_ms = Some(now_ms);
        restarted.countdown_acted = Some(false);
    }
    open_window(&mut restarted, rule.schedule.clone(), now_ms);
    restarted
}

/// Fold one poll's readings into every rule and report what crossed.
///
/// A rule with no reading still rolls: its device is away and its counter is
/// frozen. Every rule is folded and rolled before any allowance is tested, so
/// a shared allowance is never judged against another member's pre-roll
/// usage. Nothing here writes to a device.
pub fn evaluate_meters(
    rules: &[MeterRule],
    readings: &[MeterReading],
    now_ms: i64,
) -> (Vec<MeterRule>, Vec<MeterTransition>) {
    let by_key: HashMap<&str, &MeterReading> = readings
        .iter()
        .map(|reading| (reading.client_key.as_str(), reading))
        .collect();

    let rolled: Vec<MeterRule> = rules
        .iter()
        .map(|current| {
            let mut rule = current.clone();
            if let Some(reading) = by_key.get(rule.client_key.as_str()) {
                rule.observed_rx = reading.lifetime_rx;
                rule.observed_tx = reading.lifetime_tx;
            }
            if let Some(schedule) = rule.schedule.as_ref()
                && schedule_active(Some(schedule))
                && now_ms >= rule.window_end_ms.unwrap_or(0)
            {
                let segment = schedule_segment(schedule, now_ms);
                rule.window_end_ms = Some(segment.end_ms);
                rule.window_blocked = Some(segment.blocked);
                rule.window_acted = Some(false);
            }
            if rule.period_end_ms.is_some_and(|end| now_ms >= end) {
                let (start, end) = period_bounds(&rule.cycle, now_ms);
                rule.anchor_rx = rule.observed_rx;
                rule.anchor_tx = rule.observed_tx;
                rule.period_start_ms = cycle_start_ms(&rule.cycle, start, now_ms);
                rule.period_end_ms = end;
                rule.acted_this_cycle = false;
            }
            rule
        })
        .collect();

    let shared_usage = shared_usage_by_group(&rolled);
    let mut transitions = Vec::new();
    let next = rolled
        .into_iter()
        .map(|mut rule| {
            let charged = charged_bytes(&rule, &shared_usage);
            // One announcement per rule, whichever measure runs out first. A
            // shut window never counts as an announcement already made.
            let mut held = rule.acted_this_cycle || rule.countdown_acted == Some(true);
            let mut reach = |rule: &mut MeterRule, transitions: &mut Vec<MeterTransition>| {
                if held {
                    return;
                }
                held = true;
                rule.reached_at_ms = Some(now_ms);
                transitions.push(MeterTransition {
                    kind: MeterTransitionKind::Reached,
                    client_key: rule.client_key.clone(),
                    usage_bytes: charged,
                    at_ms: now_ms,
                    rule: rule.clone(),
                });
            };
            if !rule.acted_this_cycle && allocation_spent(&rule, &shared_usage) {
                rule.acted_this_cycle = true;
                reach(&mut rule, &mut transitions);
            }
            if rule.countdown_acted != Some(true) && countdown_spent(&rule, now_ms) {
                rule.countdown_acted = Some(true);
                reach(&mut rule, &mut transitions);
            }
            if window_shut(&rule) && rule.window_acted != Some(true) {
                rule.window_acted = Some(true);
            }
            if rule
                .reached_at_ms
                .is_some_and(|reached| now_ms - reached >= METER_ALERT_HOLD_MS)
            {
                rule.reached_at_ms = None;
                transitions.push(MeterTransition {
                    kind: MeterTransitionKind::Expired,
                    client_key: rule.client_key.clone(),
                    usage_bytes: charged,
                    at_ms: now_ms,
                    rule: rule.clone(),
                });
            }
            rule
        })
        .collect();
    (next, transitions)
}

/// Transitions with the members of one group reduced to the first that
/// crossed: every member is still held, but it is one thing happening.
pub fn collapse_group_announcements(transitions: Vec<MeterTransition>) -> Vec<MeterTransition> {
    let mut seen = std::collections::HashSet::new();
    transitions
        .into_iter()
        .filter(|transition| {
            !announces_as_group(&transition.rule)
                || seen.insert(format!(
                    "{}:{:?}",
                    announcement_subject(&transition.rule),
                    transition.kind
                ))
        })
        .collect()
}

/// Move rules onto the identities their devices now answer to. Where two
/// rules land on one key, the one whose terms were set most recently wins.
/// Nothing is dropped for being absent.
pub fn resolve_rule_keys(
    rules: &[MeterRule],
    resolve_key: impl Fn(&str) -> String,
) -> Vec<MeterRule> {
    let mut kept: Vec<MeterRule> = Vec::new();
    for rule in rules {
        let client_key = resolve_key(&rule.client_key);
        let moved = MeterRule {
            client_key,
            ..rule.clone()
        };
        let key = rule_key(&moved.client_key, moved.group_id.as_deref());
        match kept
            .iter_mut()
            .find(|held| rule_key(&held.client_key, held.group_id.as_deref()) == key)
        {
            Some(held) if held.updated_ms.unwrap_or(0) >= rule.updated_ms.unwrap_or(0) => {}
            Some(held) => *held = moved,
            None => kept.push(moved),
        }
    }
    kept
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::{Local, TimeZone, Timelike};

    use super::*;
    use crate::schedule::{ScheduleMode, ScheduleWindow};

    pub(crate) const KEY: &str = "2806438232";
    pub(crate) const GB: u64 = 1_000_000_000;
    const HOUR: i64 = 3_600_000;

    pub(crate) fn local_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .earliest()
            .expect("local")
            .timestamp_millis()
    }

    /// A Wednesday, mid-month, mid-afternoon: far from every boundary.
    pub(crate) fn t0() -> i64 {
        local_ms(2026, 8, 12, 15, 30)
    }

    fn local(ms: i64) -> chrono::DateTime<Local> {
        Local.timestamp_millis_opt(ms).single().expect("local")
    }

    fn terms(allocation: u64, cycle: MeterCycle) -> MeterRuleTerms {
        MeterRuleTerms::new(KEY, allocation, cycle, 0, 0, t0())
    }

    fn rule() -> MeterRule {
        create_rule(terms(20 * GB, MeterCycle::Daily))
    }

    fn read(rx: u64, tx: u64) -> Vec<MeterReading> {
        vec![MeterReading {
            client_key: KEY.into(),
            lifetime_rx: rx,
            lifetime_tx: tx,
        }]
    }

    fn kinds(transitions: &[MeterTransition]) -> Vec<MeterTransitionKind> {
        transitions
            .iter()
            .map(|transition| transition.kind)
            .collect()
    }

    // SPEC-RULE-001: each cycle kind's boundaries, on local midnights.
    #[test]
    fn cycles_fall_on_local_calendar_boundaries() {
        let (start, end) = period_bounds(&MeterCycle::Daily, t0());
        assert_eq!((local(start).day(), local(start).hour()), (12, 0));
        assert_eq!(end, Some(local_ms(2026, 8, 13, 0, 0)));

        let (start, end) = period_bounds(&MeterCycle::Weekly { weekday: 1 }, t0());
        assert_eq!(local(start).weekday().num_days_from_sunday(), 1);
        assert_eq!(local(start).day(), 10);
        assert_eq!(end, Some(local_ms(2026, 8, 17, 0, 0)));

        let (start, end) = period_bounds(&MeterCycle::Monthly { day: 7 }, t0());
        assert_eq!((local(start).month(), local(start).day()), (8, 7));
        assert_eq!(end.map(|end| local(end).month()), Some(9));
        let before = local_ms(2026, 8, 3, 9, 0);
        let (start, end) = period_bounds(&MeterCycle::Monthly { day: 7 }, before);
        assert_eq!(local(start).month(), 7);
        assert_eq!(end.map(|end| local(end).day()), Some(7));

        let in_june = local_ms(2026, 6, 15, 0, 0);
        let (start, end) = period_bounds(&MeterCycle::Monthly { day: 31 }, in_june);
        assert_eq!((local(start).month(), local(start).day()), (5, 31));
        assert_eq!(
            end.map(|end| (local(end).month(), local(end).day())),
            Some((6, 30))
        );

        let custom = MeterCycle::Custom {
            days: 10,
            start_ms: local_ms(2026, 8, 1, 0, 0),
        };
        let (start, end) = period_bounds(&custom, t0());
        assert_eq!(local(start).day(), 11);
        assert_eq!(end, Some(start + 10 * DAY_MS));

        let (start, end) = period_bounds(&MeterCycle::Billing { day: 6 }, t0());
        assert_eq!(local(start).day(), 6);
        assert_eq!(end.map(|end| local(end).day()), Some(6));

        assert_eq!(period_bounds(&MeterCycle::Once, t0()).1, None);
    }

    // SPEC-RULE-002: usage runs from the rule's anchor; a cycle rolls for an
    // offline device and re-anchors.
    #[test]
    fn usage_is_measured_from_the_anchor_and_cycles_roll_offline() {
        let existing = create_rule(MeterRuleTerms::new(
            KEY,
            20 * GB,
            MeterCycle::Daily,
            40 * GB,
            0,
            t0(),
        ));
        let (rules, _) = evaluate_meters(&[existing], &read(45 * GB, 0), t0() + 1_000);
        assert_eq!(usage_bytes(&rules[0]), 5 * GB);

        let tripped = MeterRule {
            acted_this_cycle: true,
            ..rule()
        };
        let (rules, _) = evaluate_meters(
            std::slice::from_ref(&tripped),
            &read(25 * GB, 0),
            t0() + DAY_MS,
        );
        assert!(!rule_holds_device(&rules[0]));
        assert_eq!(usage_bytes(&rules[0]), 0);

        let away = MeterRule {
            observed_rx: 25 * GB,
            ..tripped
        };
        let (rules, _) = evaluate_meters(&[away], &[], t0() + DAY_MS);
        assert!(!rule_holds_device(&rules[0]));
        assert_eq!(rules[0].anchor_rx, 25 * GB);
        assert_eq!(usage_bytes(&rules[0]), 0);

        let mut floored = rule();
        floored.anchor_rx = 10;
        assert_eq!(usage_bytes(&floored), 0);
    }

    // SPEC-RULE-003: an allowance is reached once per cycle, counts upload,
    // and is announced whether or not the rule would pause; the announcement
    // retires after its minute.
    #[test]
    fn allowances_are_reached_once_and_announcements_retire() {
        let mut rules = vec![rule()];
        let mut seen = Vec::new();
        for rx in [19 * GB, 21 * GB, 25 * GB, 30 * GB] {
            let (next, transitions) = evaluate_meters(&rules, &read(rx, 0), t0() + 1_000);
            rules = next;
            seen.extend(kinds(&transitions));
        }
        assert_eq!(seen, [MeterTransitionKind::Reached]);
        assert!(rule_holds_device(&rules[0]));

        let (_, transitions) = evaluate_meters(&[rule()], &read(11 * GB, 10 * GB), t0() + 1_000);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Reached]);

        let (rules, _) = evaluate_meters(&[rule()], &read(21 * GB, 0), t0());
        assert_eq!(rules[0].reached_at_ms, Some(t0()));
        let (rules, transitions) =
            evaluate_meters(&rules, &read(21 * GB, 0), t0() + METER_ALERT_HOLD_MS);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Expired]);
        assert_eq!(rules[0].reached_at_ms, None);
        assert!(
            rule_holds_device(&rules[0]),
            "retiring the alert keeps the latch"
        );

        let sold = create_rule(terms(20 * GB, MeterCycle::Once));
        let month_on = t0() + 31 * DAY_MS;
        let (rules, transitions) = evaluate_meters(&[sold], &read(5 * GB, 0), month_on);
        assert!(transitions.is_empty());
        assert_eq!(usage_bytes(&rules[0]), 5 * GB);
        let topped_up = restart_cycle(&rules[0], month_on);
        assert_eq!(usage_bytes(&topped_up), 0);
        assert!(!topped_up.acted_this_cycle);
    }

    fn with_countdown(allocation: u64, cycle: MeterCycle, countdown: i64) -> MeterRule {
        create_rule(MeterRuleTerms {
            countdown_ms: Some(countdown),
            ..terms(allocation, cycle)
        })
    }

    // SPEC-RULE-003: a countdown and an allowance stack on one rule, on their
    // own clocks, and announce once between them.
    #[test]
    fn countdowns_stack_with_allowances() {
        let both = || with_countdown(20 * GB, MeterCycle::Monthly { day: 1 }, 2 * HOUR);
        let written = both();
        assert_eq!(written.cycle, MeterCycle::Monthly { day: 1 });
        assert!(written.period_end_ms.is_some());
        assert_eq!(countdown_left_ms(&written, t0()), Some(2 * HOUR));

        let (rules, transitions) = evaluate_meters(&[both()], &read(GB, 0), t0() + 2 * HOUR);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Reached]);
        assert_eq!(rules[0].countdown_acted, Some(true));
        assert!(!rules[0].acted_this_cycle);
        assert_eq!(usage_bytes(&rules[0]), GB);

        let (rules, transitions) = evaluate_meters(&[both()], &read(20 * GB, 0), t0() + HOUR);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Reached]);
        assert!(rules[0].acted_this_cycle);
        assert_eq!(countdown_left_ms(&rules[0], t0() + HOUR), Some(HOUR));

        let (spent, transitions) = evaluate_meters(&[both()], &read(20 * GB, 0), t0() + 2 * HOUR);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Reached]);
        let next_month = period_bounds(&MeterCycle::Monthly { day: 1 }, t0())
            .1
            .unwrap();
        let (rolled, _) = evaluate_meters(&spent, &read(20 * GB, 0), next_month);
        assert!(!rolled[0].acted_this_cycle);
        assert!(rule_holds_device(&rolled[0]), "the sitting is still over");

        let restarted = restart_cycle(&spent[0], t0() + 3 * HOUR);
        assert!(!rule_holds_device(&restarted));
        assert_eq!(
            countdown_left_ms(&restarted, t0() + 3 * HOUR),
            Some(2 * HOUR)
        );

        let timer = || with_countdown(0, MeterCycle::Once, 2 * HOUR);
        let (rules, transitions) = evaluate_meters(&[timer()], &read(900 * GB, 0), t0() + HOUR);
        assert!(transitions.is_empty());
        assert!(!rule_holds_device(&rules[0]));
        let mut rules = vec![timer()];
        let mut reached = 0;
        for offset in [0, 1_000, 2_000] {
            let (next, transitions) =
                evaluate_meters(&rules, &read(0, 0), t0() + 2 * HOUR + offset);
            rules = next;
            reached += kinds(&transitions)
                .iter()
                .filter(|kind| **kind == MeterTransitionKind::Reached)
                .count();
        }
        assert_eq!(reached, 1);
        assert!(rule_holds_device(&rules[0]));

        assert_eq!(
            with_countdown(0, MeterCycle::Once, 40 * HOUR).countdown_ms,
            Some(MAX_COUNTDOWN_MS)
        );
    }

    // SPEC-RULE-005: re-timing a countdown, even to the same length, runs it
    // from now; a raised allowance re-arms.
    #[test]
    fn edits_restart_countdowns_and_rearm_raised_allowances() {
        let spent = MeterRule {
            countdown_start_ms: Some(t0() - 3 * HOUR),
            countdown_acted: Some(true),
            ..with_countdown(0, MeterCycle::Once, 2 * HOUR)
        };
        assert!(rule_holds_device(&spent));
        for countdown in [2 * HOUR, 3 * HOUR] {
            let edited = upsert_rule(
                Some(&spent),
                MeterRuleTerms {
                    countdown_ms: Some(countdown),
                    ..terms(0, MeterCycle::Once)
                },
                None,
            );
            assert_eq!(countdown_left_ms(&edited, t0()), Some(countdown));
            assert!(!rule_holds_device(&edited));
        }

        let reached = MeterRule {
            acted_this_cycle: true,
            observed_rx: 25 * GB,
            ..rule()
        };
        let raised = upsert_rule(Some(&reached), terms(30 * GB, MeterCycle::Daily), None);
        assert!(!raised.acted_this_cycle);
        assert_eq!(raised.anchor_rx, 0, "anchors stand");
        let still_over = upsert_rule(Some(&reached), terms(22 * GB, MeterCycle::Daily), None);
        assert!(still_over.acted_this_cycle);
        let moved = upsert_rule(
            Some(&reached),
            terms(30 * GB, MeterCycle::Weekly { weekday: 1 }),
            None,
        );
        assert_eq!(moved.period_start_ms, local_ms(2026, 8, 10, 0, 0));
    }

    fn timetable() -> Schedule {
        Schedule {
            mode: ScheduleMode::Allow,
            windows: vec![ScheduleWindow {
                weekdays: vec![1, 2, 3, 4, 5],
                start_minute: 16 * 60,
                end_minute: 20 * 60,
                from_date: None,
                to_date: None,
            }],
        }
    }

    fn timetabled(allocation: u64) -> MeterRule {
        create_rule(MeterRuleTerms {
            schedule: Some(timetable()),
            ..terms(allocation, MeterCycle::Monthly { day: 1 })
        })
    }

    // SPEC-RULE-003, SPEC-RULE-005: a timetable holds the device without announcing,
    // keeps its own boundary, and is not restarted with the allowance.
    #[test]
    fn timetables_hold_quietly_on_their_own_boundary() {
        let opens = local_ms(2026, 8, 12, 16, 0);
        let closes = local_ms(2026, 8, 12, 20, 0);
        let written = timetabled(0);
        assert_eq!(written.window_blocked, Some(true));
        assert_eq!(written.window_end_ms, Some(opens));

        let (rules, transitions) = evaluate_meters(&[timetabled(0)], &read(0, 0), t0());
        assert!(transitions.is_empty());
        assert!(rule_holds_device(&rules[0]));
        assert_eq!(rules[0].reached_at_ms, None);

        let shut = MeterRule {
            window_acted: Some(true),
            ..timetabled(20 * GB)
        };
        let (rules, transitions) =
            evaluate_meters(std::slice::from_ref(&shut), &read(30 * GB, 0), t0() + 1_000);
        assert_eq!(kinds(&transitions), [MeterTransitionKind::Reached]);
        assert!(rules[0].acted_this_cycle);

        let (rules, _) = evaluate_meters(std::slice::from_ref(&shut), &read(8 * GB, 0), opens);
        assert_eq!(rules[0].window_blocked, Some(false));
        assert!(!rule_holds_device(&rules[0]));
        assert_eq!(rules[0].window_end_ms, Some(closes));
        assert_eq!(
            usage_bytes(&rules[0]),
            8 * GB,
            "the month is not re-anchored"
        );
        assert_eq!(rules[0].period_end_ms, shut.period_end_ms);

        let moved = upsert_rule(
            Some(&timetabled(0)),
            MeterRuleTerms {
                schedule: Some(Schedule {
                    mode: ScheduleMode::Allow,
                    windows: vec![ScheduleWindow {
                        start_minute: 9 * 60,
                        end_minute: 21 * 60,
                        ..timetable().windows[0].clone()
                    }],
                }),
                ..terms(0, MeterCycle::Monthly { day: 1 })
            },
            None,
        );
        assert_eq!(moved.window_blocked, Some(false));
        assert_eq!(moved.window_end_ms, Some(local_ms(2026, 8, 12, 21, 0)));

        let restarted = restart_cycle(&shut, t0());
        assert_eq!(restarted.window_blocked, Some(true));
        assert!(rule_holds_device(&restarted));
    }

    // SPEC-PAUSE-006: the alert names the allowance or timer the way the
    // Electron announcement does, and its label survives a round trip.
    #[test]
    fn alert_labels_carry_the_measure_and_the_subject() {
        assert_eq!(format_allowance(20 * GB), "20.0 GB");
        assert_eq!(format_allowance(1_500 * GB), "1.50 TB");
        assert_eq!(format_duration(45 * 60_000), "45m");
        assert_eq!(format_duration(2 * HOUR), "2h");
        assert_eq!(format_duration(80 * 60_000), "1h 20m");

        let limit = create_rule(terms(20 * GB, MeterCycle::Daily));
        let label = DataLimitLabel::for_rule(&limit, "Phone|Tablet", false);
        assert_eq!(DataLimitLabel::parse(&label.encode()), Some(label.clone()));
        assert_eq!(label.detail, "20.0 GB");
        assert!(!label.timing && !label.as_group);
        let timer = with_countdown(0, MeterCycle::Once, 2 * HOUR);
        let label = DataLimitLabel::for_rule(&timer, "Kids", true);
        assert_eq!(
            (label.detail.as_str(), label.timing, label.as_group),
            ("2h", true, true)
        );
        assert_eq!(
            DataLimitLabel::parse("Phone"),
            None,
            "an older plain label is not one"
        );
    }

    // SPEC-RULE-007: a never-rolling end survives JSON as null, and the field
    // names are the TypeScript ones.
    #[test]
    fn rules_serialize_in_the_typescript_shape() {
        let once = create_rule(terms(50 * GB, MeterCycle::Once));
        let json = serde_json::to_value(&once).expect("json");
        assert!(json["periodEndMs"].is_null());
        assert_eq!(json["cycle"], serde_json::json!({ "kind": "once" }));
        assert_eq!(json["autoPause"], false);
        let back: MeterRule = serde_json::from_value(json).expect("read back");
        assert_eq!(back, once);

        let custom = serde_json::to_value(MeterCycle::Custom {
            days: 10,
            start_ms: 5,
        })
        .unwrap();
        assert_eq!(
            custom,
            serde_json::json!({ "kind": "custom", "days": 10, "startMs": 5 })
        );

        let past = MeterRule {
            period_end_ms: Some(t0() - 1),
            ..once
        };
        let (rules, _) = evaluate_meters(&[past], &read(0, 0), t0());
        assert_eq!(
            rules[0].period_start_ms,
            t0(),
            "a once cycle opens now, not at the epoch"
        );
    }
}
