//! One recorder tick, shared by every host.
//!
//! The GTK app and the headless CLI have the same job: at the approved cadences,
//! ask the dish for its history, status, and obstruction map, stamp the router
//! readings from the router status reply it already fetches, and hand both to
//! [`Recorder`]. That loop belongs above the recorder rather than in either
//! binary, so the two cannot drift apart or accidentally add a second poll.
//!
//! This is deliberately one owner. Hosts own their scheduling and cancellation:
//! the GTK app stops a background thread on quit, while the CLI waits on
//! SIGINT. Neither path can call the dish twice for one cycle, and neither
//! polls the router beyond the historian's existing per-cycle status reply.

use thiserror::Error;

use crate::alert::{AlertEngine, AlertSnapshot, AlertTransition, RouterPresence};
use crate::calendar::now_ms;
use crate::client::{DishClient, DishClientError, Target};
use crate::control::{ControlAction, ControlTarget};
use crate::jsonl::JsonlError;
use crate::meter::ClientMonthlyLimit;
use crate::network::{
    ClientRateTracker, NetworkDevice, NetworkSnapshot, RadioReading, RadioStatsReply, WifiClient,
    WifiClientStats, WifiConfig, build_nodes,
};
use crate::odometer::{ClientReading, ClientTotal};
use crate::recorder::{PollOutcome, Recorder, RecorderError};
use crate::store::{ObstructionSnapshot, pack_cells};
use crate::telemetry::{
    DishHistory, DishObstructionMap, DishStatus, RouterReadings, RouterStatus, TelemetrySample,
};

/// The result of one history poll, including the newest sample so a host can
/// paint live throughput without reading the stores back off disk.
#[derive(Debug, Clone, PartialEq)]
pub struct PollReport {
    pub outcome: PollOutcome,
    /// The newest sample in the rolling window, or `None` when the dish has not
    /// written one yet.
    pub latest_sample: Option<TelemetrySample>,
    /// Whether the router answered the status read that this poll already makes.
    /// This is an observation from the existing cycle, not a new router poll.
    pub router_reachable: bool,
    /// `Some(None)` means the existing router status call failed; `None` means
    /// no parseable status observation was available.
    pub router_alerts: Option<Option<std::collections::BTreeMap<String, bool>>>,
}

