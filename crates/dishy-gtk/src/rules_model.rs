//! The Rules tab's view model: what each card says, and how the rule dialog's
//! fields become a recorder command.
//!
//! Port of the arithmetic behind upstream `components/network/rules/RuleCard.tsx`,
//! upstream `RuleDialog.tsx`, and the pause gate in upstream `lib/routerClientUpdate.ts`. Kept
//! apart from the widgets so every line a card shows is testable without a
//! display. Rules are watch-only here (spec 0169); a reached rule offers an
//! explicit pause and never sends one (spec 0170).

use std::collections::HashMap;

use dishy_core::data_meter::{
    MeterCycle, MeterRule, MeterRuleTerms, charged_bytes, countdown_left_ms, format_allowance,
    format_duration, rule_holds_device, shared_usage_by_group,
};
use dishy_core::device_group::{DeviceGroup, GroupAllowanceMode, new_group_id};
use dishy_core::schedule::{Schedule, ScheduleMode, schedule_governs};

use crate::i18n::Translator;
use crate::recorder::{RecorderCommand, RulesSnapshot};

/// The three kinds of rule the dialog offers, as in Electron.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Limit,
    Schedule,
    Timer,
}

/// What a card shows for one device rule or one group.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleCard {
    /// The device key for a device's own rule, or `None` for a group.
    pub client_key: Option<String>,
    pub group_id: Option<String>,
    pub title: String,
    pub members: String,
    /// "20.0 GB shared · Monthly", "2h timer", "On a schedule".
    pub measure: String,
    /// Usage against the allowance, 0–1, when the rule has an allowance.
    pub fraction: Option<f64>,
    /// "4.2 GB of 20.0 GB" / "1h 10m left" / "Active".
    pub progress: String,
    /// Whether any member's rule has been reached or is holding.
    pub reached: bool,
}

/// The dialog's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleForm {
    pub kind: RuleKind,
    pub devices: Vec<String>,
    pub name: String,
    pub allowance_gb: f64,
    pub cycle: MeterCycle,
    pub shared: bool,
    pub countdown_minutes: u32,
    pub schedule: Schedule,
    /// Set when editing an existing group.
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    NoDevices,
    NoAllowance,
    NoCountdown,
    NoWindows,
}

/// A pause can be offered only with an account, a known client id, an
/// identified viewer, and a row that is not the viewer's own: a paused device
/// cannot unpause itself (SPEC-PAUSE-003).
pub fn pause_available(
    cloud_connected: bool,
    viewer_identified: bool,
    is_self: bool,
    client_id: Option<i64>,
) -> bool {
    client_id.is_some()
        && pause_refusal_reason(cloud_connected, viewer_identified, is_self).is_none()
}

/// Why a pause control is not offered, when `pause_available` refuses
/// (SPEC-PAUSE-007). The page-wide reasons come first — no account, then an
/// unidentified viewer — and the viewer's own row is explained only once the
/// rest of the gate holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseRefusal {
    /// `network.pauseDisabledAccount`
    Account,
    /// `network.pauseDisabledSelf`
    SelfDevice,
    /// `network.pauseDisabledBoth`
    Both,
    /// `errors.selfPauseRefused`
    OwnRow,
}

impl PauseRefusal {
    /// The shared-catalog key that states this refusal.
    pub fn message_key(self) -> &'static str {
        match self {
            PauseRefusal::Account => "network.pauseDisabledAccount",
            PauseRefusal::SelfDevice => "network.pauseDisabledSelf",
            PauseRefusal::Both => "network.pauseDisabledBoth",
            PauseRefusal::OwnRow => "errors.selfPauseRefused",
        }
    }
}

pub fn pause_refusal_reason(
    cloud_connected: bool,
    viewer_identified: bool,
    is_self: bool,
) -> Option<PauseRefusal> {
    match (cloud_connected, viewer_identified, is_self) {
        (false, false, _) => Some(PauseRefusal::Both),
        (false, true, _) => Some(PauseRefusal::Account),
        (true, false, _) => Some(PauseRefusal::SelfDevice),
        (true, true, true) => Some(PauseRefusal::OwnRow),
        (true, true, false) => None,
    }
}

