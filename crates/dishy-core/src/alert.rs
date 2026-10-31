//! Alert definitions, transition tracking, and durable episodes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::jsonl::{
    JsonlResult, ensure_parent_directory, read_json_lines, write_json_lines_atomically,
};
use crate::meter::ClientMonthlyLimit;
use crate::odometer::ClientTotal;

const RETENTION_MS: i64 = 48 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSource {
    Dish,
    Router,
    System,
}

impl AlertSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dish => "dish",
            Self::Router => "router",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Warning,
    Advisory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertTransitionKind {
    Fired,
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterPresence {
    Unknown,
    Present,
    Bypassed,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertState {
    pub source: AlertSource,
    pub key: String,
    pub severity: Severity,
    pub active: bool,
    pub label: Option<String>,
}

impl AlertState {
    pub fn message_key(&self, kind: AlertTransitionKind) -> String {
        if self.source == AlertSource::System && self.key.starts_with("monthlyDataLimit:") {
            return format!(
                "alerts.system.monthlyDataLimit.{}",
                match kind {
                    AlertTransitionKind::Fired => "firing",
                    AlertTransitionKind::Cleared => "ok",
                }
            );
        }
        let face = match kind {
            AlertTransitionKind::Fired => "firing",
            AlertTransitionKind::Cleared => "ok",
        };
        format!("alerts.{}.{}.{}", self.source.as_str(), self.key, face)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertTransition {
    pub kind: AlertTransitionKind,
    pub source: AlertSource,
    pub key: String,
    pub severity: Severity,
    pub at_ms: i64,
    pub label: Option<String>,
}

impl AlertTransition {
    pub fn message_key(&self) -> String {
        if self.source == AlertSource::System && self.key.starts_with("monthlyDataLimit:") {
            return format!(
                "alerts.system.monthlyDataLimit.{}",
                match self.kind {
                    AlertTransitionKind::Fired => "firing",
                    AlertTransitionKind::Cleared => "ok",
                }
            );
        }
        let face = match self.kind {
            AlertTransitionKind::Fired => "firing",
            AlertTransitionKind::Cleared => "ok",
        };
        format!("alerts.{}.{}.{}", self.source.as_str(), self.key, face)
    }

    pub fn notification_key(&self) -> String {
        format!(
            "alert-{}:{}{}",
            self.source.as_str(),
            self.key,
            if self.kind == AlertTransitionKind::Cleared {
                "-cleared"
            } else {
                ""
            }
        )
    }

    pub fn is_notifiable(&self) -> bool {
        // A rule announcement retires on a timer, so its clearing would claim a
        // device is within its allowance while it is still over (spec 0169).
        if self.source == AlertSource::System && self.key.starts_with("dataLimit:") {
            return self.kind == AlertTransitionKind::Fired;
        }
        (self.source == AlertSource::System && self.key.starts_with("monthlyDataLimit:"))
            || specification(self.source, &self.key).is_some_and(|spec| spec.notify)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSnapshot {
    pub active: Vec<AlertState>,
    pub status_list: Vec<AlertState>,
    pub history: Vec<AlertEpisode>,
    pub transitions: Vec<AlertTransition>,
    pub dish_reachable: Option<bool>,
    pub router_reachable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertEpisode {
    pub source: AlertSource,
    pub key: String,
    pub severity: Severity,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Default)]
pub struct AlertEngine {
    states: BTreeMap<(AlertSource, String), AlertState>,
    router_presence: Option<RouterPresence>,
}

impl AlertEngine {
    pub fn new(open_episodes: &[AlertEpisode]) -> Self {
        let mut engine = Self::default();
        for episode in open_episodes {
            if episode.end_ms.is_some() {
                continue;
            }
            let state = AlertState {
                source: episode.source,
                key: episode.key.clone(),
                severity: episode.severity,
                active: true,
                label: episode.label.clone(),
            };
            engine
                .states
                .insert((state.source, state.key.clone()), state);
        }
        engine
    }

    pub fn observe_dish(
        &mut self,
        alerts: Option<&BTreeMap<String, bool>>,
        eth_speed_mbps: Option<f64>,
        router_presence: Option<RouterPresence>,
        at_ms: i64,
    ) -> Vec<AlertTransition> {
        if let Some(presence) = router_presence {
            self.router_presence = Some(presence);
        }
        match alerts {
            Some(alerts) => {
                let mut transitions = Vec::new();
                for spec in DISH_SPECS {
                    let mut active = alerts.get(spec.key).copied().unwrap_or(false);
                    if spec.key == "noEthernetLink"
                        && active
                        && eth_speed_mbps.is_some_and(|speed| speed > 0.0)
                    {
                        active = false;
                    }
                    self.set_active(
                        AlertSource::Dish,
                        spec.key,
                        spec.severity,
                        active,
                        at_ms,
                        &mut transitions,
                    );
                }
                self.set_active(
                    AlertSource::System,
                    "dishUnreachable",
                    Severity::Critical,
                    false,
                    at_ms,
                    &mut transitions,
                );
                transitions
            }
            None => {
                let mut transitions = Vec::new();
                self.set_active(
                    AlertSource::System,
                    "dishUnreachable",
                    Severity::Critical,
                    true,
                    at_ms,
                    &mut transitions,
                );
                transitions
            }
        }
    }

    /// The satellite side, judged from the recorder's own samples: neither box
    /// flags an outage while the dish answers and its pings do not come back
    /// (spec 0174).
    pub fn observe_link(
        &mut self,
        samples: &[crate::telemetry::TelemetrySample],
        at_ms: i64,
    ) -> Vec<AlertTransition> {
        let mut transitions = Vec::new();
        self.set_active(
            AlertSource::System,
            "starlinkOutage",
            Severity::Critical,
            crate::telemetry::is_starlink_outage(samples),
            at_ms,
            &mut transitions,
        );
        transitions
    }

    pub fn observe_router(
        &mut self,
        alerts: Option<&BTreeMap<String, bool>>,
        at_ms: i64,
    ) -> Vec<AlertTransition> {
        match alerts {
            Some(alerts) => {
                let mut transitions = Vec::new();
                for spec in ROUTER_SPECS {
                    self.set_active(
                        AlertSource::Router,
                        spec.key,
                        spec.severity,
                        alerts.get(spec.key).copied().unwrap_or(false),
                        at_ms,
                        &mut transitions,
                    );
                }
                self.set_active(
                    AlertSource::System,
                    "routerUnreachable",
                    Severity::Warning,
                    false,
                    at_ms,
                    &mut transitions,
                );
                transitions
            }
            None => {
                let expected = matches!(
                    self.router_presence,
                    Some(RouterPresence::Bypassed | RouterPresence::Absent)
                );
                let mut transitions = Vec::new();
                self.set_active(
                    AlertSource::System,
                    "routerUnreachable",
                    Severity::Warning,
                    !expected,
                    at_ms,
                    &mut transitions,
                );
                transitions
            }
        }
    }

    /// Announce rule transitions (spec 0169, SPEC-RULE-009). A plain monthly
    /// limit is left to `observe_monthly_limits`, so no rule alerts twice.
    pub fn observe_meter_transitions(
        &mut self,
        meter_transitions: &[crate::data_meter::MeterTransition],
        label_for: impl Fn(&crate::data_meter::MeterRule) -> Option<String>,
        at_ms: i64,
    ) -> Vec<AlertTransition> {
        let mut transitions = Vec::new();
        for meter in meter_transitions {
            if crate::meter_store::is_monthly_limit(&meter.rule) {
                continue;
            }
            let key = format!(
                "dataLimit:{}",
                crate::data_meter::announcement_subject(&meter.rule)
            );
            self.set_active_with_label(
                AlertSource::System,
                &key,
                Severity::Warning,
                meter.kind == crate::data_meter::MeterTransitionKind::Reached,
                label_for(&meter.rule),
                at_ms,
                &mut transitions,
            );
        }
        transitions
    }

    /// Evaluate watch-only monthly caps against already-observed odometer
    /// totals. Missing totals are deliberately not treated as zero usage.
    pub fn observe_monthly_limits(
        &mut self,
        limits: &[ClientMonthlyLimit],
        totals: &[ClientTotal],
        at_ms: i64,
    ) -> Vec<AlertTransition> {
        let mut transitions = Vec::new();
        let mut retained_keys = std::collections::HashSet::new();
        for limit in limits {
            let key = format!("monthlyDataLimit:{}", limit.client_key);
            retained_keys.insert(key.clone());
            let Some(total) = totals.iter().find(|total| {
                total
                    .client_id
                    .map(|client_id| client_id.to_string())
                    .unwrap_or_else(|| total.mac_address.clone())
                    == limit.client_key
            }) else {
                continue;
            };
            let label = total
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&limit.client_key)
                .to_owned();
            let used_bytes = total.rx_bytes.saturating_add(total.tx_bytes);
            self.set_active_with_label(
                AlertSource::System,
                &key,
                Severity::Warning,
                used_bytes >= limit.limit_bytes,
                Some(label),
                at_ms,
                &mut transitions,
            );
        }

        let removed: Vec<(String, Option<String>)> = self
            .states
            .iter()
            .filter(|((source, key), state)| {
                *source == AlertSource::System
                    && state.active
                    && key.starts_with("monthlyDataLimit:")
                    && !retained_keys.contains(key)
            })
            .map(|((_, key), state)| (key.clone(), state.label.clone()))
            .collect();
        for (key, label) in removed {
            self.set_active_with_label(
                AlertSource::System,
                &key,
                Severity::Warning,
                false,
                label,
                at_ms,
                &mut transitions,
            );
        }
        transitions
    }

    pub fn active_alerts(&self) -> Vec<AlertState> {
        let mut active: Vec<_> = self
            .states
            .values()
            .filter(|state| state.active)
            .cloned()
            .collect();
        active.sort_by_key(|state| (state.severity, state.source, state.key.clone()));
        active
    }

    pub fn status_list(&self) -> Vec<AlertState> {
        DISH_SPECS
            .iter()
            .map(|spec| self.status_for(AlertSource::Dish, spec))
            .chain(
                ROUTER_SPECS
                    .iter()
                    .map(|spec| self.status_for(AlertSource::Router, spec)),
            )
            .collect()
    }

    fn status_for(&self, source: AlertSource, spec: &AlertSpec) -> AlertState {
        self.states
            .get(&(source, spec.key.to_owned()))
            .cloned()
            .unwrap_or_else(|| AlertState {
                source,
                key: spec.key.to_owned(),
                severity: spec.severity,
                active: false,
                label: None,
            })
    }

    fn set_active(
        &mut self,
        source: AlertSource,
        key: &str,
        severity: Severity,
        active: bool,
        at_ms: i64,
        transitions: &mut Vec<AlertTransition>,
    ) {
        self.set_active_with_label(source, key, severity, active, None, at_ms, transitions);
    }

    #[allow(clippy::too_many_arguments)]
    fn set_active_with_label(
        &mut self,
        source: AlertSource,
        key: &str,
        severity: Severity,
        active: bool,
        label: Option<String>,
        at_ms: i64,
        transitions: &mut Vec<AlertTransition>,
    ) {
        let id = (source, key.to_owned());
        let previous = self.states.get(&id);
        let was_active = previous.is_some_and(|state| state.active);
        let label = label.or_else(|| previous.and_then(|state| state.label.clone()));
        self.states.insert(
            id,
            AlertState {
                source,
                key: key.to_owned(),
                severity,
                active,
                label: label.clone(),
            },
        );
        if was_active != active {
            transitions.push(AlertTransition {
                kind: if active {
                    AlertTransitionKind::Fired
                } else {
                    AlertTransitionKind::Cleared
                },
                source,
                key: key.to_owned(),
                severity,
                at_ms,
                label,
            });
        }
    }
}

#[derive(Debug)]
pub struct AlertStore {
    file_path: PathBuf,
    episodes: Vec<AlertEpisode>,
}

impl AlertStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        let episodes = read_json_lines::<AlertEpisode>(&file_path)?;
        Ok(Self {
            file_path,
            episodes,
        })
    }

    pub fn record(&mut self, transitions: &[AlertTransition], now_ms: i64) -> JsonlResult<()> {
        if transitions.is_empty() {
            return Ok(());
        }
        for transition in transitions {
            let existing = self.episodes.iter_mut().find(|episode| {
                episode.source == transition.source
                    && episode.key == transition.key
                    && episode.end_ms.is_none()
            });
            match (transition.kind, existing) {
                (AlertTransitionKind::Fired, None) => self.episodes.push(AlertEpisode {
                    source: transition.source,
                    key: transition.key.clone(),
                    severity: transition.severity,
                    start_ms: transition.at_ms,
                    end_ms: None,
                    label: transition.label.clone(),
                }),
                (AlertTransitionKind::Cleared, Some(episode)) => {
                    episode.end_ms = Some(transition.at_ms);
                    if transition.label.is_some() {
                        episode.label.clone_from(&transition.label);
                    }
                }
                _ => {}
            }
        }
        self.retain(now_ms);
        self.flush()
    }

    pub fn all(&self, now_ms: i64) -> Vec<AlertEpisode> {
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        let mut episodes: Vec<_> = self
            .episodes
            .iter()
            .filter(|episode| episode.end_ms.is_none_or(|end_ms| end_ms >= cutoff))
            .cloned()
            .collect();
        episodes.sort_by_key(|episode| std::cmp::Reverse(episode.start_ms));
        episodes
    }

    pub fn open_episodes(&self) -> Vec<AlertEpisode> {
        self.episodes
            .iter()
            .filter(|episode| episode.end_ms.is_none())
            .cloned()
            .collect()
    }

    fn retain(&mut self, now_ms: i64) {
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        self.episodes
            .retain(|episode| episode.end_ms.is_none_or(|end_ms| end_ms >= cutoff));
    }

    fn flush(&mut self) -> JsonlResult<()> {
        self.episodes.sort_by_key(|episode| episode.start_ms);
        write_json_lines_atomically(&self.file_path, &self.episodes)
    }
}

#[derive(Clone, Copy)]
struct AlertSpec {
    key: &'static str,
    severity: Severity,
    notify: bool,
}

const DISH_SPECS: &[AlertSpec] = &[
    spec("dishWaterDetected", Severity::Critical, true),
    spec("routerWaterDetected", Severity::Critical, true),
    spec("thermalShutdown", Severity::Critical, true),
    spec("noEthernetLink", Severity::Critical, true),
    spec("motorsStuck", Severity::Critical, true),
    spec("thermalThrottle", Severity::Warning, true),
    spec("powerSupplyThermalThrottle", Severity::Warning, true),
    spec("slowEthernetSpeeds", Severity::Warning, true),
    spec("slowEthernetSpeeds100", Severity::Warning, false),
    spec("upsuRouterPortSlow", Severity::Warning, false),
    spec("mastNotNearVertical", Severity::Warning, false),
    spec("lowMotorCurrent", Severity::Warning, false),
    spec("lowerSignalThanPredicted", Severity::Warning, false),
    spec("dbfTelemStale", Severity::Warning, false),
    spec("unexpectedLocation", Severity::Warning, false),
    spec("obstructionMapReset", Severity::Advisory, false),
    spec("roaming", Severity::Advisory, false),
    spec("isHeating", Severity::Advisory, false),
    spec("isPowerSaveIdle", Severity::Advisory, false),
    spec("installPending", Severity::Advisory, false),
];

const ROUTER_SPECS: &[AlertSpec] = &[
    spec("poeFuseBlown", Severity::Critical, true),
    spec("poeVinOvervoltage", Severity::Critical, true),
    spec("poeVinUndervoltage", Severity::Critical, true),
    spec("poeRouterOvercurrent", Severity::Critical, true),
    spec("poeOnDishUnreachable", Severity::Critical, true),
    spec("ethSwitchError", Severity::Critical, true),
    spec("wanEthPoorConnection", Severity::Warning, true),
    spec("highCablePingDropRate", Severity::Warning, true),
    spec("thermalThrottle", Severity::Warning, true),
    spec("meshUnreliableBackhaul", Severity::Warning, false),
    spec("meshTopologyChangingOften", Severity::Warning, false),
    spec("lanEthSlowLink10", Severity::Warning, false),
    spec("lanEthSlowLink100", Severity::Warning, false),
    spec("wiredMeshNotUsingWanIface", Severity::Warning, false),
    spec("poeOffCurrentNominal", Severity::Warning, false),
    spec("radiusMissingProcess", Severity::Warning, false),
    spec("installPending", Severity::Advisory, false),
    spec("freshlyFused", Severity::Advisory, false),
    spec("sandboxDisabled", Severity::Advisory, false),
    spec("onlyOverflightBlocked", Severity::Advisory, false),
    spec("offlineNetworksDisabled", Severity::Advisory, false),
];

const fn spec(key: &'static str, severity: Severity, notify: bool) -> AlertSpec {
    AlertSpec {
        key,
        severity,
        notify,
    }
}

fn specification(source: AlertSource, key: &str) -> Option<AlertSpec> {
    let specs = match source {
        AlertSource::Dish => DISH_SPECS,
        AlertSource::Router => ROUTER_SPECS,
        AlertSource::System => {
            return match key {
                "dishUnreachable" => Some(spec("dishUnreachable", Severity::Critical, true)),
                "routerUnreachable" => Some(spec("routerUnreachable", Severity::Warning, true)),
                "starlinkOutage" => Some(spec("starlinkOutage", Severity::Critical, true)),
                _ => None,
            };
        }
    };
    specs.iter().find(|spec| spec.key == key).copied()
}

pub fn is_notifiable(source: AlertSource, key: &str) -> bool {
    specification(source, key).is_some_and(|spec| spec.notify)
}

pub fn severity_for(source: AlertSource, key: &str) -> Option<Severity> {
    specification(source, key).map(|spec| spec.severity)
}

pub fn known_status_keys(source: AlertSource) -> Vec<&'static str> {
    let specs = match source {
        AlertSource::Dish => DISH_SPECS,
        AlertSource::Router => ROUTER_SPECS,
        AlertSource::System => {
            return vec!["dishUnreachable", "routerUnreachable", "starlinkOutage"];
        }
    };
    specs.iter().map(|spec| spec.key).collect()
}

pub fn expected_router_silence(presence: Option<RouterPresence>) -> bool {
    matches!(
        presence,
        Some(RouterPresence::Bypassed | RouterPresence::Absent)
    )
}

pub fn history_path(data_dir: &Path) -> PathBuf {
    data_dir.join("alerts.ndjson")
}