/// What one [`RecorderRunner::tick`] did. Each reply field is present only when
/// its poll was due and answered. A history failure is reported separately so
/// the successfully fetched status and obstruction replies still reach the UI.
#[derive(Debug, Default)]
pub struct TickReport {
    pub history: Option<PollReport>,
    pub status: Option<DishStatus>,
    pub obstruction: Option<DishObstructionMap>,
    pub history_error: Option<RunnerError>,
    /// A network snapshot, present only while the page is open and only when
    /// the roster, config, or radios changed (or the router went unreachable).
    pub network: Option<NetworkSnapshot>,
    /// Per-device usage totals from the odometer.
    pub client_totals: Option<Vec<ClientTotal>>,
    /// Dish settings read only while the Settings page is open.
    pub dish_config: Option<serde_json::Value>,
    pub alerts: Option<AlertSnapshot>,
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("dish client: {0}")]
    Client(#[from] DishClientError),
    #[error("dish history reply: {0}")]
    Decode(#[from] serde_json::Error),
    #[error(transparent)]
    Recorder(#[from] RecorderError),
    #[error(transparent)]
    Store(#[from] JsonlError),
}

/// A dish client, a router client, and the recorder they feed.
///
/// Both clients are constructed once and reused, which keeps one connection
/// pool for the process and makes each cycle's request budget obvious.
pub struct RecorderRunner {
    dish: DishClient,
    router: DishClient,
    recorder: Recorder,
    alerts: AlertEngine,
    dish_alert_reachable: Option<bool>,
    router_alert_reachable: Option<bool>,
    last_history_ms: Option<i64>,
    last_status_ms: Option<i64>,
    last_obstruction_ms: Option<i64>,

    // The network page's demand-gated reads. `wanted` is set by the host while
    // that page is mapped; nothing here polls the router unless it is true.
    network_wanted: bool,
    device_usage_wanted: bool,
    dish_config_wanted: bool,
    last_dish_config_ms: Option<i64>,
    last_network_clients_ms: Option<i64>,
    last_network_config_ms: Option<i64>,
    last_network_radio_ms: Option<i64>,
    network_config_dirty: bool,
    network_clients: Vec<NetworkDevice>,
    network_config: Option<WifiConfig>,
    network_radios: Vec<RadioReading>,
    network_tracker: ClientRateTracker,
    network_reachable: bool,
    network_misses: u32,

    // The two saved dimensions of the recorder's LAN router endpoint. `None`
    // means "use the discovered default" for that dimension.
    router_address: Option<String>,
    router_port: Option<u16>,
}

impl RecorderRunner {
    pub fn open(data_dir: impl AsRef<std::path::Path>) -> Result<Self, RunnerError> {
        Self::open_with_router_address(data_dir, None)
    }

    pub fn open_with_router_address(
        data_dir: impl AsRef<std::path::Path>,
        router_address: Option<&str>,
    ) -> Result<Self, RunnerError> {
        Self::open_with_router_endpoint(data_dir, router_address, None)
    }

    pub fn open_with_router_endpoint(
        data_dir: impl AsRef<std::path::Path>,
        router_address: Option<&str>,
        router_port: Option<u16>,
    ) -> Result<Self, RunnerError> {
        let recorder = Recorder::open(data_dir)?;
        let alerts = AlertEngine::new(&recorder.alerts().open_episodes());
        let router_address = router_address.map(str::to_owned);
        Ok(Self {
            dish: DishClient::new(Target::Dish)?,
            router: DishClient::router_client(router_address.as_deref(), router_port)?,
            recorder,
            alerts,
            dish_alert_reachable: None,
            router_alert_reachable: None,
            last_history_ms: None,
            last_status_ms: None,
            last_obstruction_ms: None,
            network_wanted: false,
            device_usage_wanted: false,
            dish_config_wanted: false,
            last_dish_config_ms: None,
            last_network_clients_ms: None,
            last_network_config_ms: None,
            last_network_radio_ms: None,
            network_config_dirty: true,
            network_clients: Vec::new(),
            network_config: None,
            network_radios: Vec::new(),
            network_tracker: ClientRateTracker::default(),
            network_reachable: false,
            network_misses: 0,
            router_address,
            router_port,
        })
    }

    /// Shared reset path: re-apply the router endpoint's client and reset the
    /// network-demand cycle exactly once. Neither an address-only nor a
    /// port-only save may add a poll.
    // SPEC-CTRL-011: one shared reset path for both dimensions.
    fn apply_router_endpoint(&mut self) -> Result<(), RunnerError> {
        self.router = DishClient::router_client(self.router_address.as_deref(), self.router_port)?;
        self.last_network_clients_ms = None;
        self.last_network_config_ms = None;
        self.last_network_radio_ms = None;
        self.network_config_dirty = true;
        self.network_clients.clear();
        self.network_config = None;
        self.network_radios.clear();
        self.network_reachable = false;
        self.network_misses = 0;
        Ok(())
    }

    /// Swap the LAN endpoint after the user saves a corrected address, keeping
    /// the saved port. Force the existing network-demand cycle to obtain a fresh
    /// config and roster.
    pub fn set_router_address(&mut self, address: Option<&str>) -> Result<(), RunnerError> {
        self.router_address = address.map(str::to_owned);
        self.apply_router_endpoint()
    }

    /// Swap the LAN endpoint after the user saves a corrected access port,
    /// keeping the saved address.
    pub fn set_router_port(&mut self, port: Option<u16>) -> Result<(), RunnerError> {
        self.router_port = port;
        self.apply_router_endpoint()
    }

    pub fn data_dir(&self) -> &std::path::Path {
        self.recorder.data_dir()
    }

    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    pub fn recorder_mut(&mut self) -> &mut Recorder {
        &mut self.recorder
    }

    /// Fetch support diagnostics only after a user request. This is intentionally
    /// separate from `tick` so it never joins any automatic polling cadence.
    // SPEC-DIAG-002: both reads reuse the recorder-owned dish client.
    pub async fn diagnostics(&self) -> Result<(serde_json::Value, serde_json::Value), RunnerError> {
        let device_info = self.dish.get_device_info().await?;
        let diagnostics = self.dish.get_diagnostics().await?;
        Ok((device_info, diagnostics))
    }

    /// Reset one device's current-month usage while preserving its lifetime total.
    pub fn reset_client_total(&mut self, client_key: &str, at_ms: i64) -> bool {
        self.recorder.reset_client_total(client_key, at_ms)
    }

    /// Remove one device's usage history.
    pub fn remove_client_total(&mut self, client_key: &str) -> Result<bool, RunnerError> {
        Ok(self.recorder.remove_client_total(client_key)?)
    }

    pub fn set_client_monthly_limit(
        &mut self,
        limit: ClientMonthlyLimit,
    ) -> Result<(), RunnerError> {
        self.recorder.set_client_monthly_limit(limit)?;
        Ok(())
    }

    pub fn remove_client_monthly_limit(&mut self, client_key: &str) -> Result<bool, RunnerError> {
        Ok(self.recorder.remove_client_monthly_limit(client_key)?)
    }

    /// Clear all per-device usage history.
    pub fn clear_client_totals(&mut self) -> Result<(), RunnerError> {
        self.recorder.clear_client_totals()?;
        Ok(())
    }

    /// Re-evaluate watch-only monthly caps against the already persisted
    /// odometer state. This performs no device request.
    pub fn evaluate_monthly_limit_alerts(
        &mut self,
        at_ms: i64,
    ) -> Result<Option<AlertSnapshot>, RunnerError> {
        let mut transitions = self.alerts.observe_monthly_limits(
            &self.recorder.client_monthly_limits(),
            &self.recorder.client_totals(None),
            at_ms,
        );
        transitions.extend(self.observe_rules(at_ms));
        if transitions.is_empty() {
            return Ok(None);
        }
        self.recorder.alerts_mut().record(&transitions, at_ms)?;
        Ok(Some(self.alert_snapshot(transitions, at_ms)))
    }

    /// Fold the odometer into the rules and announce what crossed. Uses only
    /// recorder state (SPEC-RULE-008); a failed rule write is logged, not fatal.
    fn observe_rules(&mut self, at_ms: i64) -> Vec<AlertTransition> {
        let meter_transitions = match self.recorder.evaluate_meters(at_ms) {
            Ok(transitions) => transitions,
            Err(error) => {
                tracing::warn!("metering rules not saved: {error}");
                return Vec::new();
            }
        };
        let totals = self.recorder.client_totals(None);
        let groups = self.recorder.meters().groups().to_vec();
        let label_for = |rule: &crate::data_meter::MeterRule| {
            let group = groups
                .iter()
                .find(|group| Some(&group.group_id) == rule.group_id.as_ref())
                .filter(|group| {
                    crate::data_meter::announces_as_group(rule) && group.member_keys.len() > 1
                });
            let subject = group.map(|group| group.name.clone()).unwrap_or_else(|| {
                totals
                    .iter()
                    .find(|total| {
                        crate::odometer::key_of(total.client_id, &total.mac_address)
                            == rule.client_key
                    })
                    .and_then(|total| total.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| rule.client_key.clone())
            });
            Some(
                crate::data_meter::DataLimitLabel::for_rule(rule, &subject, group.is_some())
                    .encode(),
            )
        };
        self.alerts
            .observe_meter_transitions(&meter_transitions, label_for, at_ms)
    }

    fn alert_snapshot(&self, transitions: Vec<AlertTransition>, at_ms: i64) -> AlertSnapshot {
        AlertSnapshot {
            active: self.alerts.active_alerts(),
            status_list: self.alerts.status_list(),
            history: self.recorder.alerts().all(at_ms),
            transitions,
            dish_reachable: self.dish_alert_reachable,
            router_reachable: self.router_alert_reachable,
        }
    }

    /// Open or close the network page's demand gate.
    ///
    /// Turning it on makes the first roster and radio reads due immediately.
    /// Turning it off drops the rate tracker's counters, so a later open cannot
    /// compute a rate across the closed interval.
    // SPEC-SAFETY-003: the page's reads are demand-gated and stop on close.
    pub fn set_network_wanted(&mut self, wanted: bool) {
        if wanted == self.network_wanted {
            return;
        }
        self.network_wanted = wanted;
        if wanted {
            self.last_network_clients_ms = None;
            self.last_network_radio_ms = None;
            self.network_misses = 0;
            if self.network_config.is_none() {
                self.network_config_dirty = true;
                self.last_network_config_ms = None;
            }
        } else {
            if !self.device_usage_wanted {
                self.network_tracker.reset();
                self.last_network_clients_ms = None;
            }
            self.last_network_radio_ms = None;
            self.last_network_config_ms = None;
        }
    }

    /// Open or close the device usage page's demand gate.
    // SPEC-ODO-006: viewing device usage activates client roster polling.
    pub fn set_device_usage_wanted(&mut self, wanted: bool) {
        if wanted == self.device_usage_wanted {
            return;
        }
        self.device_usage_wanted = wanted;
        if wanted {
            self.last_network_clients_ms = None;
            self.network_misses = 0;
        } else if !self.network_wanted {
            self.network_tracker.reset();
            self.last_network_clients_ms = None;
        }
    }

    pub fn set_dish_config_wanted(&mut self, wanted: bool) {
        if wanted && !self.dish_config_wanted {
            self.last_dish_config_ms = None;
        }
        self.dish_config_wanted = wanted;
    }

    /// Send one confirmed maintenance write through the client the action names.
    /// This is the only write path, so a view cannot accidentally use the wrong
    /// box or open its own client.
    // SPEC-CTRL-002, SPEC-SAFETY-004: serialized one-shot writes, no retry.
    pub async fn control(&self, action: ControlAction) -> Result<(), RunnerError> {
        let client = match action.target() {
            ControlTarget::Dish => &self.dish,
            ControlTarget::Router => &self.router,
        };
        client.perform_control(action).await?;
        Ok(())
    }

    fn network_snapshot(&self) -> NetworkSnapshot {
        NetworkSnapshot {
            reachable: self.network_reachable,
            devices: self.network_clients.clone(),
            nodes: build_nodes(&self.network_clients, self.network_config.as_ref()),
            config: self.network_config.clone(),
            radios: self.network_radios.clone(),
        }
    }

    /// Poll whatever is due at `at_ms`.
    ///
    /// A live-poll failure is logged and retried at its next interval; a
    /// history failure is returned in the report because the host's charts
    /// depend on it. Marking the attempt before the call keeps a failing device
    /// from being hammered by a fast host loop. History runs first so the
    /// dashboard paints before the sky's first (larger) obstruction reply, and a
    /// failed history poll never starves the status and obstruction cadences.
    pub async fn tick(&mut self, at_ms: i64) -> TickReport {
        let mut report = TickReport::default();
        let mut history_error = None;
        let mut alert_transitions = Vec::new();
        let mut has_alert_observation = false;

        if due(self.last_history_ms, at_ms, crate::HISTORIAN_POLL_INTERVAL) {
            self.last_history_ms = Some(at_ms);
            match self.poll_once_at(at_ms).await {
                Ok(poll) => {
                    has_alert_observation = true;
                    alert_transitions
                        .extend(self.alerts.observe_link(self.recorder.samples(), at_ms));
                    if let Some(router_alerts) = &poll.router_alerts {
                        has_alert_observation = true;
                        self.router_alert_reachable = Some(router_alerts.is_some());
                        alert_transitions
                            .extend(self.alerts.observe_router(router_alerts.as_ref(), at_ms));
                    }
                    report.history = Some(poll);
                }
                Err(error) => history_error = Some(error),
            }
        }

        if due(self.last_status_ms, at_ms, crate::STATUS_POLL_INTERVAL) {
            self.last_status_ms = Some(at_ms);
            match self.poll_status(at_ms).await {
                Ok(status) => {
                    has_alert_observation = true;
                    self.dish_alert_reachable = Some(true);
                    let presence = router_presence(&status, at_ms);
                    alert_transitions.extend(self.alerts.observe_dish(
                        Some(&status.alerts),
                        Some(status.eth_speed_mbps.0),
                        Some(presence),
                        at_ms,
                    ));
                    report.status = Some(status);
                }
                Err(error) => {
                    tracing::warn!("status poll failed: {error}");
                    has_alert_observation = true;
                    self.dish_alert_reachable = Some(false);
                    alert_transitions.extend(self.alerts.observe_dish(None, None, None, at_ms));
                }
            }
        }

        if due(
            self.last_obstruction_ms,
            at_ms,
            crate::OBSTRUCTION_POLL_INTERVAL,
        ) {
            self.last_obstruction_ms = Some(at_ms);
            match self.poll_obstruction(at_ms).await {
                Ok(map) => report.obstruction = map,
                Err(error) => tracing::warn!("obstruction poll failed: {error}"),
            }
        }

        if self.dish_config_wanted
            && due(
                self.last_dish_config_ms,
                at_ms,
                crate::DISH_CONFIG_POLL_INTERVAL,
            )
        {
            self.last_dish_config_ms = Some(at_ms);
            match self.dish.get_config().await {
                Ok(config) => report.dish_config = Some(config),
                Err(error) => tracing::warn!("dish config read failed: {error}"),
            }
        }

        if self.network_wanted || self.device_usage_wanted {
            let mut changed = false;

            if due(
                self.last_network_clients_ms,
                at_ms,
                crate::NETWORK_CLIENTS_POLL_INTERVAL,
            ) {
                self.last_network_clients_ms = Some(at_ms);
                match self.poll_network_clients(at_ms).await {
                    Ok(reported) => {
                        changed |= reported;
                        let mut transitions = self.alerts.observe_monthly_limits(
                            &self.recorder.client_monthly_limits(),
                            &self.recorder.client_totals(None),
                            at_ms,
                        );
                        transitions.extend(self.observe_rules(at_ms));
                        if !transitions.is_empty() {
                            alert_transitions.extend(transitions);
                            has_alert_observation = true;
                        }
                        if self.device_usage_wanted {
                            report.client_totals = Some(self.recorder.client_totals(None));
                        }
                    }
                    Err(error) => warn_network(&error),
                }
            }

            if self.network_wanted && self.network_reachable {
                if self.network_config_dirty
                    && due(
                        self.last_network_config_ms,
                        at_ms,
                        crate::NETWORK_CONFIG_RETRY_INTERVAL,
                    )
                {
                    self.last_network_config_ms = Some(at_ms);
                    match self.poll_network_config().await {
                        Ok(()) => changed = true,
                        Err(error) => warn_network(&error),
                    }
                }

                if due(
                    self.last_network_radio_ms,
                    at_ms,
                    crate::NETWORK_RADIO_POLL_INTERVAL,
                ) {
                    self.last_network_radio_ms = Some(at_ms);
                    match self.poll_network_radios().await {
                        Ok(()) => changed = true,
                        Err(error) => warn_network(&error),
                    }
                }
            }

            if self.network_wanted && changed {
                report.network = Some(self.network_snapshot());
            }
        }

        if has_alert_observation {
            // Before recording: the open episode is the outage's start.
            if let Err(error) = self
                .recorder
                .write_outage_reports(&alert_transitions, at_ms)
            {
                tracing::warn!("outage report write failed: {error}");
            }
            if let Err(error) = self.recorder.alerts_mut().record(&alert_transitions, at_ms) {
                tracing::warn!("alert history write failed: {error}");
            }
            report.alerts = Some(self.alert_snapshot(alert_transitions, at_ms));
        }
        report.history_error = history_error;
        report
    }

    /// Read the roster and rebuild the normalised device list. Returns true when
    /// the page should be sent a snapshot.
    async fn poll_network_clients(&mut self, at_ms: i64) -> Result<bool, RunnerError> {
        let value = match self.router.get_wifi_clients().await {
            Ok(value) => value,
            Err(error) => {
                let reported = self.network_failed();
                if reported {
                    warn_network(&RunnerError::Client(error));
                    return Ok(true);
                }
                return Err(error.into());
            }
        };
        let clients: Vec<WifiClient> = serde_json::from_value(value)?;

        // SPEC-ODO-001, SPEC-ODO-006: feed the roster to the odometer.
        let entries: Vec<(Option<i64>, &str)> = clients
            .iter()
            .filter(|client| client.is_client())
            .filter_map(|c| c.mac_address.as_deref().map(|mac| (c.client_id, mac)))
            .collect();
        let live_keys = self.recorder.odometer_mut().core_mut().note_poll(&entries);

        for client in &clients {
            observe_client_usage(
                self.recorder.odometer_mut().core_mut(),
                client,
                at_ms,
                &live_keys,
            );
        }

        let was_reachable = self.network_reachable;
        self.network_reachable = true;
        self.network_misses = 0;
        if !was_reachable {
            // A reconnect can mean a different subnet and a different config.
            self.network_config_dirty = true;
            self.last_network_config_ms = None;
            self.last_network_radio_ms = None;
        }
        let mut tracker = std::mem::take(&mut self.network_tracker);
        let snapshot = NetworkSnapshot::from_roster(
            &clients,
            self.network_config.as_ref(),
            self.network_radios.clone(),
            &mut tracker,
            at_ms,
            true,
        );
        self.network_tracker = tracker;
        // Keep the rates this read already computed for the device detail
        // chart (spec 0175); nothing extra is asked of the router.
        let rates: Vec<crate::client_window::ClientRate> = snapshot
            .devices
            .iter()
            .filter(|device| device.is_client)
            .map(|device| crate::client_window::ClientRate {
                key: device.usage_key.clone(),
                mac_address: device.mac_address.clone(),
                down_mbps: device.down_mbps,
                up_mbps: device.up_mbps,
            })
            .collect();
        self.recorder.client_window_mut().ingest(&rates, at_ms);
        self.network_clients = snapshot.devices;
        Ok(true)
    }

    async fn poll_network_config(&mut self) -> Result<(), RunnerError> {
        let value = self.router.get_wifi_config().await?;
        let config: WifiConfig = serde_json::from_value(value)?;
        // Fill names the roster already has without another router read.
        for device in &mut self.network_clients {
            if device.name.is_none() {
                device.name = config
                    .saved_name_for(device.client_id, Some(device.mac_address.as_str()))
                    .map(str::to_owned)
                    .or_else(|| device.hostname.clone());
            }
        }
        self.network_config = Some(config);
        self.network_config_dirty = false;
        Ok(())
    }

    async fn poll_network_radios(&mut self) -> Result<(), RunnerError> {
        let value = self.router.get_radio_stats().await?;
        let reply: RadioStatsReply = serde_json::from_value(value)?;
        self.network_radios = reply.readings();
        Ok(())
    }

    /// Count a failed roster read. The first miss leaves the last good snapshot
    /// on screen; the second marks the router unreachable and clears the roster.
    /// Returns true on the transition, which is the one failure that needs a
    /// snapshot delivered.
    // SPEC-NET-006: two consecutive misses are what mark the router unreachable.
    fn network_failed(&mut self) -> bool {
        self.network_misses = self.network_misses.saturating_add(1);
        if self.network_misses != 2 {
            return false;
        }
        self.network_reachable = false;
        self.network_clients.clear();
        self.network_tracker.reset();
        true
    }

    /// Poll history and fold it into the stores.
    ///
    /// A router failure is not fatal: the history still records, and its live
    /// router columns stay empty rather than carrying the last value forward.
    /// A dish failure is returned to the host, which decides whether to retry.
    pub async fn poll_once(&mut self) -> Result<PollReport, RunnerError> {
        self.poll_once_at(now_ms()).await
    }
    async fn poll_once_at(&mut self, at_ms: i64) -> Result<PollReport, RunnerError> {
        let history: DishHistory = serde_json::from_value(self.dish.get_history().await?)?;

        let mut router_alerts = None;
        let (readings, router_reachable) = match self.router.get_router_status().await {
            Ok(value) => match serde_json::from_value::<RouterStatus>(value) {
                Ok(status) => {
                    router_alerts = Some(Some(status.alerts.clone()));
                    (status.readings(), true)
                }
                Err(error) => {
                    // The transport answered, so the router is reachable even if
                    // this reply could not be typed.
                    tracing::warn!("router status unreadable: {error}");
                    (RouterReadings::default(), true)
                }
            },
            Err(error) => {
                tracing::warn!("router unreachable: {error}");
                router_alerts = Some(None);
                (RouterReadings::default(), false)
            }
        };

        let outcome = self.recorder.record_poll(&history, at_ms, readings)?;
        let latest_sample = self.recorder.latest_sample().cloned();
        Ok(PollReport {
            outcome,
            latest_sample,
            router_reachable,
            router_alerts,
        })
    }

    /// Fetch one status reply and record its alert edges. This is also where
    /// thermal episodes stop relying on a browser to notice them.
    async fn poll_status(&mut self, at_ms: i64) -> Result<DishStatus, RunnerError> {
        let value = self.dish.get_status().await?;
        let status: DishStatus = serde_json::from_value(value)?;
        self.recorder.record_thermal(&status.alerts, at_ms)?;
        Ok(status)
    }

    /// Fetch the live obstruction grid. The durable snapshot is written only
    /// when the store says an hourly one is due; the live reply still feeds the
    /// dome on every poll.
    async fn poll_obstruction(
        &mut self,
        at_ms: i64,
    ) -> Result<Option<DishObstructionMap>, RunnerError> {
        let value = self.dish.get_obstruction_map().await?;
        let map: DishObstructionMap = serde_json::from_value(value)?;
        if map.num_rows == 0 || map.num_rows != map.num_cols || map.snr.is_empty() {
            return Ok(None);
        }
        if self.recorder.obstruction().is_due(at_ms) {
            let snapshot = ObstructionSnapshot {
                taken_at_ms: at_ms,
                grid_size: map.num_rows,
                packed_cells: pack_cells(&map.snr),
                max_theta_deg: map.max_theta_deg,
            };
            self.recorder.record_obstruction(snapshot)?;
        }
        Ok(Some(map))
    }

    /// Flush the rolling sample window at shutdown.
    pub fn write_snapshot(&mut self) -> Result<(), RunnerError> {
        self.recorder.write_snapshot(now_ms())?;
        Ok(())
    }
}

/// Whether a router sits behind the dish, and whether it is bypassed, from a
/// status reply already in hand.
pub fn router_presence(status: &DishStatus, at_ms: i64) -> RouterPresence {
    let downstream: Vec<_> = status
        .downstream_routers
        .iter()
        .filter(|(_, router)| {
            router.last_seen.is_none_or(|seen| {
                let seen_ms = i64::try_from(seen.0 / 1_000_000).unwrap_or(i64::MAX);
                at_ms.saturating_sub(seen_ms) < 60_000
            })
        })
        .collect();
    if downstream.is_empty() {
        if status.downstream_routers.is_empty() && !status.connected_routers.is_empty() {
            RouterPresence::Present
        } else {
            RouterPresence::Absent
        }
    } else if downstream.iter().any(|(_, router)| {
        router
            .role
            .as_deref()
            .is_some_and(|role| role.eq_ignore_ascii_case("BYPASSED"))
    }) {
        RouterPresence::Bypassed
    } else {
        RouterPresence::Present
    }
}

fn observe_client_usage(
    core: &mut crate::odometer::ClientTotalsCore,
    client: &WifiClient,
    at_ms: i64,
    live_keys: &std::collections::HashSet<String>,
) {
    if !client.is_client() {
        return;
    }
    let Some(mac_address) = client.mac_address.as_deref() else {
        return;
    };
    let (Some(rx_bytes), Some(tx_bytes)) = (
        client
            .rx_stats
            .as_ref()
            .and_then(WifiClientStats::byte_count),
        client
            .tx_stats
            .as_ref()
            .and_then(WifiClientStats::byte_count),
    ) else {
        return;
    };

    core.observe(
        ClientReading {
            client_id: client.client_id,
            mac_address,
            rx_bytes,
            tx_bytes,
            at_ms,
            name: client.given_name.as_deref().or(client.name.as_deref()),
            ip_address: client.ip_address.as_deref(),
        },
        live_keys,
    );
}

fn warn_network(error: &RunnerError) {
    tracing::warn!("network poll failed: {error}");
}

/// Is a poll of this interval due at `at_ms`? `None` means never polled, so the
/// first tick fires all three.
pub fn due(last_ms: Option<i64>, at_ms: i64, interval: std::time::Duration) -> bool {
    last_ms.is_none_or(|last| at_ms.saturating_sub(last) >= interval.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::odometer::{ClientReading, ClientTotalsCore};
    use std::time::Duration;

    // SPEC-SAFETY-002, SPEC-SAFETY-003, SPEC-ALERT-004: alerts keep the
    // approved cadence and demand-gated request budget unchanged.
    #[test]
    fn the_runner_uses_the_approved_cadences() {
        assert_eq!(crate::HISTORIAN_POLL_INTERVAL, Duration::from_secs(5));
        assert_eq!(crate::STATUS_POLL_INTERVAL, Duration::from_secs(2));
        assert_eq!(crate::OBSTRUCTION_POLL_INTERVAL, Duration::from_secs(30));
        assert_eq!(crate::NETWORK_CLIENTS_POLL_INTERVAL, Duration::from_secs(5));
        assert_eq!(crate::NETWORK_RADIO_POLL_INTERVAL, Duration::from_secs(15));
        assert_eq!(
            crate::NETWORK_CONFIG_RETRY_INTERVAL,
            Duration::from_secs(30)
        );
        // SPEC-DISHCFG-005, SPEC-SAFETY-005: the expensive settings snapshot
        // is demand-gated and refreshed no faster than once per minute.
        assert_eq!(crate::DISH_CONFIG_POLL_INTERVAL, Duration::from_secs(60));
    }

    // SPEC-ALERT-004: router presence comes from the existing dish status reply.
    #[test]
    fn existing_dish_status_classifies_router_presence_for_alerts() {
        use crate::telemetry::{DownstreamRouter, U64Value};

        let now_ms = 1_781_524_800_000_i64;
        let mut status = DishStatus::default();
        assert_eq!(router_presence(&status, now_ms), RouterPresence::Absent);

        status.connected_routers.push("router".into());
        assert_eq!(router_presence(&status, now_ms), RouterPresence::Present);
        status.connected_routers.clear();

        status.downstream_routers.insert(
            "router".into(),
            DownstreamRouter {
                role: Some("BYPASSED".into()),
                last_seen: Some(U64Value((now_ms as u64) * 1_000_000)),
            },
        );
        assert_eq!(router_presence(&status, now_ms), RouterPresence::Bypassed);
        status.downstream_routers.get_mut("router").unwrap().role = Some("PRIMARY".into());
        assert_eq!(router_presence(&status, now_ms), RouterPresence::Present);
        status
            .downstream_routers
            .get_mut("router")
            .unwrap()
            .last_seen = Some(U64Value(((now_ms - 60_000) as u64) * 1_000_000));
        assert_eq!(router_presence(&status, now_ms), RouterPresence::Absent);
    }

    // SPEC-CTRL-007: the host preference is the endpoint used by the same
    // recorder client that owns network polling and router controls.
    #[test]
    fn configured_router_endpoint_is_used_and_can_be_reset() {
        let directory = tempfile::tempdir().unwrap();
        let mut runner =
            RecorderRunner::open_with_router_address(directory.path(), Some("10.0.0.1")).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            "http://10.0.0.1:9001/SpaceX.API.Device.Device/Handle"
        );
        runner.set_router_address(Some("2001:db8::1")).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            "http://[2001:db8::1]:9001/SpaceX.API.Device.Device/Handle"
        );
        runner.set_router_address(None).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            crate::client::router_handle_url(
                &crate::gateway::default_router_address().to_string(),
                None
            )
        );
    }

    // SPEC-CTRL-010, SPEC-CTRL-011: the access port rides the same endpoint
    // handling as the address. A port-only save works against the discovered
    // gateway, keeps the saved address on the next address save, and clearing
    // the port returns to the device port without adding a poll.
    #[test]
    fn router_access_port_selects_the_lan_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let mut runner = RecorderRunner::open_with_router_endpoint(
            directory.path(),
            Some("10.0.0.1"),
            Some(42622),
        )
        .unwrap();
        assert_eq!(
            runner.router.handle_url(),
            "http://10.0.0.1:42622/SpaceX.API.Device.Device/Handle"
        );
        // An address-only save keeps the forwarded port.
        runner.set_router_address(Some("10.0.0.2")).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            "http://10.0.0.2:42622/SpaceX.API.Device.Device/Handle"
        );
        // Clearing the port returns to the device port, address untouched.
        runner.set_router_port(None).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            "http://10.0.0.2:9001/SpaceX.API.Device.Device/Handle"
        );
        // A port-only save reaches the current discovered gateway.
        runner.set_router_port(Some(42622)).unwrap();
        let gateway = crate::gateway::default_router_address().to_string();
        runner.set_router_address(None).unwrap();
        assert_eq!(
            runner.router.handle_url(),
            format!("http://{gateway}:42622/SpaceX.API.Device.Device/Handle")
        );
    }

    // SPEC-CTRL-011: the endpoint client resolves each missing dimension to its
    // LAN default inside `client::router_client`; the runner only remembers the
    // two saved dimensions, so a missing one keeps its default.
    #[test]
    fn open_without_an_address_uses_the_default_gateway_with_the_port() {
        let directory = tempfile::tempdir().unwrap();
        let runner =
            RecorderRunner::open_with_router_endpoint(directory.path(), None, Some(42622)).unwrap();
        let gateway = crate::gateway::default_router_address().to_string();
        assert_eq!(
            runner.router.handle_url(),
            format!("http://{gateway}:42622/SpaceX.API.Device.Device/Handle")
        );
    }

    // SPEC-DISHCFG-005, SPEC-SAFETY-005: the settings snapshot has its own
    // visibility demand gate; merely starting the recorder does not activate it.
    #[test]
    fn dish_config_reads_are_settings_demand_gated() {
        let directory = tempfile::tempdir().unwrap();
        let mut runner = RecorderRunner::open(directory.path()).unwrap();
        assert!(!runner.dish_config_wanted);
        runner.set_dish_config_wanted(true);
        assert!(runner.dish_config_wanted);
        runner.set_dish_config_wanted(false);
        assert!(!runner.dish_config_wanted);
    }

    // SPEC-METER-ALERT-005: evaluating a local monthly cap uses persisted
    // recorder state and does not activate or execute a router poll.
    #[test]
    fn monthly_limit_alert_evaluation_reuses_odometer_without_polling() {
        let directory = tempfile::tempdir().unwrap();
        let mut runner = RecorderRunner::open(directory.path()).unwrap();
        let at_ms = now_ms();
        let mac = "10:00:20:11:22:33";
        let live_keys = runner
            .recorder
            .odometer_mut()
            .core_mut()
            .note_poll(&[(Some(42), mac)]);
        for (offset, rx_bytes, tx_bytes) in [(0, 0, 0), (1_000, 100, 50)] {
            runner.recorder.odometer_mut().core_mut().observe(
                ClientReading {
                    client_id: Some(42),
                    mac_address: mac,
                    rx_bytes,
                    tx_bytes,
                    at_ms: at_ms + offset,
                    name: Some("Phone"),
                    ip_address: None,
                },
                &live_keys,
            );
        }
        runner
            .set_client_monthly_limit(ClientMonthlyLimit::new("42", 150).unwrap())
            .unwrap();

        let snapshot = runner
            .evaluate_monthly_limit_alerts(at_ms + 1_000)
            .unwrap()
            .expect("limit crossing");
        assert_eq!(snapshot.transitions.len(), 1);
        assert_eq!(snapshot.transitions[0].label.as_deref(), Some("Phone"));
        assert_eq!(snapshot.history[0].label.as_deref(), Some("Phone"));
        assert!(!runner.network_wanted);
        assert!(runner.last_network_clients_ms.is_none());
        assert!(
            runner
                .evaluate_monthly_limit_alerts(at_ms + 2_000)
                .unwrap()
                .is_none()
        );
    }

    // SPEC-RULE-008: rules are evaluated from the odometer the recorder already
    // holds, announce under their own key, and start no device poll.
    #[test]
    fn rule_evaluation_reuses_odometer_without_polling() {
        let directory = tempfile::tempdir().unwrap();
        let mut runner = RecorderRunner::open(directory.path()).unwrap();
        let at_ms = now_ms();
        let mac = "10:00:20:11:22:44";
        let live_keys = runner
            .recorder
            .odometer_mut()
            .core_mut()
            .note_poll(&[(Some(43), mac)]);
        let observe = |runner: &mut RecorderRunner, offset: i64, rx_bytes: u64| {
            runner.recorder.odometer_mut().core_mut().observe(
                ClientReading {
                    client_id: Some(43),
                    mac_address: mac,
                    rx_bytes,
                    tx_bytes: 0,
                    at_ms: at_ms + offset,
                    name: Some("Tablet"),
                    ip_address: None,
                },
                &live_keys,
            );
        };
        observe(&mut runner, 0, 0);
        runner
            .recorder
            .upsert_device_rule(crate::data_meter::MeterRuleTerms::new(
                "43",
                100,
                crate::data_meter::MeterCycle::Daily,
                0,
                0,
                at_ms,
            ))
            .unwrap();
        observe(&mut runner, 1_000, 150);

        let snapshot = runner
            .evaluate_monthly_limit_alerts(at_ms + 1_000)
            .unwrap()
            .expect("rule reached");
        assert_eq!(snapshot.transitions[0].key, "dataLimit:43");
        let label = crate::data_meter::DataLimitLabel::parse(
            snapshot.transitions[0].label.as_deref().unwrap_or_default(),
        )
        .expect("structured label");
        assert_eq!(
            (label.subject.as_str(), label.detail.as_str()),
            ("Tablet", "0.0 GB")
        );
        assert!(!runner.network_wanted);
        assert!(runner.last_network_clients_ms.is_none());
    }

    // SPEC-SAFETY-002: a poll fires at its interval, never before, and an
    // unaired poll fires on the first tick.
    #[test]
    fn a_poll_is_due_only_at_its_interval() {
        let now = 1_781_524_800_000;
        assert!(due(None, now, crate::STATUS_POLL_INTERVAL));
        assert!(due(Some(now), now + 2_000, crate::STATUS_POLL_INTERVAL));
        assert!(!due(Some(now), now + 1_999, crate::STATUS_POLL_INTERVAL));
        assert!(!due(
            Some(now),
            now + 29_999,
            crate::OBSTRUCTION_POLL_INTERVAL
        ));
        assert!(due(
            Some(now),
            now + 30_000,
            crate::OBSTRUCTION_POLL_INTERVAL
        ));
    }

    // SPEC-ODO-007, SPEC-ODO-010: missing counters and non-client entries are ignored.
    #[test]
    fn incomplete_client_counters_do_not_mutate_usage_state() {
        let mut core = ClientTotalsCore::with_options(15_000, true);
        let mac = "10:00:20:11:22:33";
        let live = core.note_poll(&[(Some(7), mac)]);
        core.observe(
            ClientReading {
                client_id: Some(7),
                mac_address: mac,
                rx_bytes: 1_000,
                tx_bytes: 500,
                at_ms: 1_781_524_800_000,
                name: Some("Work laptop"),
                ip_address: Some("192.168.1.20"),
            },
            &live,
        );

        let client = WifiClient {
            client_id: Some(7),
            mac_address: Some(mac.to_owned()),
            name: Some("Renamed".into()),
            ip_address: Some("192.168.1.21".into()),
            rx_stats: Some(WifiClientStats {
                bytes: Some("0".into()),
                ..WifiClientStats::default()
            }),
            tx_stats: None,
            ..WifiClient::default()
        };
        observe_client_usage(&mut core, &client, 1_781_524_805_000, &live);

        let total = core.totals(None).pop().expect("initial total remains");
        assert_eq!(total.rx_bytes, 0);
        assert_eq!(total.tx_bytes, 0);
        assert_eq!(total.last_seen_ms, 1_781_524_800_000);
        assert_eq!(total.name.as_deref(), Some("Work laptop"));
        assert_eq!(total.ip_address.as_deref(), Some("192.168.1.20"));
    }

    // SPEC-ODO-010: router infrastructure does not become a usage device.
    #[test]
    fn non_client_roster_entries_are_excluded_from_usage() {
        let mut core = ClientTotalsCore::with_options(15_000, true);
        let client = WifiClient {
            client_id: Some(99),
            mac_address: Some("60:74:f4:aa:bb:01".into()),
            role: Some("REPEATER".into()),
            rx_stats: Some(WifiClientStats {
                bytes: Some("500".into()),
                ..WifiClientStats::default()
            }),
            tx_stats: Some(WifiClientStats {
                bytes: Some("200".into()),
                ..WifiClientStats::default()
            }),
            ..WifiClient::default()
        };
        let live = core.note_poll(&[]);
        observe_client_usage(&mut core, &client, 1_781_524_800_000, &live);
        assert!(core.totals(None).is_empty());
    }

    // SPEC-ODO-010: the saved router name is the display name, ahead of hostname.
    #[test]
    fn usage_keeps_the_router_given_name() {
        let mut core = ClientTotalsCore::with_options(15_000, true);
        let client = WifiClient {
            client_id: Some(100),
            mac_address: Some("10:00:20:11:22:33".into()),
            name: Some("workstation.local".into()),
            given_name: Some("Design Mac".into()),
            rx_stats: Some(WifiClientStats {
                bytes: Some("500".into()),
                ..WifiClientStats::default()
            }),
            tx_stats: Some(WifiClientStats {
                bytes: Some("200".into()),
                ..WifiClientStats::default()
            }),
            ..WifiClient::default()
        };
        let live = core.note_poll(&[(Some(100), "10:00:20:11:22:33")]);
        observe_client_usage(&mut core, &client, 1_781_524_800_000, &live);
        assert_eq!(core.totals(None)[0].name.as_deref(), Some("Design Mac"));
    }
}
