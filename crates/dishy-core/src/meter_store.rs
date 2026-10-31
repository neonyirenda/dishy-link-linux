//! The recorder's metering rules and groups, on disk in the TypeScript
//! historian's `meters.json` and `device-groups.json` shapes.
//!
//! Port of upstream `collector/meterStore.mts` and upstream `collector/groupStore.mts`, without
//! enforcement: the `pauses` the TypeScript historian keeps are preserved
//! verbatim and never acted on (spec 0169). A device's own monthly rule with
//! no countdown or timetable doubles as that device's monthly limit, which is
//! how the spec 0160 limits migrate in.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::data_meter::{
    MeterCycle, MeterReading, MeterRule, MeterRuleTerms, MeterTransition,
    collapse_group_announcements, create_rule, evaluate_meters, resolve_rule_keys, restart_cycle,
    rule_key, upsert_rule,
};
use crate::device_group::{DeviceGroup, project_group_rules, resolve_group_members};
use crate::meter::ClientMonthlyLimit;

const METERS_VERSION: u32 = 2;
const GROUPS_VERSION: u32 = 1;

/// How long advancing counters may sit unwritten. They are re-read from the
/// odometer after a restart, so this only spares the disk.
const COUNTER_FLUSH_MS: i64 = 30_000;

/// What the odometer knows about one device: its lifetime counter and how much
/// of it fell in the current month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifetimeReading {
    pub client_key: String,
    pub lifetime_rx: u64,
    pub lifetime_tx: u64,
    pub month_rx: u64,
    pub month_tx: u64,
}

