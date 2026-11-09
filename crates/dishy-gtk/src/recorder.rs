//! The recorder as a background thread, owned by the app rather than a window.
//!
//! Hiding or closing the window must not stop collection. This thread starts at
//! application startup, uses the shared [`RecorderRunner`], and is stopped only
//! by application shutdown. Its output channel is how the tray learns the latest
//! throughput without touching the network or the stores itself.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use dishy_core::alert::AlertSnapshot;
use dishy_core::calendar::now_ms;
use dishy_core::control::ControlAction;
use dishy_core::data_meter::{MeterRule, MeterRuleTerms};
use dishy_core::device_group::DeviceGroup;
use dishy_core::meter::ClientMonthlyLimit;
use dishy_core::network::NetworkSnapshot;
use dishy_core::odometer::ClientTotal;
use dishy_core::runner::RecorderRunner;
use dishy_core::telemetry::{DishObstructionMap, DishStatus, TelemetrySample};
use dishy_core::usage::{UsageRange, UsageSummary};
use tokio::sync::watch;

use crate::throughput::ThroughputSample;

/// How often the host checks whether a poll is due. Each poll keeps its own
/// approved cadence inside `RecorderRunner::tick`.
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Work the UI asks the one recorder process to do. The recorder owns the data
/// directory lock, so a page cannot open the stores itself.
#[derive(Debug, Clone)]
pub enum RecorderCommand {
    /// Summarise the stored traffic for one of the shared ranges.
    Usage(UsageRange),
    /// Open or close the network page's demand gate.
    NetworkDemand(bool),
    /// Open or close the Settings page's dish-config read gate.
    DishConfigDemand(bool),
    /// Send one confirmed maintenance write through the recorder thread.
    Control(ControlAction),
    /// Fetch support diagnostics once, only after the user requests a copy.
    GetDiagnostics,
    /// Open or close the device usage page's demand gate.
    DeviceUsageDemand(bool),
    /// Request current client totals from the odometer.
    GetClientTotals,
    /// Store a watch-only local-calendar-month device cap.
    SetClientMonthlyLimit {
        client_key: String,
        limit_bytes: u64,
    },
    /// Remove one device's watch-only monthly cap.
    RemoveClientMonthlyLimit(String),
    /// Reset one device's current month usage.
    ResetClientTotal(String),
    /// Remove one device's record entirely.
    RemoveClientTotal(String),
    /// Clear all device usage records for this month.
    ClearClientTotals,
    /// Change the router endpoint without requiring the old endpoint to answer.
    SetRouterAddress(Option<String>),
    /// Change the router access port, keeping the saved address.
    SetRouterPort(Option<u16>),
    /// Read the metering rules and groups (spec 0170).
    GetRules,
    /// Energy over a range for an open tile detail (spec 0172). Its own
    /// command so the Data Usage page's summary is left alone.
    DetailEnergy(UsageRange),
    /// Latency quality over a range for the latency detail (spec 0172).
    LatencySummary(UsageRange),
    /// The dish event log and outage reports, for the Events tab (spec 0174).
    GetEvents,
    /// One device's throughput history, for its detail (spec 0175).
    GetDeviceThroughput(String),
    /// The hourly obstruction snapshots, for the time-lapse (spec 0179).
    GetObstructionSnapshots,
    /// The user's answer to a duplicate-device proposal (spec 0176).
    MergeClients {
        from: String,
        to: String,
    },
    RejectMerge {
        a: String,
        b: String,
    },
    /// Write a device's own rule, keeping the anchors of one it has.
    UpsertDeviceRule(MeterRuleTerms),
    RemoveDeviceRule(String),
    /// Add or replace a group and project its members' rules.
    UpsertGroup(DeviceGroup),
    RemoveGroup(String),
    /// Start one rule over from now.
    RestartRule {
        client_key: String,
        group_id: Option<String>,
    },
}

