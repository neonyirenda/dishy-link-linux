//! A data allowance set across several devices at once.
//!
//! Port of upstream `core/deviceGroup.ts`. A group owns no metering arithmetic: it is
//! projected into ordinary per-device rules, one per member, so cycle rolls
//! and key resolution run exactly as they do for a device's own rule. The
//! only thing a group adds is that a pooled member is charged the group's
//! summed usage.

use serde::{Deserialize, Serialize};

use std::collections::{HashMap, HashSet};

use crate::data_meter::{
    MeterCycle, MeterReading, MeterRule, MeterRuleTerms, charged_bytes, countdown_start_ms,
    rule_key, shared_usage_by_group, upsert_rule,
};
use crate::schedule::Schedule;

/// `Pooled` charges members the sum of what they have all spent, so they
/// cross together. `PerMember` gives each the full allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupAllowanceMode {
    Pooled,
    PerMember,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceGroup {
    pub group_id: String,
    pub name: String,
    pub member_keys: Vec<String>,
    pub allocation_bytes: u64,
    pub auto_pause: bool,
    pub cycle: MeterCycle,
    pub mode: GroupAllowanceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countdown_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    pub updated_ms: i64,
    pub created_ms: i64,
}

/// An id unique against the groups already held, prefixed so it never reads
/// as a device key inside a rule key. The clock alone could collide for two
/// groups written in one millisecond, so a suffix varies until it is free.
pub fn new_group_id(taken: &[DeviceGroup], now_ms: i64) -> String {
    let held: HashSet<&str> = taken.iter().map(|group| group.group_id.as_str()).collect();
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    (0_u32..)
        .map(|attempt| {
            let suffix = (seed.wrapping_add(attempt.wrapping_mul(7_919))) % 36_u32.pow(4);
            format!(
                "group-{}{:0>4}",
                base36(now_ms.unsigned_abs()),
                base36(u64::from(suffix))
            )
        })
        .find(|candidate| !held.contains(candidate.as_str()))
        .expect("an unused id exists")
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii digits")
}

/// Every group metering a device, newest first.
pub fn groups_for_device<'a>(groups: &'a [DeviceGroup], client_key: &str) -> Vec<&'a DeviceGroup> {
    let mut found: Vec<&DeviceGroup> = groups
        .iter()
        .filter(|group| group.member_keys.iter().any(|key| key == client_key))
        .collect();
    found.sort_by_key(|group| std::cmp::Reverse(group.updated_ms));
    found
}

/// Move members onto the identities their devices now answer to, counting a
/// merged device once and dropping no member for being absent.
pub fn resolve_group_members(
    groups: &[DeviceGroup],
    resolve_key: impl Fn(&str) -> String,
) -> Vec<DeviceGroup> {
    groups
        .iter()
        .filter_map(|group| {
            let mut resolved: Vec<String> = Vec::new();
            for member in &group.member_keys {
                let current = resolve_key(member);
                if !resolved.contains(&current) {
                    resolved.push(current);
                }
            }
            (!resolved.is_empty()).then(|| DeviceGroup {
                member_keys: resolved,
                ..group.clone()
            })
        })
        .collect()
}

/// Whether a member's rule already says what its group says. A rule that does
/// is returned untouched, so projection can run on every poll.
fn carries_terms_of(rule: &MeterRule, group: &DeviceGroup) -> bool {
    rule.group_id.as_deref() == Some(group.group_id.as_str())
        && rule.shared_allowance.unwrap_or(false) == (group.mode == GroupAllowanceMode::Pooled)
        && rule.allocation_bytes == group.allocation_bytes
        && rule.auto_pause == group.auto_pause
        && rule.countdown_ms == group.countdown_ms
        && rule.cycle == group.cycle
        && rule.schedule == group.schedule
}

/// The group countdown's one clock: the start of a member already running
/// this duration, else now. An edit to the other terms keeps it; a changed
/// duration finds no such member and starts afresh.
fn group_clock(group: &DeviceGroup, rules: &[MeterRule], now_ms: i64) -> Option<i64> {
    group.countdown_ms?;
    Some(
        rules
            .iter()
            .find(|rule| {
                rule.group_id.as_deref() == Some(group.group_id.as_str())
                    && rule.countdown_ms == group.countdown_ms
            })
            .map_or(now_ms, countdown_start_ms),
    )
}