impl LifetimeReading {
    pub fn meter_reading(&self) -> MeterReading {
        MeterReading {
            client_key: self.client_key.clone(),
            lifetime_rx: self.lifetime_rx,
            lifetime_tx: self.lifetime_tx,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MetersSnapshot {
    version: u32,
    rules: Vec<MeterRule>,
    #[serde(default)]
    pauses: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GroupsSnapshot {
    version: u32,
    groups: Vec<DeviceGroup>,
}

#[derive(Debug, Deserialize)]
struct LegacyLimits {
    limits: Vec<ClientMonthlyLimit>,
}

/// Whether a rule is a device's plain monthly limit (spec 0169, SPEC-RULE-009).
pub fn is_monthly_limit(rule: &MeterRule) -> bool {
    rule.group_id.is_none()
        && rule.cycle == MeterCycle::Monthly { day: 1 }
        && rule.allocation_bytes > 0
        && rule.countdown_ms.is_none()
        && rule.schedule.is_none()
}

/// A rule with its advancing counters blanked, to tell a rule that moved from
/// one whose device merely sent traffic.
fn without_counters(rules: &[MeterRule]) -> Vec<MeterRule> {
    rules
        .iter()
        .map(|rule| MeterRule {
            observed_rx: 0,
            observed_tx: 0,
            ..rule.clone()
        })
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Atomic, like every other recorder file.
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(value).map_err(io::Error::other)?,
    )?;
    fs::rename(temporary, path)
}

pub struct MeterStore {
    meters_path: PathBuf,
    groups_path: PathBuf,
    rules: Vec<MeterRule>,
    groups: Vec<DeviceGroup>,
    pauses: Vec<Value>,
    counters_written_ms: i64,
}

impl MeterStore {
    /// Open both files in `data_dir`. An unreadable or newer snapshot starts
    /// empty rather than refusing to boot, as the TypeScript stores do.
    pub fn open(data_dir: &Path) -> Self {
        let meters_path = data_dir.join("meters.json");
        let groups_path = data_dir.join("device-groups.json");
        let meters = read_json::<MetersSnapshot>(&meters_path)
            .filter(|snapshot| snapshot.version <= METERS_VERSION);
        let groups = read_json::<GroupsSnapshot>(&groups_path)
            .filter(|snapshot| snapshot.version == GROUPS_VERSION);
        let (rules, pauses) = meters.map_or_else(Default::default, |snapshot| {
            (snapshot.rules, snapshot.pauses)
        });
        Self {
            meters_path,
            groups_path,
            rules,
            groups: groups.map_or_else(Vec::new, |snapshot| snapshot.groups),
            pauses,
            counters_written_ms: 0,
        }
    }

    pub fn rules(&self) -> &[MeterRule] {
        &self.rules
    }

    pub fn groups(&self) -> &[DeviceGroup] {
        &self.groups
    }

    /// Write the new state, then adopt it. A failed write leaves memory as it
    /// was, so nothing unsaved is ever shown as saved.
    fn commit(
        &mut self,
        rules: Option<Vec<MeterRule>>,
        groups: Option<Vec<DeviceGroup>>,
    ) -> io::Result<()> {
        if let Some(rules) = &rules {
            write_json(
                &self.meters_path,
                &MetersSnapshot {
                    version: METERS_VERSION,
                    rules: rules.clone(),
                    pauses: self.pauses.clone(),
                },
            )?;
        }
        if let Some(groups) = &groups {
            write_json(
                &self.groups_path,
                &GroupsSnapshot {
                    version: GROUPS_VERSION,
                    groups: groups.clone(),
                },
            )?;
        }
        if let Some(rules) = rules {
            self.rules = rules;
        }
        if let Some(groups) = groups {
            self.groups = groups;
        }
        Ok(())
    }

    /// The rules with a device's own rule put in place.
    fn with_device_rule(&self, rule: MeterRule) -> Vec<MeterRule> {
        let key = rule_key(&rule.client_key, None);
        let mut rules = self.rules.clone();
        match rules
            .iter_mut()
            .find(|held| rule_key(&held.client_key, held.group_id.as_deref()) == key)
        {
            Some(held) => *held = rule,
            None => rules.push(rule),
        }
        rules
    }

    /// A device's monthly-limit rule, anchored at the start of the month.
    fn monthly_limit_rule(
        &self,
        limit: &ClientMonthlyLimit,
        reading: Option<&LifetimeReading>,
        now_ms: i64,
    ) -> MeterRule {
        let (lifetime_rx, lifetime_tx) =
            reading.map_or((0, 0), |reading| (reading.lifetime_rx, reading.lifetime_tx));
        let terms = MeterRuleTerms::new(
            limit.client_key.clone(),
            limit.limit_bytes,
            MeterCycle::Monthly { day: 1 },
            lifetime_rx,
            lifetime_tx,
            now_ms,
        );
        let existing = self
            .rules
            .iter()
            .find(|rule| rule.group_id.is_none() && rule.client_key == limit.client_key);
        if let Some(existing) = existing.filter(|rule| is_monthly_limit(rule)) {
            return upsert_rule(Some(existing), terms, None);
        }
        let mut rule = create_rule(terms);
        if let Some(reading) = reading {
            rule.anchor_rx = reading.lifetime_rx.saturating_sub(reading.month_rx);
            rule.anchor_tx = reading.lifetime_tx.saturating_sub(reading.month_tx);
        }
        rule
    }

    /// Convert `monthly-limits.json` into monthly rules anchored at the start of
    /// the month, so each keeps the month's usage. The file is renamed aside so
    /// this happens once. Returns whether anything migrated.
    pub fn migrate_monthly_limits(
        &mut self,
        limits_path: &Path,
        readings: &[LifetimeReading],
        now_ms: i64,
    ) -> io::Result<bool> {
        if !limits_path.exists() {
            return Ok(false);
        }
        let legacy =
            read_json::<LegacyLimits>(limits_path).map_or_else(Vec::new, |file| file.limits);
        let mut staged = Self {
            meters_path: self.meters_path.clone(),
            groups_path: self.groups_path.clone(),
            rules: self.rules.clone(),
            groups: Vec::new(),
            pauses: Vec::new(),
            counters_written_ms: 0,
        };
        for limit in &legacy {
            let reading = readings
                .iter()
                .find(|reading| reading.client_key == limit.client_key);
            let rule = staged.monthly_limit_rule(limit, reading, now_ms);
            staged.rules = staged.with_device_rule(rule);
        }
        self.commit(Some(staged.rules), None)?;
        fs::rename(limits_path, limits_path.with_extension("json.migrated"))?;
        Ok(true)
    }

    /// The monthly limits the rules describe, for the Data Usage page.
    pub fn monthly_limits(&self) -> Vec<ClientMonthlyLimit> {
        self.rules
            .iter()
            .filter(|rule| is_monthly_limit(rule))
            .filter_map(|rule| {
                ClientMonthlyLimit::new(rule.client_key.clone(), rule.allocation_bytes).ok()
            })
            .collect()
    }

    /// Set a device's monthly limit as its own monthly rule, anchored at the
    /// start of the month so the cap covers the whole month (spec 0160).
    pub fn set_monthly_limit(
        &mut self,
        limit: &ClientMonthlyLimit,
        reading: Option<&LifetimeReading>,
        now_ms: i64,
    ) -> io::Result<()> {
        let rule = self.monthly_limit_rule(limit, reading, now_ms);
        let rules = self.with_device_rule(rule);
        self.commit(Some(rules), None)
    }

    /// Write a device's own rule, keeping the anchors of one it already has.
    pub fn upsert_device_rule(&mut self, terms: MeterRuleTerms) -> io::Result<()> {
        let terms = MeterRuleTerms {
            group_id: None,
            ..terms
        };
        let existing = self
            .rules
            .iter()
            .find(|rule| rule.group_id.is_none() && rule.client_key == terms.client_key);
        let rule = upsert_rule(existing, terms, None);
        let rules = self.with_device_rule(rule);
        self.commit(Some(rules), None)
    }

    /// Remove a device's own rule. Its group rules stay.
    pub fn remove_device_rule(&mut self, client_key: &str) -> io::Result<bool> {
        let mut rules = self.rules.clone();
        rules.retain(|rule| rule.group_id.is_some() || rule.client_key != client_key);
        let removed = rules.len() != self.rules.len();
        if removed {
            self.commit(Some(rules), None)?;
        }
        Ok(removed)
    }

    /// Add or replace a group and project its members' rules now.
    pub fn upsert_group(
        &mut self,
        group: DeviceGroup,
        readings: &[MeterReading],
        now_ms: i64,
    ) -> io::Result<()> {
        let mut groups = self.groups.clone();
        match groups
            .iter_mut()
            .find(|held| held.group_id == group.group_id)
        {
            Some(held) => *held = group,
            None => groups.push(group),
        }
        let rules = project_group_rules(&groups, &self.rules, readings, now_ms);
        self.commit(Some(rules), Some(groups))
    }

    pub fn remove_group(&mut self, group_id: &str) -> io::Result<bool> {
        let mut groups = self.groups.clone();
        groups.retain(|group| group.group_id != group_id);
        if groups.len() == self.groups.len() {
            return Ok(false);
        }
        let mut rules = self.rules.clone();
        rules.retain(|rule| rule.group_id.as_deref() != Some(group_id));
        self.commit(Some(rules), Some(groups))?;
        Ok(true)
    }

    /// Start one rule over from now.
    pub fn restart(
        &mut self,
        client_key: &str,
        group_id: Option<&str>,
        now_ms: i64,
    ) -> io::Result<bool> {
        let key = rule_key(client_key, group_id);
        let mut rules = self.rules.clone();
        let Some(rule) = rules
            .iter_mut()
            .find(|rule| rule_key(&rule.client_key, rule.group_id.as_deref()) == key)
        else {
            return Ok(false);
        };
        *rule = restart_cycle(rule, now_ms);
        self.commit(Some(rules), None)?;
        Ok(true)
    }

    /// Forget a device: its own rule, and its place in every group.
    pub fn remove_device(&mut self, client_key: &str) -> io::Result<bool> {
        let mut rules = self.rules.clone();
        rules.retain(|rule| rule.client_key != client_key);
        let mut groups = self.groups.clone();
        for group in &mut groups {
            group.member_keys.retain(|member| member != client_key);
        }
        groups.retain(|group| !group.member_keys.is_empty());
        let rules_changed = rules.len() != self.rules.len();
        let groups_changed = groups != self.groups;
        self.commit(
            rules_changed.then_some(rules),
            groups_changed.then_some(groups),
        )?;
        Ok(rules_changed || groups_changed)
    }

    pub fn clear(&mut self) -> io::Result<()> {
        let pauses = std::mem::take(&mut self.pauses);
        let cleared = self.commit(Some(Vec::new()), Some(Vec::new()));
        if cleared.is_err() {
            self.pauses = pauses;
        }
        cleared
    }

    /// Fold one poll's readings into the rules: follow identity merges,
    /// project groups, evaluate, and persist when a rule moved. Counters alone
    /// are flushed at most every 30 s.
    pub fn evaluate(
        &mut self,
        readings: &[MeterReading],
        resolve_key: impl Fn(&str) -> String,
        now_ms: i64,
    ) -> io::Result<Vec<MeterTransition>> {
        let readings: Vec<MeterReading> = readings
            .iter()
            .map(|reading| MeterReading {
                client_key: resolve_key(&reading.client_key),
                ..reading.clone()
            })
            .collect();
        let groups = resolve_group_members(&self.groups, &resolve_key);
        let groups_changed = groups != self.groups;
        let resolved = resolve_rule_keys(&self.rules, &resolve_key);
        let projected = project_group_rules(&groups, &resolved, &readings, now_ms);
        let (next, transitions) = evaluate_meters(&projected, &readings, now_ms);
        let moved = without_counters(&next) != without_counters(&self.rules);
        let counted = next != self.rules;
        let flush = moved || (counted && now_ms - self.counters_written_ms >= COUNTER_FLUSH_MS);
        if flush || groups_changed {
            self.commit(
                flush.then_some(next.clone()),
                groups_changed.then_some(groups),
            )?;
        }
        if flush {
            self.counters_written_ms = now_ms;
        } else {
            // Counters advance in memory between flushes; the odometer holds
            // them durably, so a restart loses nothing.
            self.rules = next;
        }
        Ok(collapse_group_announcements(transitions))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::data_meter::tests::{GB, local_ms};
    use crate::data_meter::{MeterTransitionKind, usage_bytes};
    use crate::device_group::GroupAllowanceMode;

    fn now() -> i64 {
        local_ms(2026, 8, 12, 15, 30)
    }

    fn reading(key: &str, lifetime: u64, month: u64) -> LifetimeReading {
        LifetimeReading {
            client_key: key.into(),
            lifetime_rx: lifetime,
            lifetime_tx: 0,
            month_rx: month,
            month_tx: 0,
        }
    }

    const TS_METERS: &str = r#"{
      "version": 2,
      "rules": [{
        "clientKey": "42", "allocationBytes": 5000000000, "autoPause": true,
        "cycle": {"kind": "once"}, "anchorRx": 10, "anchorTx": 0,
        "observedRx": 20, "observedTx": 0, "periodStartMs": 1, "periodEndMs": null,
        "actedThisCycle": false, "createdMs": 1
      }],
      "pauses": [{"clientKey": "42", "state": "applied", "checkedMs": 5}]
    }"#;

    // SPEC-RULE-007: a TypeScript-written snapshot reads back, its pauses and
    // autoPause are preserved untouched, and a newer version starts empty.
    #[test]
    fn typescript_snapshots_round_trip_with_pauses_preserved() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("meters.json"), TS_METERS).unwrap();
        let mut store = MeterStore::open(directory.path());
        assert_eq!(store.rules().len(), 1);
        assert_eq!(store.rules()[0].period_end_ms, None);
        assert!(store.rules()[0].auto_pause);
        assert_eq!(usage_bytes(&store.rules()[0]), 10);

        store
            .upsert_device_rule(MeterRuleTerms::new("7", GB, MeterCycle::Daily, 0, 0, now()))
            .unwrap();
        let written: Value = serde_json::from_str(
            &fs::read_to_string(directory.path().join("meters.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(written["version"], 2);
        assert_eq!(written["pauses"][0]["state"], "applied");
        assert_eq!(written["rules"][0]["autoPause"], true);
        assert_eq!(written["rules"][1]["autoPause"], false);
        assert!(written["rules"][0]["periodEndMs"].is_null());
        assert!(!directory.path().join("meters.json.tmp").exists());

        let reopened = MeterStore::open(directory.path());
        assert_eq!(reopened.rules(), store.rules());

        fs::write(
            directory.path().join("meters.json"),
            r#"{"version": 9, "rules": []}"#,
        )
        .unwrap();
        assert!(MeterStore::open(directory.path()).rules().is_empty());
        fs::write(directory.path().join("meters.json"), "not json").unwrap();
        assert!(MeterStore::open(directory.path()).rules().is_empty());
    }

    // SPEC-RULE-007, SPEC-RULE-009: monthly limits migrate once into monthly rules that
    // keep the month's usage, and read back as limits.
    #[test]
    fn monthly_limits_migrate_once_and_keep_the_month() {
        let directory = tempdir().unwrap();
        let limits_path = directory.path().join("monthly-limits.json");
        fs::write(
            &limits_path,
            r#"{"version":1,"limits":[{"clientKey":"42","limitBytes":3000000000}]}"#,
        )
        .unwrap();
        let mut store = MeterStore::open(directory.path());
        let readings = [reading("42", 50 * GB, 2 * GB)];
        assert!(
            store
                .migrate_monthly_limits(&limits_path, &readings, now())
                .unwrap()
        );
        assert!(!limits_path.exists());
        assert!(
            !store
                .migrate_monthly_limits(&limits_path, &readings, now())
                .unwrap()
        );

        let rule = &store.rules()[0];
        assert_eq!(rule.cycle, MeterCycle::Monthly { day: 1 });
        assert!(!rule.auto_pause);
        assert_eq!(usage_bytes(rule), 2 * GB, "the month's usage carries over");
        assert!(is_monthly_limit(rule));
        assert_eq!(
            store.monthly_limits(),
            [ClientMonthlyLimit::new("42", 3 * GB).unwrap()]
        );

        store
            .set_monthly_limit(
                &ClientMonthlyLimit::new("7", GB).unwrap(),
                Some(&reading("7", 9 * GB, GB)),
                now(),
            )
            .unwrap();
        let seven = store
            .rules()
            .iter()
            .find(|rule| rule.client_key == "7")
            .unwrap();
        assert_eq!(usage_bytes(seven), GB);
        assert!(store.remove_device_rule("7").unwrap());
        assert_eq!(store.monthly_limits().len(), 1);

        let reopened = MeterStore::open(directory.path());
        assert_eq!(reopened.monthly_limits().len(), 1);
    }

    // SPEC-RULE-007: a failed write reports an error and leaves the rules and
    // groups as they were, so nothing unsaved is shown as saved.
    #[test]
    fn a_failed_write_changes_nothing() {
        let directory = tempdir().unwrap();
        let mut store = MeterStore::open(directory.path());
        fs::create_dir(directory.path().join("meters.json.tmp")).unwrap();
        fs::create_dir(directory.path().join("device-groups.json.tmp")).unwrap();
        assert!(
            store
                .upsert_device_rule(MeterRuleTerms::new("7", GB, MeterCycle::Daily, 0, 0, now()))
                .is_err()
        );
        assert!(
            store
                .set_monthly_limit(&ClientMonthlyLimit::new("7", GB).unwrap(), None, now())
                .is_err()
        );
        assert!(store.upsert_group(kids(&["1"]), &[], now()).is_err());
        assert!(store.rules().is_empty());
        assert!(store.groups().is_empty());
    }

    fn kids(members: &[&str]) -> DeviceGroup {
        DeviceGroup {
            group_id: "group-kids".into(),
            name: "Kids".into(),
            member_keys: members.iter().map(|key| (*key).into()).collect(),
            allocation_bytes: 10 * GB,
            auto_pause: false,
            cycle: MeterCycle::Monthly { day: 1 },
            mode: GroupAllowanceMode::Pooled,
            countdown_ms: None,
            schedule: None,
            updated_ms: now(),
            created_ms: now(),
        }
    }

    // SPEC-RULE-006, SPEC-RULE-008: groups persist and project; evaluation follows
    // merges, announces once per group, and removing a device takes it out of
    // its groups. Nothing here writes to a device.
    #[test]
    fn groups_persist_project_and_evaluate() {
        let directory = tempdir().unwrap();
        let mut store = MeterStore::open(directory.path());
        let zero = [
            MeterReading {
                client_key: "1".into(),
                lifetime_rx: 0,
                lifetime_tx: 0,
            },
            MeterReading {
                client_key: "2".into(),
                lifetime_rx: 0,
                lifetime_tx: 0,
            },
        ];
        store.upsert_group(kids(&["1", "2"]), &zero, now()).unwrap();
        assert_eq!(store.rules().len(), 2);
        assert!(store.rules().iter().all(|rule| !is_monthly_limit(rule)));
        assert_eq!(MeterStore::open(directory.path()).groups().len(), 1);

        let over = [
            MeterReading {
                client_key: "1".into(),
                lifetime_rx: 6 * GB,
                lifetime_tx: 0,
            },
            MeterReading {
                client_key: "old-2".into(),
                lifetime_rx: 6 * GB,
                lifetime_tx: 0,
            },
        ];
        let resolve = |key: &str| {
            if key == "old-2" {
                "2".to_string()
            } else {
                key.to_string()
            }
        };
        let transitions = store.evaluate(&over, resolve, now() + 1_000).unwrap();
        assert_eq!(transitions.len(), 1, "a pooled group announces once");
        assert_eq!(transitions[0].kind, MeterTransitionKind::Reached);
        assert!(
            store
                .evaluate(&over, resolve, now() + 2_000)
                .unwrap()
                .is_empty()
        );

        assert!(
            store
                .restart("1", Some("group-kids"), now() + 3_000)
                .unwrap()
        );
        let restarted = store
            .rules()
            .iter()
            .find(|rule| rule.client_key == "1")
            .unwrap();
        assert_eq!(usage_bytes(restarted), 0);

        assert!(store.remove_device("2").unwrap());
        assert_eq!(store.groups()[0].member_keys, ["1"]);
        assert!(store.rules().iter().all(|rule| rule.client_key != "2"));

        assert!(store.remove_group("group-kids").unwrap());
        assert!(store.rules().is_empty());
        store.clear().unwrap();
        assert!(MeterStore::open(directory.path()).groups().is_empty());
    }
}
