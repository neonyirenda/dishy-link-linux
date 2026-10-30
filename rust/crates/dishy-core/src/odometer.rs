//! Per-device data-usage odometer, bucketed by billing month.
//!
//! Mirrors `core/clientTotals.ts` and `collector/clientTotals.mts`.
//! The router resets client counters on each Wi-Fi re-association. This odometer
//! accumulates counters into authoritative cumulative bytes, handles Wi-Fi
//! counter resets, skips unobserved gaps, rolls billing months, and re-anchors
//! devices across router factory resets when MACs are unique.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// On-disk snapshot schema version.
pub const VERSION: u32 = 4;

/// Completed months kept alongside the current running month.
pub const MONTHS_KEPT: usize = 6;

/// Gaps wider than this (15 seconds) represent unobserved spans and force a re-baseline.
pub const DEFAULT_MAX_GAP_MS: i64 = 15_000;

/// 2.5 Gbps link ceiling in bytes per millisecond: 312,500 bytes/ms.
pub const MAX_BYTES_PER_MS: u64 = 312_500;

/// A completed month total: `year * 12 + month` (0-indexed month) and bytes transferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthTotal {
    pub period_month: i32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// One proposed merge: `from_key`'s record is believed to be the same device as
/// `to_key`'s, because they carry the same name (spec 0176). A proposal only;
/// nothing folds until the user says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeCandidate {
    pub from_key: String,
    pub to_key: String,
    /// The shared name both records carry.
    pub detail: String,
    /// Whether accepting adds the month's bytes: only when both records cover
    /// the same month.
    pub folds_bytes: bool,
    pub result_rx_bytes: u64,
    pub result_tx_bytes: u64,
}

/// How long a record must have gone unseen to be a merge source: anything
/// still being polled is a different device.
pub const MERGE_IDLE_AFTER_MS: i64 = 10 * 60_000;

/// Public total for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTotal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<i64>,
    pub mac_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub since_ms: i64,
    pub last_seen_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub months: Option<Vec<MonthTotal>>,
}

/// Internal state for one tracked device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<i64>,
    pub mac_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captive_client_id: Option<String>,
    pub lifetime_rx: u64,
    pub lifetime_tx: u64,
    pub month_anchor_rx: u64,
    pub month_anchor_tx: u64,
    pub since_ms: i64,
    pub last_seen_ms: i64,
    pub period_month: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub months: Option<Vec<MonthTotal>>,
    pub prev_rx: u64,
    pub prev_tx: u64,
    pub last_poll_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_traffic_debt_rx: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_traffic_debt_tx: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrected_rx: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrected_tx: Option<u64>,
}

/// Snapshot persisted to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: u32,
    pub totals: Vec<TotalState>,
    pub shared_macs: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<(String, String)>,
    #[serde(default)]
    pub user_merged: Vec<String>,
    #[serde(default)]
    pub rejected_pairs: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotV3 {
    version: u32,
    totals: Vec<TotalStateV3>,
    shared_macs: Vec<String>,
    #[serde(default)]
    aliases: Vec<(String, String)>,
    #[serde(default)]
    rejected_pairs: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TotalStateV3 {
    #[serde(default)]
    client_id: Option<i64>,
    mac_address: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    ip_address: Option<String>,
    #[serde(default)]
    captive_client_id: Option<String>,
    rx_bytes: u64,
    tx_bytes: u64,
    since_ms: i64,
    last_seen_ms: i64,
    period_month: i32,
    #[serde(default)]
    months: Option<Vec<MonthTotal>>,
    prev_rx: u64,
    prev_tx: u64,
    last_poll_ms: i64,
    #[serde(default)]
    self_traffic_debt_rx: Option<u64>,
    #[serde(default)]
    self_traffic_debt_tx: Option<u64>,
    #[serde(default)]
    corrected_rx: Option<u64>,
    #[serde(default)]
    corrected_tx: Option<u64>,
}

/// One router client counter sample used by the odometer.
pub struct ClientReading<'a> {
    pub client_id: Option<i64>,
    pub mac_address: &'a str,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub at_ms: i64,
    pub name: Option<&'a str>,
    pub ip_address: Option<&'a str>,
}

/// Map key for a client device: stringified `clientId` when available, else MAC.
// SPEC-ODO-001: keying by clientId, falling back to MAC.
pub fn key_of(client_id: Option<i64>, mac_address: &str) -> String {
    match client_id {
        Some(id) => id.to_string(),
        None => mac_address.to_string(),
    }
}