/// The event log and the outage reports, as the recorder holds them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventsSnapshot {
    pub events: Vec<dishy_core::telemetry::OutageEvent>,
    pub reports: Vec<dishy_core::postmortem::OutageReport>,
}

/// The rules and groups as the recorder holds them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RulesSnapshot {
    pub rules: Vec<MeterRule>,
    pub groups: Vec<DeviceGroup>,
}

/// The outcome of a maintenance write, so the page can report exactly what was
/// sent or why it was not.
#[derive(Debug, Clone)]
pub struct ControlReport {
    pub action: ControlAction,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone)]
pub enum RecorderUpdate {
    /// The newest sample in the rolling window, for the tray readout.
    Sample(ThroughputSample),
    /// The dashboard's view of the recorder's whole rolling window. Shared as an
    /// `Arc` so the GTK thread takes a handle, not another 21,600-row copy.
    Samples(Arc<Vec<TelemetrySample>>),
    /// The latest dish status reply: boresight, obstruction stats, thermal
    /// flags, and device identity all ride this one poll.
    Status(Arc<DishStatus>),
    /// The latest live obstruction grid, for the sky view.
    Obstruction(Arc<DishObstructionMap>),
    /// A data-usage summary, computed in the recorder thread on request.
    Usage(Arc<UsageSummary>),
    /// The network page's latest roster, config, and radios.
    Network(Arc<NetworkSnapshot>),
    /// Dish configuration returned by the recorder-owned LAN client.
    DishConfig(Arc<serde_json::Value>),
    /// Whether the router answered the status read the history cycle already
    /// makes. Changes only when that observation changes.
    RouterReachable(bool),
    /// The result of a maintenance write.
    Control(ControlReport),
    /// The result of the one-shot support diagnostics read.
    Diagnostics(Result<(serde_json::Value, serde_json::Value), String>),
    /// Latest per-device usage totals from the odometer.
    ClientTotals(Arc<Vec<ClientTotal>>),
    /// Latest watch-only per-device monthly limits.
    ClientMonthlyLimits(Arc<Vec<ClientMonthlyLimit>>),
    /// Latest limits after a successful limit write or usage-record cleanup.
    ClientMonthlyLimitsChanged(Arc<Vec<ClientMonthlyLimit>>),
    /// A local monthly-limit write failed and was not reported as successful.
    MonthlyLimitWriteFailed(String),
    /// Latest alert state from the existing dish and router status replies.
    Alerts(Arc<AlertSnapshot>),
    /// The metering rules and groups, after a read or a successful write.
    Rules(Arc<RulesSnapshot>),
    DetailEnergy(Arc<UsageSummary>),
    LatencySummary(Arc<dishy_core::latency_summary::LatencySummary>),
    Events(Arc<EventsSnapshot>),
    MergeCandidates(Arc<Vec<dishy_core::odometer::MergeCandidate>>),
    ObstructionSnapshots(Arc<Vec<dishy_core::store::ObstructionSnapshot>>),
    DeviceThroughput {
        key: String,
        samples: Arc<Vec<dishy_core::client_window::ClientSample>>,
    },
    /// A rule write failed and was not applied.
    RuleWriteFailed(String),
    /// The recorder could not start (usually another writer owns the directory).
    Failed(String),
}

pub struct RecorderThread {
    stop: watch::Sender<bool>,
    commands: Sender<RecorderCommand>,
    join: Option<JoinHandle<()>>,
}

impl RecorderThread {
    pub fn start(data_dir: PathBuf) -> std::io::Result<(Self, Receiver<RecorderUpdate>)> {
        Self::start_with_router_address(data_dir, None)
    }

    pub fn start_with_router_address(
        data_dir: PathBuf,
        router_address: Option<String>,
    ) -> std::io::Result<(Self, Receiver<RecorderUpdate>)> {
        Self::start_with_router_endpoint(data_dir, router_address, None)
    }