pub fn cycle_label(cycle: &MeterCycle, translator: &Translator) -> String {
    match cycle {
        MeterCycle::Daily => translator.text("rules.cycleDaily"),
        MeterCycle::Weekly { .. } => translator.text("rules.cycleWeekly"),
        MeterCycle::Monthly { .. } => translator.text("rules.cycleMonthly"),
        MeterCycle::Billing { .. } => translator.text("rules.cycleBilling"),
        MeterCycle::Once => translator.text("rules.cycleOnce"),
        MeterCycle::Custom { days, .. } => {
            translator.text_with("rules.cycleCustom", &[("count", &days.to_string())])
        }
    }
}

fn device_name(key: &str, names: &HashMap<String, String>, translator: &Translator) -> String {
    names
        .get(key)
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| translator.text_with("rules.deviceFallback", &[("key", key)]))
}

/// "Tablet", "Tablet and Console", "Tablet and 2 others".
fn members_label(
    keys: &[String],
    names: &HashMap<String, String>,
    translator: &Translator,
) -> String {
    let first = keys
        .first()
        .map(|key| device_name(key, names, translator))
        .unwrap_or_default();
    match keys.len() {
        0 | 1 => first,
        2 => translator.text_with(
            "rules.groupNameTwo",
            &[
                ("first", &first),
                ("second", &device_name(&keys[1], names, translator)),
            ],
        ),
        count => translator.text_with(
            "rules.groupNameMany",
            &[("first", &first), ("count", &(count - 1).to_string())],
        ),
    }
}

/// What a rule measures and how far along it is, for a device rule or for a
/// group's member rules.
fn summarise(
    rules: &[&MeterRule],
    shared: Option<bool>,
    shared_usage: &HashMap<String, u64>,
    now_ms: i64,
    translator: &Translator,
) -> (String, Option<f64>, String) {
    let Some(first) = rules.first() else {
        return (String::new(), None, String::new());
    };
    let cycle = cycle_label(&first.cycle, translator);
    if let Some(countdown) = first.countdown_ms {
        let left = rules
            .iter()
            .filter_map(|rule| countdown_left_ms(rule, now_ms))
            .min()
            .unwrap_or(countdown);
        return (
            translator.text("rules.timer"),
            None,
            translator.text_with(
                "rules.durationLeft",
                &[("duration", &format_duration(left))],
            ),
        );
    }
    if first.allocation_bytes == 0 {
        let progress = match &first.schedule {
            Some(schedule) if !schedule_governs(schedule, now_ms) => {
                translator.text("rules.notScheduledToday")
            }
            Some(_) if first.window_blocked == Some(true) => {
                format!(
                    "{} · {}",
                    translator.text("rules.onASchedule"),
                    translator.text("rules.holding")
                )
            }
            _ => translator.text("rules.active"),
        };
        return (translator.text("rules.onASchedule"), None, progress);
    }
    let total = first.allocation_bytes;
    let used = rules
        .iter()
        .map(|rule| charged_bytes(rule, shared_usage))
        .max()
        .unwrap_or(0);
    let cap = format_allowance(total);
    let measure_key = match shared {
        Some(true) => translator.text_with("rules.capShared", &[("bytes", &cap)]),
        Some(false) => translator.text_with("rules.capEach", &[("bytes", &cap)]),
        None => translator.text_with("rules.limitCap", &[("cap", &cap)]),
    };
    (
        format!("{measure_key} · {cycle}"),
        Some((used as f64 / total as f64).min(1.0)),
        translator.text_with(
            "rules.usedOfTotal",
            &[("used", &format_allowance(used)), ("total", &cap)],
        ),
    )
}