/// Computes the calendar month key `year * 12 + month_0_indexed`.
pub fn month_of(at_ms: i64, utc: bool) -> i32 {
    if utc {
        let dt = Utc.timestamp_millis_opt(at_ms).unwrap();
        dt.year() * 12 + (dt.month0() as i32)
    } else {
        let dt = Local.timestamp_millis_opt(at_ms).unwrap();
        dt.year() * 12 + (dt.month0() as i32)
    }
}

/// Computes the start of the month in milliseconds (epoch ms).
pub fn month_start_ms(at_ms: i64, utc: bool) -> i64 {
    if utc {
        let dt = Utc.timestamp_millis_opt(at_ms).unwrap();
        Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
    } else {
        let dt = Local.timestamp_millis_opt(at_ms).unwrap();
        Local
            .with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
    }
}

/// One counter's contribution since the last reading.
// SPEC-ODO-002: counter advance and reset within ceiling bound.
pub fn advance(counter: u64, previous: u64, ceiling_bytes: u64) -> u64 {
    if counter >= previous {
        counter - previous
    } else if counter <= ceiling_bytes {
        counter
    } else {
        0
    }
}

fn monthly_rx(state: &TotalState) -> u64 {
    state.lifetime_rx.saturating_sub(state.month_anchor_rx)
}

fn monthly_tx(state: &TotalState) -> u64 {
    state.lifetime_tx.saturating_sub(state.month_anchor_tx)
}

fn rebase_self_traffic(state: &mut TotalState, rx_bytes: u64, tx_bytes: u64) {
    state.self_traffic_debt_rx = Some(0);
    state.self_traffic_debt_tx = Some(0);
    state.corrected_rx = Some(rx_bytes);
    state.corrected_tx = Some(tx_bytes);
}

/// The pure odometer core tracking client byte accumulators.
#[derive(Debug, Clone)]
pub struct ClientTotalsCore {
    states: HashMap<String, TotalState>,
    shared_macs: HashSet<String>,
    aliases: HashMap<String, String>,
    user_merged: HashSet<String>,
    rejected_pairs: HashMap<String, (String, String)>,
    max_gap_ms: i64,
    utc: bool,
}