/// The rule set the groups and the devices' own rules describe together: one
/// rule per group per member, keeping each member's anchors, plus every
/// device's own rule. A rule left from a group the device has left is dropped.
pub fn project_group_rules(
    groups: &[DeviceGroup],
    rules: &[MeterRule],
    counters: &[MeterReading],
    now_ms: i64,
) -> Vec<MeterRule> {
    let by_key: HashMap<String, &MeterRule> = rules
        .iter()
        .map(|rule| (rule_key(&rule.client_key, rule.group_id.as_deref()), rule))
        .collect();
    let counter_by_key: HashMap<&str, &MeterReading> = counters
        .iter()
        .map(|reading| (reading.client_key.as_str(), reading))
        .collect();
    let shared_usage = shared_usage_by_group(rules);
    let mut projected = Vec::new();
    let mut claimed = HashSet::new();

    for group in groups {
        let clock = group_clock(group, rules, now_ms);
        for client_key in &group.member_keys {
            let key = rule_key(client_key, Some(&group.group_id));
            if !claimed.insert(key.clone()) {
                continue;
            }
            let existing = by_key.get(&key).copied();
            if let Some(existing) = existing
                && carries_terms_of(existing, group)
            {
                projected.push(existing.clone());
                continue;
            }
            let counter = counter_by_key.get(client_key.as_str());
            let terms = MeterRuleTerms {
                client_key: client_key.clone(),
                allocation_bytes: group.allocation_bytes,
                auto_pause: group.auto_pause,
                cycle: group.cycle.clone(),
                lifetime_rx: counter
                    .map(|counter| counter.lifetime_rx)
                    .or(existing.map(|rule| rule.observed_rx))
                    .unwrap_or(0),
                lifetime_tx: counter
                    .map(|counter| counter.lifetime_tx)
                    .or(existing.map(|rule| rule.observed_tx))
                    .unwrap_or(0),
                now_ms,
                group_id: Some(group.group_id.clone()),
                shared_allowance: group.mode == GroupAllowanceMode::Pooled,
                countdown_ms: group.countdown_ms,
                schedule: group.schedule.clone(),
            };
            let charged = existing.map_or(0, |rule| charged_bytes(rule, &shared_usage));
            let mut written = upsert_rule(existing, terms, Some(charged));
            if let Some(clock) = clock {
                written.countdown_start_ms = Some(clock);
                // A timer still on its old clock keeps its latch, so an edit
                // beside it cannot hand back a sitting already spent.
                if let Some(existing) = existing
                    && clock == countdown_start_ms(existing)
                {
                    written.countdown_acted = existing.countdown_acted;
                }
            }
            projected.push(written);
        }
    }
    projected.extend(rules.iter().filter(|rule| rule.group_id.is_none()).cloned());
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_meter::tests::{GB, local_ms};
    use crate::data_meter::{
        MeterRuleTerms, MeterTransitionKind, collapse_group_announcements, countdown_start_ms,
        create_rule, evaluate_meters, resolve_rule_keys, rule_holds_device, usage_bytes,
    };

    const TABLET: &str = "101";
    const CONSOLE: &str = "102";
    const LAPTOP: &str = "103";

    fn t0() -> i64 {
        local_ms(2026, 8, 12, 15, 30)
    }

    fn group() -> DeviceGroup {
        DeviceGroup {
            group_id: "kids".into(),
            name: "Kids".into(),
            member_keys: vec![TABLET.into(), CONSOLE.into()],
            allocation_bytes: 50 * GB,
            auto_pause: false,
            cycle: MeterCycle::Monthly { day: 1 },
            mode: GroupAllowanceMode::Pooled,
            countdown_ms: None,
            schedule: None,
            updated_ms: t0(),
            created_ms: t0(),
        }
    }

    fn counters(entries: &[(&str, u64)]) -> Vec<MeterReading> {
        entries
            .iter()
            .map(|(key, rx)| MeterReading {
                client_key: (*key).into(),
                lifetime_rx: *rx,
                lifetime_tx: 0,
            })
            .collect()
    }

    fn rules_at(group: &DeviceGroup, at: &[(&str, u64)]) -> Vec<MeterRule> {
        let zeros: Vec<(&str, u64)> = group
            .member_keys
            .iter()
            .map(|key| (key.as_str(), 0))
            .collect();
        let opened = project_group_rules(std::slice::from_ref(group), &[], &counters(&zeros), t0());
        evaluate_meters(&opened, &counters(at), t0() + 1_000).0
    }

    fn own_rule(observed_rx: u64) -> MeterRule {
        let mut rule = create_rule(MeterRuleTerms::new(
            TABLET,
            10 * GB,
            MeterCycle::Monthly { day: 1 },
            0,
            0,
            t0(),
        ));
        rule.observed_rx = observed_rx;
        rule
    }

    // SPEC-RULE-006: one rule per member carrying the group's terms; a device's
    // own rule stands beside its group rule; a leaver loses its group rule.
    #[test]
    fn groups_project_one_rule_per_member() {
        let rules = project_group_rules(
            &[group()],
            &[],
            &counters(&[(TABLET, 0), (CONSOLE, 0)]),
            t0(),
        );
        assert_eq!(
            rules
                .iter()
                .map(|rule| rule.client_key.as_str())
                .collect::<Vec<_>>(),
            [TABLET, CONSOLE]
        );
        assert!(rules.iter().all(|rule| rule.allocation_bytes == 50 * GB));
        assert!(
            rules
                .iter()
                .all(|rule| rule.group_id.as_deref() == Some("kids"))
        );
        assert!(rules.iter().all(|rule| rule.shared_allowance == Some(true)));

        let per_member = DeviceGroup {
            mode: GroupAllowanceMode::PerMember,
            ..group()
        };
        let rules = project_group_rules(
            &[per_member],
            &[],
            &counters(&[(TABLET, 0), (CONSOLE, 0)]),
            t0(),
        );
        assert!(rules.iter().all(|rule| rule.shared_allowance.is_none()));

        let tablet_only = DeviceGroup {
            member_keys: vec![TABLET.into()],
            ..group()
        };
        let joined = project_group_rules(
            std::slice::from_ref(&tablet_only),
            &[own_rule(4 * GB)],
            &counters(&[(TABLET, 4 * GB)]),
            t0(),
        );
        let carried = joined.iter().find(|rule| rule.group_id.is_none()).unwrap();
        let from_group = joined.iter().find(|rule| rule.group_id.is_some()).unwrap();
        assert_eq!(
            (usage_bytes(carried), carried.allocation_bytes),
            (4 * GB, 10 * GB)
        );
        assert_eq!(
            (usage_bytes(from_group), from_group.allocation_bytes),
            (0, 50 * GB)
        );

        let held = rules_at(&group(), &[(TABLET, GB), (CONSOLE, GB)]);
        let shrunk = project_group_rules(
            &[tablet_only],
            &held,
            &counters(&[(TABLET, GB), (CONSOLE, GB)]),
            t0(),
        );
        assert_eq!(
            shrunk
                .iter()
                .map(|rule| rule.client_key.as_str())
                .collect::<Vec<_>>(),
            [TABLET]
        );

        let first = project_group_rules(
            &[group()],
            &[],
            &counters(&[(TABLET, 0), (CONSOLE, 0)]),
            t0(),
        );
        let again = project_group_rules(
            &[group()],
            &first,
            &counters(&[(TABLET, 0), (CONSOLE, 0)]),
            t0() + 200,
        );
        assert_eq!(
            again, first,
            "rules already carrying the terms are untouched"
        );

        let older = DeviceGroup {
            group_id: "old".into(),
            member_keys: vec![TABLET.into()],
            updated_ms: t0() - 1_000,
            ..group()
        };
        let newer = DeviceGroup {
            group_id: "new".into(),
            member_keys: vec![TABLET.into()],
            ..group()
        };
        let rules = project_group_rules(
            &[older.clone(), newer.clone()],
            &[],
            &counters(&[(TABLET, 0)]),
            t0(),
        );
        let mut ids: Vec<_> = rules
            .iter()
            .filter_map(|rule| rule.group_id.clone())
            .collect();
        ids.sort();
        assert_eq!(ids, ["new", "old"]);
        let listed = [older, newer];
        let for_tablet = groups_for_device(&listed, TABLET);
        assert_eq!(
            for_tablet.first().map(|group| group.group_id.as_str()),
            Some("new")
        );
    }

    // SPEC-RULE-006: a pooled allowance takes every member together, once
    // announced; a per-member one takes only the member over it.
    #[test]
    fn pooled_allowances_cross_together() {
        let rules = rules_at(&group(), &[(TABLET, 30 * GB), (CONSOLE, 25 * GB)]);
        assert!(rules.iter().all(rule_holds_device));
        let rules = rules_at(&group(), &[(TABLET, 20 * GB), (CONSOLE, 20 * GB)]);
        assert!(!rules.iter().any(|rule| rule.acted_this_cycle));
        let rules = rules_at(&group(), &[(TABLET, 55 * GB), (CONSOLE, 0)]);
        assert!(rules.iter().all(rule_holds_device));

        let per_member = DeviceGroup {
            mode: GroupAllowanceMode::PerMember,
            ..group()
        };
        let rules = rules_at(&per_member, &[(TABLET, 55 * GB), (CONSOLE, 10 * GB)]);
        let held = |key: &str| {
            rule_holds_device(rules.iter().find(|rule| rule.client_key == key).unwrap())
        };
        assert!(held(TABLET) && !held(CONSOLE));

        let opened = project_group_rules(
            &[group()],
            &[],
            &counters(&[(TABLET, 0), (CONSOLE, 0)]),
            t0(),
        );
        let (_, transitions) = evaluate_meters(
            &opened,
            &counters(&[(TABLET, 30 * GB), (CONSOLE, 25 * GB)]),
            t0() + 1_000,
        );
        assert_eq!(transitions.len(), 2);
        let collapsed = collapse_group_announcements(transitions);
        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].kind, MeterTransitionKind::Reached);
    }

    // SPEC-RULE-006: identity merges move members and rules, count a merged
    // device once, keep the newest rule, and never drop for absence.
    #[test]
    fn identity_merges_move_members_and_rules() {
        let resolved = resolve_group_members(&[group()], |key| {
            if key == CONSOLE {
                LAPTOP.into()
            } else {
                key.into()
            }
        });
        assert_eq!(resolved[0].member_keys, [TABLET, LAPTOP]);
        assert_eq!(
            resolve_group_members(&[group()], |_| TABLET.into())[0].member_keys,
            [TABLET]
        );
        assert_eq!(
            resolve_group_members(&[group()], str::to_string)[0].member_keys,
            [TABLET, CONSOLE]
        );

        let older = own_rule(0);
        let mut newer = own_rule(0);
        newer.client_key = LAPTOP.into();
        newer.allocation_bytes = 99 * GB;
        newer.updated_ms = Some(t0() + 5);
        let moved = resolve_rule_keys(&[older, newer], |_| TABLET.into());
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].allocation_bytes, 99 * GB);
        assert_eq!(moved[0].client_key, TABLET);
    }

    // SPEC-RULE-006: a device joining a group drops the announcement filed
    // under its own key; a group countdown runs on one clock that a later
    // member joins and an unrelated edit keeps.
    #[test]
    fn group_countdowns_share_one_clock() {
        let mut announced = own_rule(20 * GB);
        announced.acted_this_cycle = true;
        announced.reached_at_ms = Some(t0());
        announced.group_id = Some("kids".into());
        let joined = project_group_rules(
            &[group()],
            &[announced],
            &counters(&[(TABLET, 20 * GB), (CONSOLE, 0)]),
            t0(),
        );
        assert_eq!(
            joined
                .iter()
                .find(|rule| rule.client_key == TABLET)
                .unwrap()
                .reached_at_ms,
            None
        );

        let timer = |members: &[&str], allocation: u64| DeviceGroup {
            countdown_ms: Some(3_600_000),
            cycle: MeterCycle::Once,
            mode: GroupAllowanceMode::PerMember,
            member_keys: members.iter().map(|key| (*key).into()).collect(),
            allocation_bytes: allocation,
            ..group()
        };
        let running = project_group_rules(&[timer(&[TABLET], 50 * GB)], &[], &[], t0());
        let grown = project_group_rules(
            &[timer(&[TABLET, CONSOLE], 50 * GB)],
            &running,
            &[],
            t0() + 600_000,
        );
        assert!(grown.iter().all(|rule| countdown_start_ms(rule) == t0()));

        let edited =
            project_group_rules(&[timer(&[TABLET], 5 * GB)], &running, &[], t0() + 600_000);
        assert_eq!(countdown_start_ms(&edited[0]), t0());

        let moved_group = DeviceGroup {
            group_id: "second".into(),
            ..timer(&[TABLET], 50 * GB)
        };
        let moved = project_group_rules(&[moved_group], &running, &[], t0() + 3_000_000);
        assert_eq!(countdown_start_ms(&moved[0]), t0() + 3_000_000);

        let taken = [group()];
        let id = new_group_id(&taken, t0());
        assert!(id.starts_with("group-") && id != "kids");
    }
}