/// One card per device rule and per group, groups first, newest first.
pub fn rule_cards(
    snapshot: &RulesSnapshot,
    names: &HashMap<String, String>,
    now_ms: i64,
    translator: &Translator,
) -> Vec<RuleCard> {
    let shared_usage = shared_usage_by_group(&snapshot.rules);
    let mut groups: Vec<&DeviceGroup> = snapshot.groups.iter().collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.updated_ms));
    let mut cards: Vec<RuleCard> = groups
        .into_iter()
        .map(|group| {
            let rules: Vec<&MeterRule> = snapshot
                .rules
                .iter()
                .filter(|rule| rule.group_id.as_deref() == Some(group.group_id.as_str()))
                .collect();
            let (measure, fraction, progress) = summarise(
                &rules,
                Some(group.mode == GroupAllowanceMode::Pooled),
                &shared_usage,
                now_ms,
                translator,
            );
            RuleCard {
                client_key: None,
                group_id: Some(group.group_id.clone()),
                title: group.name.clone(),
                members: members_label(&group.member_keys, names, translator),
                measure,
                fraction,
                progress,
                reached: rules.iter().any(|rule| rule_holds_device(rule)),
            }
        })
        .collect();
    let mut own: Vec<&MeterRule> = snapshot
        .rules
        .iter()
        .filter(|rule| rule.group_id.is_none())
        .collect();
    own.sort_by_key(|rule| std::cmp::Reverse(rule.updated_ms.unwrap_or(rule.created_ms)));
    cards.extend(own.into_iter().map(|rule| {
        let (measure, fraction, progress) =
            summarise(&[rule], None, &shared_usage, now_ms, translator);
        let name = device_name(&rule.client_key, names, translator);
        RuleCard {
            client_key: Some(rule.client_key.clone()),
            group_id: None,
            title: name.clone(),
            members: name,
            measure,
            fraction,
            progress,
            reached: rule_holds_device(rule),
        }
    }));
    cards
}

fn empty_schedule() -> Schedule {
    Schedule {
        mode: ScheduleMode::Allow,
        windows: Vec::new(),
    }
}

/// The form for a new rule on these devices.
pub fn new_form(devices: Vec<String>) -> RuleForm {
    RuleForm {
        kind: RuleKind::Limit,
        devices,
        name: String::new(),
        allowance_gb: 10.0,
        cycle: MeterCycle::Monthly { day: 1 },
        shared: true,
        countdown_minutes: 0,
        schedule: empty_schedule(),
        group_id: None,
    }
}

fn kind_of(
    allocation_bytes: u64,
    countdown_ms: Option<i64>,
    schedule: Option<&Schedule>,
) -> RuleKind {
    if countdown_ms.is_some() {
        RuleKind::Timer
    } else if allocation_bytes == 0 && schedule.is_some() {
        RuleKind::Schedule
    } else {
        RuleKind::Limit
    }
}

/// The form for editing a card's rule or group.
pub fn form_for(card: &RuleCard, snapshot: &RulesSnapshot) -> Option<RuleForm> {
    let minutes = |countdown: Option<i64>| countdown.map_or(0, |ms| (ms / 60_000) as u32);
    if let Some(group_id) = &card.group_id {
        let group = snapshot
            .groups
            .iter()
            .find(|group| &group.group_id == group_id)?;
        return Some(RuleForm {
            kind: kind_of(
                group.allocation_bytes,
                group.countdown_ms,
                group.schedule.as_ref(),
            ),
            devices: group.member_keys.clone(),
            name: group.name.clone(),
            allowance_gb: group.allocation_bytes as f64 / 1e9,
            cycle: group.cycle.clone(),
            shared: group.mode == GroupAllowanceMode::Pooled,
            countdown_minutes: minutes(group.countdown_ms),
            schedule: group.schedule.clone().unwrap_or_else(empty_schedule),
            group_id: Some(group.group_id.clone()),
        });
    }
    let key = card.client_key.as_ref()?;
    let rule = snapshot
        .rules
        .iter()
        .find(|rule| rule.group_id.is_none() && &rule.client_key == key)?;
    Some(RuleForm {
        kind: kind_of(
            rule.allocation_bytes,
            rule.countdown_ms,
            rule.schedule.as_ref(),
        ),
        devices: vec![key.clone()],
        name: String::new(),
        allowance_gb: rule.allocation_bytes as f64 / 1e9,
        cycle: rule.cycle.clone(),
        shared: true,
        countdown_minutes: minutes(rule.countdown_ms),
        schedule: rule.schedule.clone().unwrap_or_else(empty_schedule),
        group_id: None,
    })
}