impl Default for ClientTotalsCore {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientTotalsCore {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            shared_macs: HashSet::new(),
            aliases: HashMap::new(),
            user_merged: HashSet::new(),
            rejected_pairs: HashMap::new(),
            max_gap_ms: DEFAULT_MAX_GAP_MS,
            utc: false,
        }
    }

    /// Construct with custom max gap and timezone mode (used by UTC tests).
    pub fn with_options(max_gap_ms: i64, utc: bool) -> Self {
        Self {
            states: HashMap::new(),
            shared_macs: HashSet::new(),
            aliases: HashMap::new(),
            user_merged: HashSet::new(),
            rejected_pairs: HashMap::new(),
            max_gap_ms,
            utc,
        }
    }

    pub fn load_snapshot(&mut self, snapshot: Snapshot) {
        self.states.clear();
        self.shared_macs.clear();
        self.aliases.clear();
        self.user_merged.clear();
        self.rejected_pairs.clear();

        for state in snapshot.totals {
            let key = key_of(state.client_id, &state.mac_address);
            self.states.insert(key, state);
        }
        for mac in snapshot.shared_macs {
            self.shared_macs.insert(mac);
        }
        for (from, to) in snapshot.aliases {
            self.aliases.insert(from, to);
        }
        for from in snapshot.user_merged {
            self.user_merged.insert(from);
        }
        for (a, b) in snapshot.rejected_pairs {
            let key = pair_key(&a, &b);
            self.rejected_pairs.insert(key, (a, b));
        }
    }

    pub fn to_snapshot(&self) -> Snapshot {
        let mut totals: Vec<TotalState> = self.states.values().cloned().collect();
        totals.sort_by(|a, b| {
            key_of(a.client_id, &a.mac_address).cmp(&key_of(b.client_id, &b.mac_address))
        });

        let mut shared_macs: Vec<String> = self.shared_macs.iter().cloned().collect();
        shared_macs.sort();

        let mut aliases: Vec<(String, String)> = self
            .aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        aliases.sort();

        let mut user_merged: Vec<String> = self.user_merged.iter().cloned().collect();
        user_merged.sort();

        let mut rejected_pairs: Vec<(String, String)> =
            self.rejected_pairs.values().cloned().collect();
        rejected_pairs.sort();

        Snapshot {
            version: VERSION,
            totals,
            shared_macs,
            aliases,
            user_merged,
            rejected_pairs,
        }
    }

    pub fn resolve_key(&self, client_key: &str) -> String {
        let mut key = client_key.to_string();
        for _ in 0..32 {
            match self.aliases.get(&key) {
                Some(next) if next != &key => key = next.clone(),
                _ => break,
            }
        }
        key
    }

    /// Record the live poll roster and identify shared OUIs.
    // SPEC-ODO-004: OUI observed on multiple devices flagged permanently as shared.
    pub fn note_poll(&mut self, entries: &[(Option<i64>, &str)]) -> HashSet<String> {
        let mut live_keys = HashSet::new();
        let mut live_by_mac: HashMap<&str, usize> = HashMap::new();

        for &(client_id, mac) in entries {
            live_keys.insert(key_of(client_id, mac));
            *live_by_mac.entry(mac).or_insert(0) += 1;
        }

        for (mac, count) in live_by_mac {
            if count > 1 && !self.shared_macs.contains(mac) {
                self.shared_macs.insert(mac.to_string());
                self.states.remove(&key_of(None, mac));
            }
        }

        live_keys
    }

    /// Fold one client counter reading into the odometer.
    // SPEC-ODO-001, SPEC-ODO-002, SPEC-ODO-003, SPEC-ODO-004: observation and accumulation.
    pub fn observe(&mut self, reading: ClientReading<'_>, live_keys: &HashSet<String>) {
        let ClientReading {
            client_id,
            mac_address,
            rx_bytes,
            tx_bytes,
            at_ms,
            name,
            ip_address,
        } = reading;
        let raw = key_of(client_id, mac_address);
        let key = if self.user_merged.contains(&raw) {
            self.resolve_key(&raw)
        } else {
            raw
        };

        let month = month_of(at_ms, self.utc);

        if !self.states.contains_key(&key) {
            self.adopt_or_create(client_id, mac_address, at_ms, name, live_keys, &key);
        }

        let state = self.states.get_mut(&key).expect("state exists");

        // SPEC-ODO-003: lazy monthly boundary roll upon first sighting in a new month.
        if month != state.period_month {
            let closed = MonthTotal {
                period_month: state.period_month,
                rx_bytes: monthly_rx(state),
                tx_bytes: monthly_tx(state),
            };
            if closed.rx_bytes > 0 || closed.tx_bytes > 0 {
                let mut months = state.months.take().unwrap_or_default();
                months.push(closed);
                if months.len() > MONTHS_KEPT {
                    let drain_count = months.len() - MONTHS_KEPT;
                    months.drain(0..drain_count);
                }
                state.months = Some(months);
            }
            state.month_anchor_rx = state.lifetime_rx;
            state.month_anchor_tx = state.lifetime_tx;
            state.period_month = month;
            state.since_ms = month_start_ms(at_ms, self.utc);
            rebase_self_traffic(state, rx_bytes, tx_bytes);
        } else if state.last_poll_ms != 0 && (at_ms - state.last_poll_ms) <= self.max_gap_ms {
            // SPEC-ODO-002: normal advance or reset within ceiling.
            let elapsed_ms = (at_ms - state.last_poll_ms).max(0) as u64;
            let ceiling = elapsed_ms.saturating_mul(MAX_BYTES_PER_MS);
            let rx_advance = advance(rx_bytes, state.prev_rx, ceiling);
            let tx_advance = advance(tx_bytes, state.prev_tx, ceiling);
            state.lifetime_rx = state.lifetime_rx.saturating_add(rx_advance);
            state.lifetime_tx = state.lifetime_tx.saturating_add(tx_advance);
            state.corrected_rx = Some(if rx_bytes < state.prev_rx {
                rx_advance
            } else {
                state.corrected_rx.unwrap_or(rx_bytes) + rx_advance
            });
            state.corrected_tx = Some(if tx_bytes < state.prev_tx {
                tx_advance
            } else {
                state.corrected_tx.unwrap_or(tx_bytes) + tx_advance
            });
        } else {
            rebase_self_traffic(state, rx_bytes, tx_bytes);
        }

        state.prev_rx = rx_bytes;
        state.prev_tx = tx_bytes;
        state.last_poll_ms = at_ms;
        state.last_seen_ms = at_ms;
        if let Some(n) = name
            && !n.is_empty()
        {
            state.name = Some(n.to_string());
        }
        if let Some(ip) = ip_address
            && !ip.is_empty()
        {
            state.ip_address = Some(ip.to_string());
        }
    }

    /// Re-anchors to an orphaned bucket when the MAC is uniquely held and unshared,
    /// or creates a fresh bucket.
    // SPEC-ODO-004: adoption of orphan bucket when MAC is unique.
    fn adopt_or_create(
        &mut self,
        client_id: Option<i64>,
        mac_address: &str,
        at_ms: i64,
        name: Option<&str>,
        live_keys: &HashSet<String>,
        key: &str,
    ) {
        if client_id.is_some() && !self.shared_macs.contains(mac_address) {
            let orphans: Vec<String> = self
                .states
                .iter()
                .filter(|(k, s)| s.mac_address == mac_address && k.as_str() != key)
                .map(|(k, _)| k.clone())
                .collect();

            if orphans.len() == 1 {
                let orphan_key = &orphans[0];
                let orphan_live = live_keys.contains(orphan_key);
                if !orphan_live {
                    let mut orphan = self.states.remove(orphan_key).expect("orphan state");
                    orphan.client_id = client_id;
                    orphan.mac_address = mac_address.to_string();
                    orphan.last_poll_ms = 0;
                    if let Some(n) = name
                        && !n.is_empty()
                    {
                        orphan.name = Some(n.to_string());
                    }
                    self.states.insert(key.to_string(), orphan);
                    if orphan_key != key {
                        for target in self.aliases.values_mut() {
                            if target == orphan_key {
                                *target = key.to_string();
                            }
                        }
                        self.aliases.insert(orphan_key.to_string(), key.to_string());
                    }
                    return;
                }
            }
        }

        let fresh = TotalState {
            client_id,
            mac_address: mac_address.to_string(),
            name: name.map(str::to_string),
            ip_address: None,
            captive_client_id: None,
            lifetime_rx: 0,
            lifetime_tx: 0,
            month_anchor_rx: 0,
            month_anchor_tx: 0,
            since_ms: month_start_ms(at_ms, self.utc),
            last_seen_ms: at_ms,
            period_month: month_of(at_ms, self.utc),
            months: None,
            prev_rx: 0,
            prev_tx: 0,
            last_poll_ms: 0,
            self_traffic_debt_rx: None,
            self_traffic_debt_tx: None,
            corrected_rx: None,
            corrected_tx: None,
        };
        self.states.insert(key.to_string(), fresh);
    }

    /// Reset one device's current monthly usage to zero by advancing the anchor.
    pub fn reset(&mut self, client_key: &str, at_ms: i64) -> bool {
        let resolved = self.resolve_key(client_key);
        if let Some(state) = self.states.get_mut(&resolved) {
            state.month_anchor_rx = state.lifetime_rx;
            state.month_anchor_tx = state.lifetime_tx;
            state.since_ms = at_ms;
            true
        } else {
            false
        }
    }

    /// Remove a device's record entirely.
    pub fn remove(&mut self, client_key: &str) -> bool {
        let resolved = self.resolve_key(client_key);
        self.states.remove(&resolved).is_some()
    }

    /// Clear all device records.
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Return public usage totals, sorted by newest activity first.
    /// Every device's lifetime counter and its current-month share, for the
    /// metering rules (spec 0169).
    pub fn lifetime_readings(&self) -> Vec<crate::meter_store::LifetimeReading> {
        let mut readings: Vec<_> = self
            .states
            .iter()
            .map(|(key, state)| crate::meter_store::LifetimeReading {
                client_key: key.clone(),
                lifetime_rx: state.lifetime_rx,
                lifetime_tx: state.lifetime_tx,
                month_rx: monthly_rx(state),
                month_tx: monthly_tx(state),
            })
            .collect();
        readings.sort_by(|a, b| a.client_key.cmp(&b.client_key));
        readings
    }

    /// Fold `from_key`'s record into `to_key`'s, as one device whose id the
    /// router reissued. Bytes add only when both cover the same month; across a
    /// boundary the identity joins and the month stands. Returns false for an
    /// unknown key or two keys of one record.
    pub fn merge(&mut self, from_key: &str, to_key: &str) -> bool {
        let at = |core: &Self, key: &str| {
            if core.states.contains_key(key) {
                key.to_owned()
            } else {
                core.resolve_key(key)
            }
        };
        let (from, to) = (at(self, from_key), at(self, to_key));
        if from == to || !self.states.contains_key(&from) || !self.states.contains_key(&to) {
            return false;
        }
        // A merge is the later answer about the pair, so it retires a "no".
        let wanted = pair_key(&from, &to);
        let stale: Vec<String> = self
            .rejected_pairs
            .iter()
            .filter(|(_, (a, b))| pair_key(&self.resolve_key(a), &self.resolve_key(b)) == wanted)
            .map(|(name, _)| name.clone())
            .collect();
        for name in stale {
            self.rejected_pairs.remove(&name);
        }
        let source = self.states.remove(&from).expect("checked above");
        let survivor = self.states.get_mut(&to).expect("checked above");
        survivor.lifetime_rx += source.lifetime_rx;
        survivor.lifetime_tx += source.lifetime_tx;
        if source.period_month == survivor.period_month {
            survivor.month_anchor_rx += source.month_anchor_rx;
            survivor.month_anchor_tx += source.month_anchor_tx;
            survivor.since_ms = survivor.since_ms.min(source.since_ms);
        } else {
            survivor.month_anchor_rx += source.lifetime_rx;
            survivor.month_anchor_tx += source.lifetime_tx;
        }
        let mut months: std::collections::BTreeMap<i32, MonthTotal> =
            std::collections::BTreeMap::new();
        for month in survivor
            .months
            .iter()
            .flatten()
            .chain(source.months.iter().flatten())
        {
            let held = months.entry(month.period_month).or_insert(MonthTotal {
                period_month: month.period_month,
                rx_bytes: 0,
                tx_bytes: 0,
            });
            held.rx_bytes += month.rx_bytes;
            held.tx_bytes += month.tx_bytes;
        }
        if !months.is_empty() {
            let merged: Vec<MonthTotal> = months.into_values().collect();
            let skip = merged.len().saturating_sub(MONTHS_KEPT);
            survivor.months = Some(merged.into_iter().skip(skip).collect());
        }
        survivor.last_seen_ms = survivor.last_seen_ms.max(source.last_seen_ms);
        if survivor.name.is_none() {
            survivor.name = source.name;
        }
        // The next reading re-baselines: a delta against these counters would
        // span the whole time the source was silent.
        survivor.last_poll_ms = 0;
        for target in self.aliases.values_mut() {
            if *target == from {
                *target = to.clone();
            }
        }
        self.aliases.insert(from.clone(), to);
        self.user_merged.insert(from);
        true
    }

    /// Remember that two records are different devices, so they are not
    /// proposed again. Order does not matter.
    pub fn reject_merge(&mut self, a_key: &str, b_key: &str) -> bool {
        let (a, b) = (self.resolve_key(a_key), self.resolve_key(b_key));
        if a == b || !self.states.contains_key(&a) || !self.states.contains_key(&b) {
            return false;
        }
        self.rejected_pairs.insert(pair_key(&a, &b), (a, b));
        true
    }

    fn is_rejected(&self, a_key: &str, b_key: &str) -> bool {
        let wanted = pair_key(&self.resolve_key(a_key), &self.resolve_key(b_key));
        self.rejected_pairs
            .values()
            .any(|(a, b)| pair_key(&self.resolve_key(a), &self.resolve_key(b)) == wanted)
    }

    /// Records likely to be one device under a reissued id: same name, the
    /// source idle for more than ten minutes, the target seen since.
    pub fn merge_candidates(&self, now_ms: i64) -> Vec<MergeCandidate> {
        let named: Vec<(&String, &TotalState, String)> = self
            .states
            .iter()
            .filter_map(|(key, state)| {
                let name = state.name.as_deref()?.trim();
                (!name.is_empty()).then(|| (key, state, name.to_owned()))
            })
            .collect();
        let mut candidates = Vec::new();
        for (from_key, source, source_name) in &named {
            if now_ms - source.last_seen_ms <= MERGE_IDLE_AFTER_MS {
                continue;
            }
            for (to_key, target, target_name) in &named {
                if to_key == from_key
                    || target.last_seen_ms <= source.last_seen_ms
                    || source_name.to_lowercase() != target_name.to_lowercase()
                    || self.is_rejected(from_key, to_key)
                {
                    continue;
                }
                let folds_bytes = source.period_month == target.period_month;
                let extra = |value: u64| if folds_bytes { value } else { 0 };
                candidates.push(MergeCandidate {
                    from_key: (*from_key).clone(),
                    to_key: (*to_key).clone(),
                    detail: target_name.clone(),
                    folds_bytes,
                    result_rx_bytes: monthly_rx(target) + extra(monthly_rx(source)),
                    result_tx_bytes: monthly_tx(target) + extra(monthly_tx(source)),
                });
            }
        }
        candidates.sort_by(|a, b| a.from_key.cmp(&b.from_key).then(a.to_key.cmp(&b.to_key)));
        candidates
    }

    pub fn totals(&self, client_key: Option<&str>) -> Vec<ClientTotal> {
        let strip = |s: &TotalState| ClientTotal {
            client_id: s.client_id,
            mac_address: s.mac_address.clone(),
            name: s.name.clone(),
            ip_address: s.ip_address.clone(),
            rx_bytes: monthly_rx(s),
            tx_bytes: monthly_tx(s),
            since_ms: s.since_ms,
            last_seen_ms: s.last_seen_ms,
            months: s.months.clone(),
        };

        if let Some(key) = client_key {
            let resolved = self.resolve_key(key);
            self.states.get(&resolved).map(strip).into_iter().collect()
        } else {
            let mut list: Vec<ClientTotal> = self.states.values().map(strip).collect();
            list.sort_by(|a, b| {
                b.last_seen_ms.cmp(&a.last_seen_ms).then_with(|| {
                    key_of(a.client_id, &a.mac_address).cmp(&key_of(b.client_id, &b.mac_address))
                })
            });
            list
        }
    }
}