    pub fn start_with_router_endpoint(
        data_dir: PathBuf,
        router_address: Option<String>,
        router_port: Option<u16>,
    ) -> std::io::Result<(Self, Receiver<RecorderUpdate>)> {
        let (stop, stop_rx) = watch::channel(false);
        let (updates, receiver) = mpsc::channel();
        let (commands, command_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("dishy-recorder".into())
            .spawn(move || {
                run(
                    data_dir,
                    router_address,
                    router_port,
                    stop_rx,
                    command_rx,
                    updates,
                )
            })?;
        Ok((
            Self {
                stop,
                commands,
                join: Some(join),
            },
            receiver,
        ))
    }

    /// Cloneable handle for pages that need the recorder's stores.
    pub fn command_sender(&self) -> Sender<RecorderCommand> {
        self.commands.clone()
    }

    pub fn stop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RecorderThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    data_dir: PathBuf,
    router_address: Option<String>,
    router_port: Option<u16>,
    mut stop: watch::Receiver<bool>,
    commands: Receiver<RecorderCommand>,
    updates: Sender<RecorderUpdate>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = updates.send(RecorderUpdate::Failed(error.to_string()));
            return;
        }
    };

    runtime.block_on(async move {
        let mut runner = match RecorderRunner::open_with_router_endpoint(
            &data_dir,
            router_address.as_deref(),
            router_port,
        ) {
            Ok(runner) => runner,
            Err(error) => {
                tracing::error!("recorder cannot open {}: {error}", data_dir.display());
                let _ = updates.send(RecorderUpdate::Failed(error.to_string()));
                return;
            }
        };
        tracing::info!("recording to {}", runner.data_dir().display());
        // The recorder opened with the persisted rolling snapshot. Hand it to
        // the dashboard now, not after the first network poll, so opening the
        // window after a quiet start still shows the last known six hours.
        let _ = updates.send(RecorderUpdate::Samples(Arc::new(
            runner.recorder().samples().to_vec(),
        )));

        let mut last_router_reachable = None;
        loop {
            while let Ok(command) = commands.try_recv() {
                match command {
                    RecorderCommand::Usage(range) => {
                        match runner.recorder().usage_summary(range, now_ms()) {
                            Ok(summary) => {
                                let _ = updates.send(RecorderUpdate::Usage(Arc::new(summary)));
                            }
                            Err(error) => {
                                tracing::warn!("usage summary: {error}");
                            }
                        }
                    }
                    RecorderCommand::NetworkDemand(wanted) => {
                        runner.set_network_wanted(wanted);
                    }
                    RecorderCommand::DishConfigDemand(wanted) => {
                        runner.set_dish_config_wanted(wanted);
                    }
                    RecorderCommand::SetRouterAddress(address) => {
                        if let Err(error) = runner.set_router_address(address.as_deref()) {
                            tracing::warn!("could not apply router address: {error}");
                        }
                    }
                    RecorderCommand::SetRouterPort(port) => {
                        if let Err(error) = runner.set_router_port(port) {
                            tracing::warn!("could not apply router port: {error}");
                        }
                    }
                    RecorderCommand::Control(action) => {
                        // SPEC-CTRL-002, SPEC-CTRL-006: the one write path, and
                        // the failure is carried back rather than swallowed.
                        let result = runner
                            .control(action)
                            .await
                            .map_err(|error| error.to_string());
                        let _ =
                            updates.send(RecorderUpdate::Control(ControlReport { action, result }));
                    }
                    RecorderCommand::GetDiagnostics => {
                        // SPEC-DIAG-002: this command is the only path to these
                        // reads; they are deliberately absent from `tick`.
                        let result = runner
                            .diagnostics()
                            .await
                            .map_err(|error| error.to_string());
                        if let Err(error) = &result {
                            tracing::warn!("could not collect dish diagnostics: {error}");
                        }
                        let _ = updates.send(RecorderUpdate::Diagnostics(result));
                    }
                    RecorderCommand::DeviceUsageDemand(wanted) => {
                        runner.set_device_usage_wanted(wanted);
                        if wanted {
                            let totals = runner.recorder().client_totals(None);
                            let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                            send_merge_candidates(&runner, &updates);
                            publish_monthly_limit_alerts(&mut runner, &updates);
                        }
                    }
                    RecorderCommand::GetRules => publish_rules(&runner, &updates),
                    RecorderCommand::GetDeviceThroughput(key) => {
                        let since = now_ms() - dishy_core::client_window::WINDOW_MS;
                        let samples = runner.recorder().client_window().samples(&key, since);
                        let _ = updates.send(RecorderUpdate::DeviceThroughput {
                            key,
                            samples: Arc::new(samples),
                        });
                    }
                    RecorderCommand::GetObstructionSnapshots => {
                        let snapshots = runner.recorder().obstruction().list().to_vec();
                        let _ =
                            updates.send(RecorderUpdate::ObstructionSnapshots(Arc::new(snapshots)));
                    }
                    RecorderCommand::MergeClients { from, to } => {
                        runner.recorder_mut().merge_clients(&from, &to);
                        let totals = runner.recorder().client_totals(None);
                        let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                        send_merge_candidates(&runner, &updates);
                    }
                    RecorderCommand::RejectMerge { a, b } => {
                        runner.recorder_mut().reject_merge(&a, &b);
                        let totals = runner.recorder().client_totals(None);
                        let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                        send_merge_candidates(&runner, &updates);
                    }
                    RecorderCommand::GetEvents => {
                        let now = now_ms();
                        let _ = updates.send(RecorderUpdate::Events(Arc::new(EventsSnapshot {
                            events: runner.recorder().events().all(now),
                            reports: runner.recorder().outage_reports(now),
                        })));
                    }
                    RecorderCommand::DetailEnergy(range) => {
                        match runner.recorder().usage_summary(range, now_ms()) {
                            Ok(summary) => {
                                let _ =
                                    updates.send(RecorderUpdate::DetailEnergy(Arc::new(summary)));
                            }
                            Err(error) => tracing::warn!("detail energy summary: {error}"),
                        }
                    }
                    RecorderCommand::LatencySummary(range) => {
                        match runner.recorder().latency_summary(range, now_ms()) {
                            Ok(summary) => {
                                let _ =
                                    updates.send(RecorderUpdate::LatencySummary(Arc::new(summary)));
                            }
                            Err(error) => tracing::warn!("latency summary: {error}"),
                        }
                    }
                    RecorderCommand::UpsertDeviceRule(terms) => {
                        let result = runner.recorder_mut().upsert_device_rule(terms);
                        finish_rule_write(&mut runner, &updates, result.map(|_| ()));
                    }
                    RecorderCommand::RemoveDeviceRule(client_key) => {
                        let result = runner.recorder_mut().remove_device_rule(&client_key);
                        finish_rule_write(&mut runner, &updates, result.map(|_| ()));
                    }
                    RecorderCommand::UpsertGroup(group) => {
                        let result = runner.recorder_mut().upsert_group(group, now_ms());
                        finish_rule_write(&mut runner, &updates, result);
                    }
                    RecorderCommand::RemoveGroup(group_id) => {
                        let result = runner.recorder_mut().remove_group(&group_id);
                        finish_rule_write(&mut runner, &updates, result.map(|_| ()));
                    }
                    RecorderCommand::RestartRule {
                        client_key,
                        group_id,
                    } => {
                        let result = runner.recorder_mut().restart_rule(
                            &client_key,
                            group_id.as_deref(),
                            now_ms(),
                        );
                        finish_rule_write(&mut runner, &updates, result.map(|_| ()));
                    }
                    RecorderCommand::GetClientTotals => {
                        let totals = runner.recorder().client_totals(None);
                        let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                        send_merge_candidates(&runner, &updates);
                        let limits = runner.recorder().client_monthly_limits();
                        let _ = updates.send(RecorderUpdate::ClientMonthlyLimits(Arc::new(limits)));
                        publish_monthly_limit_alerts(&mut runner, &updates);
                    }
                    RecorderCommand::SetClientMonthlyLimit {
                        client_key,
                        limit_bytes,
                    } => {
                        let result = ClientMonthlyLimit::new(client_key, limit_bytes)
                            .map_err(|error| error.to_string())
                            .and_then(|limit| {
                                runner
                                    .set_client_monthly_limit(limit)
                                    .map_err(|error| error.to_string())
                            });
                        match result {
                            Ok(()) => {
                                let limits = runner.recorder().client_monthly_limits();
                                let _ = updates.send(RecorderUpdate::ClientMonthlyLimitsChanged(
                                    Arc::new(limits),
                                ));
                                publish_monthly_limit_alerts(&mut runner, &updates);
                            }
                            Err(error) => {
                                let _ =
                                    updates.send(RecorderUpdate::MonthlyLimitWriteFailed(error));
                            }
                        }
                    }
                    RecorderCommand::RemoveClientMonthlyLimit(client_key) => {
                        match runner.remove_client_monthly_limit(&client_key) {
                            Ok(_) => {
                                let limits = runner.recorder().client_monthly_limits();
                                let _ = updates.send(RecorderUpdate::ClientMonthlyLimitsChanged(
                                    Arc::new(limits),
                                ));
                                publish_monthly_limit_alerts(&mut runner, &updates);
                            }
                            Err(error) => {
                                let _ = updates.send(RecorderUpdate::MonthlyLimitWriteFailed(
                                    error.to_string(),
                                ));
                            }
                        }
                    }
                    RecorderCommand::ResetClientTotal(key) => {
                        runner.reset_client_total(&key, now_ms());
                        let totals = runner.recorder().client_totals(None);
                        let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                        send_merge_candidates(&runner, &updates);
                        publish_monthly_limit_alerts(&mut runner, &updates);
                    }
                    RecorderCommand::RemoveClientTotal(key) => {
                        match runner.remove_client_total(&key) {
                            Ok(_) => {
                                let totals = runner.recorder().client_totals(None);
                                let _ =
                                    updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                                send_merge_candidates(&runner, &updates);
                                let limits = runner.recorder().client_monthly_limits();
                                let _ = updates.send(RecorderUpdate::ClientMonthlyLimitsChanged(
                                    Arc::new(limits),
                                ));
                                publish_monthly_limit_alerts(&mut runner, &updates);
                            }
                            Err(error) => {
                                let _ = updates.send(RecorderUpdate::MonthlyLimitWriteFailed(
                                    error.to_string(),
                                ));
                            }
                        }
                    }
                    RecorderCommand::ClearClientTotals => match runner.clear_client_totals() {
                        Ok(()) => {
                            let totals = runner.recorder().client_totals(None);
                            let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                            send_merge_candidates(&runner, &updates);
                            let limits = runner.recorder().client_monthly_limits();
                            let _ = updates
                                .send(RecorderUpdate::ClientMonthlyLimitsChanged(Arc::new(limits)));
                            publish_monthly_limit_alerts(&mut runner, &updates);
                        }
                        Err(error) => {
                            let _ = updates
                                .send(RecorderUpdate::MonthlyLimitWriteFailed(error.to_string()));
                        }
                    },
                }
            }

            let report = runner.tick(now_ms()).await;
            if let Some(network) = report.network {
                let _ = updates.send(RecorderUpdate::Network(Arc::new(network)));
            }
            if let Some(config) = report.dish_config {
                let _ = updates.send(RecorderUpdate::DishConfig(Arc::new(config)));
            }
            if let Some(totals) = report.client_totals {
                let _ = updates.send(RecorderUpdate::ClientTotals(Arc::new(totals)));
                send_merge_candidates(&runner, &updates);
            }
            if let Some(alerts) = report.alerts {
                let _ = updates.send(RecorderUpdate::Alerts(Arc::new(alerts)));
            }
            if let Some(poll) = report.history {
                if last_router_reachable != Some(poll.router_reachable) {
                    last_router_reachable = Some(poll.router_reachable);
                    let _ = updates.send(RecorderUpdate::RouterReachable(poll.router_reachable));
                }
                let _ = updates.send(RecorderUpdate::Samples(Arc::new(
                    runner.recorder().samples().to_vec(),
                )));
                if let Some(sample) = poll.latest_sample {
                    let _ = updates.send(RecorderUpdate::Sample(ThroughputSample {
                        at_ms: sample.timestamp_ms,
                        down_bps: sample.downlink_bps,
                        up_bps: sample.uplink_bps,
                    }));
                }
                tracing::info!(
                    "{} new sample(s), {} energy and {} latency minute(s), {} event(s)",
                    poll.outcome.fresh_samples,
                    poll.outcome.energy_minutes_written.len(),
                    poll.outcome.latency_minutes_written.len(),
                    poll.outcome.new_events
                );
            }
            if let Some(status) = report.status {
                let _ = updates.send(RecorderUpdate::Status(Arc::new(status)));
            }
            if let Some(map) = report.obstruction {
                let _ = updates.send(RecorderUpdate::Obstruction(Arc::new(map)));
            }
            if let Some(error) = report.history_error {
                tracing::warn!("poll failed: {error}");
            }

            tokio::select! {
                () = tokio::time::sleep(TICK_INTERVAL) => {}
                _ = stop.changed() => break,
            }
        }

        if let Err(error) = runner.write_snapshot() {
            tracing::warn!("recorder snapshot on shutdown: {error}");
        }
    });
}