/// The recorder command a saved form sends: a device's own rule for one
/// device, a group for several.
pub fn form_command(
    form: &RuleForm,
    snapshot: &RulesSnapshot,
    names: &HashMap<String, String>,
    now_ms: i64,
    translator: &Translator,
) -> Result<RecorderCommand, FormError> {
    if form.devices.is_empty() {
        return Err(FormError::NoDevices);
    }
    let windows = (!form.schedule.windows.is_empty()).then(|| form.schedule.clone());
    let (allocation_bytes, countdown_ms, schedule) = match form.kind {
        RuleKind::Limit => {
            if form.allowance_gb.is_nan() || form.allowance_gb <= 0.0 {
                return Err(FormError::NoAllowance);
            }
            ((form.allowance_gb * 1e9).round() as u64, None, windows)
        }
        RuleKind::Timer => {
            if form.countdown_minutes == 0 {
                return Err(FormError::NoCountdown);
            }
            (0, Some(i64::from(form.countdown_minutes) * 60_000), None)
        }
        RuleKind::Schedule => (0, None, Some(windows.ok_or(FormError::NoWindows)?)),
    };
    if form.devices.len() == 1 && form.group_id.is_none() {
        return Ok(RecorderCommand::UpsertDeviceRule(MeterRuleTerms {
            countdown_ms,
            schedule,
            ..MeterRuleTerms::new(
                form.devices[0].clone(),
                allocation_bytes,
                form.cycle.clone(),
                0,
                0,
                now_ms,
            )
        }));
    }
    let existing = form
        .group_id
        .as_ref()
        .and_then(|id| snapshot.groups.iter().find(|group| &group.group_id == id));
    let name = form.name.trim();
    Ok(RecorderCommand::UpsertGroup(DeviceGroup {
        group_id: existing.map_or_else(
            || new_group_id(&snapshot.groups, now_ms),
            |group| group.group_id.clone(),
        ),
        name: if name.is_empty() {
            members_label(&form.devices, names, translator)
        } else {
            name.to_owned()
        },
        member_keys: form.devices.clone(),
        allocation_bytes,
        auto_pause: false,
        cycle: form.cycle.clone(),
        mode: if form.shared {
            GroupAllowanceMode::Pooled
        } else {
            GroupAllowanceMode::PerMember
        },
        countdown_ms,
        schedule,
        updated_ms: now_ms,
        created_ms: existing.map_or(now_ms, |group| group.created_ms),
    }))
}

#[cfg(test)]
mod tests {
    use dishy_core::data_meter::create_rule;
    use dishy_core::schedule::ScheduleWindow;

    use super::*;
    use crate::i18n::Locale;

    const GB: u64 = 1_000_000_000;
    const NOW: i64 = 1_786_000_000_000;

    fn english() -> Translator {
        Translator::new(Locale::En)
    }

    fn names() -> HashMap<String, String> {
        [("1", "Tablet"), ("2", "Console"), ("3", "Laptop")]
            .into_iter()
            .map(|(key, name)| (key.to_string(), name.to_string()))
            .collect()
    }

    fn own_rule(key: &str, allocation: u64, used: u64) -> MeterRule {
        let mut rule = create_rule(MeterRuleTerms::new(
            key,
            allocation,
            MeterCycle::Monthly { day: 1 },
            0,
            0,
            NOW,
        ));
        rule.observed_rx = used;
        rule
    }