/// Durable file-backed store wrapping `ClientTotalsCore`.
// SPEC-ODO-005: atomic file snapshot persistence.
pub struct ClientTotalsStore {
    file_path: PathBuf,
    core: ClientTotalsCore,
}

impl ClientTotalsStore {
    pub fn open(file_path: impl Into<PathBuf>) -> Self {
        let file_path = file_path.into();
        let mut core = ClientTotalsCore::new();
        if file_path.exists()
            && let Ok(content) = fs::read_to_string(&file_path)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
        {
            if let Ok(snapshot) = serde_json::from_value::<Snapshot>(value.clone()) {
                if snapshot.version == VERSION {
                    core.load_snapshot(snapshot);
                }
            } else if let Ok(snapshot) = serde_json::from_value::<SnapshotV3>(value)
                && snapshot.version == 3
            {
                core.load_snapshot(migrate_v3(snapshot));
            }
        }
        Self { file_path, core }
    }

    pub fn core(&self) -> &ClientTotalsCore {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut ClientTotalsCore {
        &mut self.core
    }

    /// Persist internal state to disk via an atomic temporary file rename.
    pub fn snapshot(&self) -> Result<(), std::io::Error> {
        let parent = self.file_path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let temp_path = self.file_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&self.core.to_snapshot())?;
        {
            let mut file = File::create(&temp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temp_path, &self.file_path)?;
        Ok(())
    }
}

fn migrate_v3(snapshot: SnapshotV3) -> Snapshot {
    let totals = snapshot
        .totals
        .into_iter()
        .map(|state| TotalState {
            client_id: state.client_id,
            mac_address: state.mac_address,
            name: state.name,
            ip_address: state.ip_address,
            captive_client_id: state.captive_client_id,
            lifetime_rx: state.rx_bytes,
            lifetime_tx: state.tx_bytes,
            month_anchor_rx: 0,
            month_anchor_tx: 0,
            since_ms: state.since_ms,
            last_seen_ms: state.last_seen_ms,
            period_month: state.period_month,
            months: state.months,
            prev_rx: state.prev_rx,
            prev_tx: state.prev_tx,
            last_poll_ms: state.last_poll_ms,
            self_traffic_debt_rx: state.self_traffic_debt_rx,
            self_traffic_debt_tx: state.self_traffic_debt_tx,
            corrected_rx: state.corrected_rx,
            corrected_tx: state.corrected_tx,
        })
        .collect();
    Snapshot {
        version: VERSION,
        totals,
        shared_macs: snapshot.shared_macs,
        aliases: snapshot.aliases,
        user_merged: Vec::new(),
        rejected_pairs: snapshot.rejected_pairs,
    }
}

fn pair_key(a: &str, b: &str) -> String {
    if a < b {
        format!("{a}\0{b}")
    } else {
        format!("{b}\0{a}")
    }
}