/// Duplicate-device proposals ride with every totals update (spec 0176).
fn send_merge_candidates(runner: &RecorderRunner, updates: &Sender<RecorderUpdate>) {
    let candidates = runner
        .recorder()
        .odometer()
        .core()
        .merge_candidates(now_ms());
    let _ = updates.send(RecorderUpdate::MergeCandidates(Arc::new(candidates)));
}

fn publish_rules(runner: &RecorderRunner, updates: &Sender<RecorderUpdate>) {
    let meters = runner.recorder().meters();
    let _ = updates.send(RecorderUpdate::Rules(Arc::new(RulesSnapshot {
        rules: meters.rules().to_vec(),
        groups: meters.groups().to_vec(),
    })));
}

/// Report a rule write: the new rules, the limits they imply, and any alert
/// they cross, or the failure, so nothing unsaved is shown as saved.
fn finish_rule_write(
    runner: &mut RecorderRunner,
    updates: &Sender<RecorderUpdate>,
    result: Result<(), dishy_core::recorder::RecorderError>,
) {
    match result {
        Ok(()) => {
            publish_rules(runner, updates);
            let limits = runner.recorder().client_monthly_limits();
            let _ = updates.send(RecorderUpdate::ClientMonthlyLimitsChanged(Arc::new(limits)));
            publish_monthly_limit_alerts(runner, updates);
        }
        Err(error) => {
            let _ = updates.send(RecorderUpdate::RuleWriteFailed(error.to_string()));
        }
    }
}

fn publish_monthly_limit_alerts(runner: &mut RecorderRunner, updates: &Sender<RecorderUpdate>) {
    match runner.evaluate_monthly_limit_alerts(now_ms()) {
        Ok(Some(snapshot)) => {
            let _ = updates.send(RecorderUpdate::Alerts(Arc::new(snapshot)));
        }
        Ok(None) => {}
        Err(error) => tracing::warn!("monthly data-limit alert update failed: {error}"),
    }
}