    // SPEC-PAUSE-003: the gate needs all four conditions.
    #[test]
    fn the_pause_gate_needs_an_account_an_identified_viewer_and_another_device() {
        assert!(pause_available(true, true, false, Some(7)));
        assert!(!pause_available(false, true, false, Some(7)));
        assert!(!pause_available(true, false, false, Some(7)));
        assert!(!pause_available(true, true, true, Some(7)));
        assert!(!pause_available(true, true, false, None));
    }

    // SPEC-PAUSE-007: each refusal names its reason; the page-wide ones come
    // first, and the viewer's own row is explained only once the rest of the
    // gate holds.
    #[test]
    fn each_pause_refusal_names_its_reason() {
        assert_eq!(
            pause_refusal_reason(false, false, false),
            Some(PauseRefusal::Both)
        );
        assert_eq!(
            pause_refusal_reason(false, true, true),
            Some(PauseRefusal::Account)
        );
        assert_eq!(
            pause_refusal_reason(true, false, false),
            Some(PauseRefusal::SelfDevice)
        );
        assert_eq!(
            pause_refusal_reason(true, true, true),
            Some(PauseRefusal::OwnRow)
        );
        assert_eq!(pause_refusal_reason(true, true, false), None);
    }

    // SPEC-PAUSE-005: cards say what each rule measures and how far along it
    // is, in the catalog's words.
    #[test]
    fn cards_summarise_allowances_timers_and_groups() {
        let translator = english();
        let mut timer = create_rule(MeterRuleTerms {
            countdown_ms: Some(2 * 3_600_000),
            ..MeterRuleTerms::new("3", 0, MeterCycle::Once, 0, 0, NOW - 50 * 60_000)
        });
        timer.updated_ms = Some(NOW - 50 * 60_000);
        let group = DeviceGroup {
            group_id: "group-kids".into(),
            name: "Kids".into(),
            member_keys: vec!["1".into(), "2".into()],
            allocation_bytes: 10 * GB,
            auto_pause: false,
            cycle: MeterCycle::Daily,
            mode: GroupAllowanceMode::Pooled,
            countdown_ms: None,
            schedule: None,
            updated_ms: NOW,
            created_ms: NOW,
        };
        let member = |key: &str, used: u64| {
            let mut rule = create_rule(MeterRuleTerms {
                group_id: Some("group-kids".into()),
                shared_allowance: true,
                ..MeterRuleTerms::new(key, 10 * GB, MeterCycle::Daily, 0, 0, NOW)
            });
            rule.observed_rx = used;
            rule
        };
        let snapshot = RulesSnapshot {
            rules: vec![
                own_rule("1", 20 * GB, 5 * GB),
                timer,
                member("1", 6 * GB),
                member("2", 5 * GB),
            ],
            groups: vec![group],
        };
        let cards = rule_cards(&snapshot, &names(), NOW, &translator);
        assert_eq!(cards.len(), 3);

        let kids = &cards[0];
        assert_eq!(
            (kids.title.as_str(), kids.members.as_str()),
            ("Kids", "Tablet and Console")
        );
        assert_eq!(kids.measure, "10.0 GB shared · Daily");
        assert_eq!(kids.fraction, Some(1.0));
        assert!(kids.reached || kids.progress.contains("11.0 GB"));

        let tablet = cards
            .iter()
            .find(|card| card.client_key.as_deref() == Some("1"))
            .unwrap();
        assert_eq!(tablet.title, "Tablet");
        assert_eq!(tablet.measure, "Limit: 20.0 GB · Monthly");
        assert_eq!(tablet.fraction, Some(0.25));
        assert_eq!(tablet.progress, "5.0 GB of 20.0 GB");

        let laptop = cards
            .iter()
            .find(|card| card.client_key.as_deref() == Some("3"))
            .unwrap();
        assert_eq!(laptop.measure, "Timer");
        assert_eq!(laptop.progress, "1h 10m left");
        assert_eq!(laptop.fraction, None);
    }

    fn window() -> ScheduleWindow {
        ScheduleWindow {
            weekdays: vec![1, 2, 3, 4, 5],
            start_minute: 16 * 60,
            end_minute: 20 * 60,
            from_date: None,
            to_date: None,
        }
    }

    // SPEC-PAUSE-005: a form for one device writes its own watch-only rule; a
    // form for several writes a group; incomplete forms are refused.
    #[test]
    fn forms_become_typed_recorder_commands() {
        let translator = english();
        let snapshot = RulesSnapshot::default();
        let mut form = new_form(vec!["1".into()]);
        form.allowance_gb = 20.0;
        match form_command(&form, &snapshot, &names(), NOW, &translator).unwrap() {
            RecorderCommand::UpsertDeviceRule(terms) => {
                assert_eq!(terms.client_key, "1");
                assert_eq!(terms.allocation_bytes, 20 * GB);
                assert!(!terms.auto_pause);
                assert_eq!(terms.cycle, MeterCycle::Monthly { day: 1 });
            }
            other => panic!("{other:?}"),
        }

        form.devices = vec!["1".into(), "2".into(), "3".into()];
        form.shared = true;
        match form_command(&form, &snapshot, &names(), NOW, &translator).unwrap() {
            RecorderCommand::UpsertGroup(group) => {
                assert!(group.group_id.starts_with("group-"));
                assert_eq!(group.name, "Tablet and 2 others");
                assert_eq!(group.mode, GroupAllowanceMode::Pooled);
                assert!(!group.auto_pause);
            }
            other => panic!("{other:?}"),
        }

        let mut timer = new_form(vec!["3".into()]);
        timer.kind = RuleKind::Timer;
        assert_eq!(
            form_command(&timer, &snapshot, &names(), NOW, &translator).err(),
            Some(FormError::NoCountdown)
        );
        timer.countdown_minutes = 90;
        match form_command(&timer, &snapshot, &names(), NOW, &translator).unwrap() {
            RecorderCommand::UpsertDeviceRule(terms) => {
                assert_eq!(terms.countdown_ms, Some(90 * 60_000));
                assert_eq!(terms.allocation_bytes, 0);
            }
            other => panic!("{other:?}"),
        }

        let mut hours = new_form(vec!["2".into()]);
        hours.kind = RuleKind::Schedule;
        assert_eq!(
            form_command(&hours, &snapshot, &names(), NOW, &translator).err(),
            Some(FormError::NoWindows)
        );
        hours.schedule.windows.push(window());
        assert!(matches!(
            form_command(&hours, &snapshot, &names(), NOW, &translator),
            Ok(RecorderCommand::UpsertDeviceRule(terms)) if terms.schedule.is_some()
        ));

        assert_eq!(
            form_command(&new_form(Vec::new()), &snapshot, &names(), NOW, &translator).err(),
            Some(FormError::NoDevices)
        );
        let mut empty = new_form(vec!["1".into()]);
        empty.allowance_gb = 0.0;
        assert_eq!(
            form_command(&empty, &snapshot, &names(), NOW, &translator).err(),
            Some(FormError::NoAllowance)
        );
    }

    // SPEC-PAUSE-005: editing reopens the form a card was saved from.
    #[test]
    fn editing_reopens_the_saved_terms() {
        let translator = english();
        let mut rule = own_rule("1", 20 * GB, 0);
        rule.schedule = Some(Schedule {
            mode: ScheduleMode::Block,
            windows: vec![window()],
        });
        let snapshot = RulesSnapshot {
            rules: vec![rule],
            groups: Vec::new(),
        };
        let card = &rule_cards(&snapshot, &names(), NOW, &translator)[0];
        let form = form_for(card, &snapshot).expect("form");
        assert_eq!(form.devices, ["1"]);
        assert_eq!(form.allowance_gb, 20.0);
        assert_eq!(form.schedule.mode, ScheduleMode::Block);
        assert_eq!(form.kind, RuleKind::Limit);
        assert_eq!(
            cycle_label(
                &MeterCycle::Custom {
                    days: 10,
                    start_ms: 0
                },
                &translator
            ),
            "Every 10 days"
        );
    }
}
