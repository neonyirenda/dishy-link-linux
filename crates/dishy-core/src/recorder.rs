//! The recorder: folds polls into the durable stores.
//!
//! This is the host-independent half of upstream `collector/historian.mts`. It owns the
//! data directory, the telemetry accumulator, the open (in-progress) minute
//! buckets, and the stores. Network polling and the HTTP API live above it.
//!
//! Honesty about gaps: only minutes present in the dish's ring window are
//! folded, and only completed minutes are written. A gap is therefore fewer
//! sample counts, never invented power or traffic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::alert::{AlertStore, history_path};
use crate::calendar::{Calendar, LocalCalendar, now_ms};
use crate::data_meter::{MeterRuleTerms, MeterTransition};
use crate::device_group::DeviceGroup;
use crate::energy::{MinuteBucket, fold_samples_to_minutes};
use crate::jsonl::JsonlError;
use crate::latency::{LatencyMinuteBucket, fold_samples_to_latency_minutes};
use crate::lock::{DataDirLock, LockError};
use crate::meter::ClientMonthlyLimit;
use crate::meter_store::MeterStore;
use crate::odometer::{ClientTotal, ClientTotalsStore};
use crate::store::{
    EnergyStore, EventStore, LatencyStore, ObstructionSnapshot, ObstructionStore, SamplesStore,
    StoreResult, ThermalStore,
};
use crate::telemetry::{
    DishHistory, RouterReadings, TelemetryAccumulator, TelemetrySample, decode_history_window,
    decode_outage_events,
};
use crate::usage::{UsageRange, UsageSummary, summarize_usage, usage_range_bounds};

/// The rolling full-resolution window served to the frontend.
pub const SAMPLE_WINDOW_MS: i64 = 6 * 3_600_000;
/// How often the snapshot is rewritten while running.
pub const SNAPSHOT_EVERY_MS: i64 = 60_000;
/// Minute detail is compacted into months once a day for a long-lived process.
pub const COMPACT_EVERY_MS: i64 = 24 * 3_600_000;

/// Thermal flags on `get_status` that the dish has no history for.
pub const THERMAL_ALERT_KEYS: [&str; 3] = [
    "thermalThrottle",
    "thermalShutdown",
    "powerSupplyThermalThrottle",
];

#[derive(Debug, Error)]
pub enum RecorderError {
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Store(#[from] JsonlError),
    #[error("metering rules: {0}")]
    Meters(#[from] std::io::Error),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct PollOutcome {
    pub fresh_samples: usize,
    pub energy_minutes_written: Vec<i64>,
    pub latency_minutes_written: Vec<i64>,
    pub new_events: usize,
    pub snapshot_written: bool,
}

pub struct Recorder {
    data_dir: PathBuf,
    calendar: Box<dyn Calendar>,
    _lock: DataDirLock,
    energy: EnergyStore,
    latency: LatencyStore,
    events: EventStore,
    postmortems: crate::postmortem::PostmortemStore,
    client_window: crate::client_window::ClientWindow,
    alerts: AlertStore,
    thermal: ThermalStore,
    obstruction: ObstructionStore,
    samples_store: SamplesStore,
    odometer: ClientTotalsStore,
    meters: MeterStore,
    samples: TelemetryAccumulator,
    open_energy: BTreeMap<i64, MinuteBucket>,
    open_latency: BTreeMap<i64, LatencyMinuteBucket>,
    last_snapshot_ms: i64,
    last_compaction_ms: i64,
}

impl Recorder {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, RecorderError> {
        Self::open_with(data_dir, now_ms(), Box::new(LocalCalendar))
    }

    /// Open with an explicit clock and calendar, for tests and for fixtures
    /// whose counterpart generator pins UTC.
    pub fn open_with(
        data_dir: impl AsRef<Path>,
        now_ms: i64,
        calendar: Box<dyn Calendar>,
    ) -> Result<Self, RecorderError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let lock = DataDirLock::acquire(&data_dir)?;
        let energy =
            EnergyStore::open_with(data_dir.join("energy.ndjson"), now_ms, calendar.as_ref())?;
        let latency =
            LatencyStore::open_with(data_dir.join("latency.ndjson"), now_ms, calendar.as_ref())?;
        let events = EventStore::open(data_dir.join("events.ndjson"))?;
        let postmortems =
            crate::postmortem::PostmortemStore::open(data_dir.join("postmortems.ndjson"))?;
        let client_window = crate::client_window::ClientWindow::restore(
            &data_dir.join("client-samples.json"),
            now_ms,
        );
        let alerts = AlertStore::open(history_path(&data_dir))?;
        let thermal = ThermalStore::open(data_dir.join("thermal.ndjson"))?;
        let obstruction = ObstructionStore::open(data_dir.join("obstruction.ndjson"))?;
        let samples_store = SamplesStore::new(data_dir.join("samples.json"));
        let odometer = ClientTotalsStore::open(data_dir.join("client-totals.json"));
        let mut meters = MeterStore::open(&data_dir);
        // Spec 0169: the spec 0160 limits become monthly rules, once.
        meters.migrate_monthly_limits(
            &data_dir.join("monthly-limits.json"),
            &odometer.core().lifetime_readings(),
            now_ms,
        )?;

        let persisted = match samples_store.read(now_ms, SAMPLE_WINDOW_MS) {
            Ok(samples) => samples,
            Err(error) => {
                tracing::warn!("snapshot unreadable, starting fresh: {error}");
                Vec::new()
            }
        };
        let mut samples = TelemetryAccumulator::new(SAMPLE_WINDOW_MS);
        samples.seed(&persisted);

        Ok(Self {
            data_dir,
            calendar,
            _lock: lock,
            energy,
            latency,
            events,
            postmortems,
            client_window,
            alerts,
            thermal,
            obstruction,
            samples_store,
            odometer,
            meters,
            samples,
            open_energy: BTreeMap::new(),
            open_latency: BTreeMap::new(),
            last_snapshot_ms: now_ms,
            last_compaction_ms: now_ms,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn samples(&self) -> &[TelemetrySample] {
        self.samples.samples()
    }

    /// The newest sample in the rolling window. The tray readout uses it to
    /// paint live throughput without re-reading the snapshot file.
    pub fn latest_sample(&self) -> Option<&TelemetrySample> {
        self.samples.samples().last()
    }

    pub fn energy(&self) -> &EnergyStore {
        &self.energy
    }

    pub fn latency(&self) -> &LatencyStore {
        &self.latency
    }

    pub fn events(&self) -> &EventStore {
        &self.events
    }

    /// Per-device throughput history (spec 0175).
    pub fn client_window(&self) -> &crate::client_window::ClientWindow {
        &self.client_window
    }

    pub fn client_window_mut(&mut self) -> &mut crate::client_window::ClientWindow {
        &mut self.client_window
    }

    pub fn outage_reports(&self, now_ms: i64) -> Vec<crate::postmortem::OutageReport> {
        self.postmortems.all(now_ms)
    }

    /// Write a report for each outage episode these transitions close (spec
    /// 0174). Call before recording the transitions: the open episode is where
    /// the outage's start comes from. Returns how many were written.
    pub fn write_outage_reports(
        &mut self,
        transitions: &[crate::alert::AlertTransition],
        at_ms: i64,
    ) -> StoreResult<usize> {
        use crate::alert::{AlertSource, AlertTransitionKind};
        use crate::postmortem::{BEFORE_WINDOW_MS, OutageReportInput, ReportAlertEpisode};
        let mut written = 0;
        for transition in transitions.iter().filter(|transition| {
            transition.kind == AlertTransitionKind::Cleared
                && transition.source == AlertSource::System
                && crate::postmortem::is_outage_source_key(&transition.key)
        }) {
            let Some(episode) = self.alerts.open_episodes().into_iter().find(|episode| {
                episode.source == AlertSource::System && episode.key == transition.key
            }) else {
                continue;
            };
            let before_start = (episode.start_ms - BEFORE_WINDOW_MS).div_euclid(60_000) * 60;
            let before_end = episode.start_ms.div_euclid(60_000) * 60;
            let buckets = self.energy_buckets_in_range(before_start, before_end)?;
            let dish_alerts: Vec<ReportAlertEpisode> = self
                .alerts
                .all(at_ms)
                .into_iter()
                .filter(|episode| episode.source == AlertSource::Dish)
                .map(|episode| ReportAlertEpisode {
                    alert_key: episode.key,
                    start_ms: episode.start_ms,
                    end_ms: episode.end_ms,
                })
                .collect();
            let report = crate::postmortem::build_outage_report(&OutageReportInput {
                source: &transition.key,
                start_ms: episode.start_ms,
                end_ms: transition.at_ms,
                dish_events: &self.events.all(at_ms),
                samples: self.samples.samples(),
                minute_buckets: &buckets,
                dish_alerts: &dish_alerts,
                generated_at_ms: crate::calendar::now_ms(),
            });
            if self.postmortems.add(report, at_ms)? {
                written += 1;
            }
        }
        Ok(written)
    }

    pub fn alerts(&self) -> &AlertStore {
        &self.alerts
    }

    pub fn alerts_mut(&mut self) -> &mut AlertStore {
        &mut self.alerts
    }

    pub fn thermal(&self) -> &ThermalStore {
        &self.thermal
    }

    pub fn obstruction(&self) -> &ObstructionStore {
        &self.obstruction
    }

    pub fn obstruction_mut(&mut self) -> &mut ObstructionStore {
        &mut self.obstruction
    }

    /// Persisted minutes plus the in-progress minute, since a summary of "today"
    /// must include the minute still being sampled.
    pub fn energy_buckets_in_range(
        &self,
        start_sec: i64,
        end_sec: i64,
    ) -> StoreResult<Vec<MinuteBucket>> {
        let mut buckets = self.energy.read_range(start_sec, end_sec)?;
        buckets.extend(
            self.open_energy
                .values()
                .filter(|bucket| bucket.minute >= start_sec && bucket.minute < end_sec)
                .cloned(),
        );
        Ok(buckets)
    }

    pub fn latency_buckets_in_range(
        &self,
        start_sec: i64,
        end_sec: i64,
    ) -> StoreResult<Vec<LatencyMinuteBucket>> {
        let mut buckets = self.latency.read_range(start_sec, end_sec)?;
        buckets.extend(
            self.open_latency
                .values()
                .filter(|bucket| bucket.minute >= start_sec && bucket.minute < end_sec)
                .cloned(),
        );
        Ok(buckets)
    }

    /// Fold the requested range's minutes into the shared usage summary. The
    /// in-progress minute is included, so "today" is never one minute stale.
    pub fn usage_summary(&self, range: UsageRange, now_ms: i64) -> StoreResult<UsageSummary> {
        let (start_sec, end_sec) = usage_range_bounds(range, now_ms, self.calendar.as_ref());
        let buckets = self.energy_buckets_in_range(start_sec, end_sec)?;
        Ok(summarize_usage(
            &buckets,
            range,
            now_ms,
            self.calendar.as_ref(),
        ))
    }

    /// Latency quality over a range, from the latency minute store (spec 0172).
    pub fn latency_summary(
        &self,
        range: UsageRange,
        now_ms: i64,
    ) -> StoreResult<crate::latency_summary::LatencySummary> {
        let (start_sec, end_sec) = usage_range_bounds(range, now_ms, self.calendar.as_ref());
        let buckets = self.latency_buckets_in_range(start_sec, end_sec)?;
        Ok(crate::latency_summary::summarize_latency(
            &buckets,
            range,
            now_ms,
            self.calendar.as_ref(),
        ))
    }

    /// Fold one dish-history poll into every store.
    pub fn record_poll(
        &mut self,
        history: &DishHistory,
        now_ms: i64,
        router: RouterReadings,
    ) -> Result<PollOutcome, RecorderError> {
        let window = decode_history_window(history, now_ms);
        let before = self.samples.samples().len();
        self.samples.ingest_window(&window, router);
        let fresh_samples = self.samples.samples().len().saturating_sub(before);

        let per_minute = fold_samples_to_minutes(&window.samples);
        let per_latency_minute = fold_samples_to_latency_minutes(&window.samples);

        // Replace, not accumulate: re-seeing the in-progress minute across
        // overlapping polls is idempotent.
        for (minute, bucket) in per_minute {
            if minute > self.energy.last_written_minute() {
                self.open_energy.insert(minute, bucket);
            }
        }
        for (minute, bucket) in per_latency_minute {
            if minute > self.latency.last_written_minute() {
                self.open_latency.insert(minute, bucket);
            }
        }

        let current_minute = now_ms.div_euclid(60_000) * 60;
        let mut energy_minutes_written = Vec::new();
        let completed: Vec<i64> = self
            .open_energy
            .range(..current_minute)
            .map(|(minute, _)| *minute)
            .collect();
        for minute in completed {
            if let Some(bucket) = self.open_energy.remove(&minute) {
                self.energy.append(&bucket)?;
                energy_minutes_written.push(minute);
            }
        }

        let mut latency_minutes_written = Vec::new();
        let completed: Vec<i64> = self
            .open_latency
            .range(..current_minute)
            .map(|(minute, _)| *minute)
            .collect();
        for minute in completed {
            if let Some(bucket) = self.open_latency.remove(&minute) {
                self.latency.append(&bucket)?;
                latency_minutes_written.push(minute);
            }
        }

        let new_events = self.events.upsert(decode_outage_events(history), now_ms)?;

        let snapshot_written = if now_ms - self.last_snapshot_ms >= SNAPSHOT_EVERY_MS {
            self.write_snapshot(now_ms)?;
            true
        } else {
            false
        };

        if now_ms - self.last_compaction_ms >= COMPACT_EVERY_MS {
            self.energy.compact_with(now_ms, self.calendar.as_ref())?;
            self.latency.compact_with(now_ms, self.calendar.as_ref())?;
            self.last_compaction_ms = now_ms;
        }

        Ok(PollOutcome {
            fresh_samples,
            energy_minutes_written,
            latency_minutes_written,
            new_events,
            snapshot_written,
        })
    }

    /// Fold router-history events into the same durable log.
    pub fn record_events(
        &mut self,
        events: impl IntoIterator<Item = crate::telemetry::OutageEvent>,
        now_ms: i64,
    ) -> StoreResult<usize> {
        self.events.upsert(events, now_ms)
    }

    /// Record false→true and true→false edges for the three thermal flags. Call
    /// only with a status reply in hand: a failed fetch is no reading, not a
    /// cleared alert.
    pub fn record_thermal(
        &mut self,
        alerts: &BTreeMap<String, bool>,
        now_ms: i64,
    ) -> StoreResult<()> {
        for key in THERMAL_ALERT_KEYS {
            let firing = alerts.get(key).copied().unwrap_or(false);
            if firing {
                self.thermal.open_episode(key, now_ms, now_ms)?;
            } else {
                self.thermal.close_episode(key, now_ms, now_ms)?;
            }
        }
        Ok(())
    }

    pub fn record_obstruction(&mut self, snapshot: ObstructionSnapshot) -> StoreResult<()> {
        self.obstruction.record(snapshot)?;
        Ok(())
    }

    pub fn odometer(&self) -> &ClientTotalsStore {
        &self.odometer
    }

    pub fn odometer_mut(&mut self) -> &mut ClientTotalsStore {
        &mut self.odometer
    }

    pub fn client_totals(&self, client_key: Option<&str>) -> Vec<ClientTotal> {
        self.odometer.core().totals(client_key)
    }

    pub fn client_monthly_limits(&self) -> Vec<ClientMonthlyLimit> {
        self.meters.monthly_limits()
    }

    pub fn set_client_monthly_limit(
        &mut self,
        limit: ClientMonthlyLimit,
    ) -> Result<(), RecorderError> {
        let key = self.odometer.core().resolve_key(&limit.client_key);
        let reading = self
            .odometer
            .core()
            .lifetime_readings()
            .into_iter()
            .find(|reading| reading.client_key == key);
        self.meters
            .set_monthly_limit(&limit, reading.as_ref(), crate::calendar::now_ms())?;
        Ok(())
    }

    pub fn remove_client_monthly_limit(&mut self, client_key: &str) -> Result<bool, RecorderError> {
        Ok(self.meters.remove_device_rule(client_key)?)
    }

    pub fn meters(&self) -> &MeterStore {
        &self.meters
    }

    /// Write a device's own rule. A new rule starts from the device's current
    /// counter, which only the recorder knows, so it measures from now rather
    /// than inheriting the device's whole history.
    pub fn upsert_device_rule(&mut self, terms: MeterRuleTerms) -> Result<(), RecorderError> {
        let key = self.odometer.core().resolve_key(&terms.client_key);
        let reading = self
            .odometer
            .core()
            .lifetime_readings()
            .into_iter()
            .find(|reading| reading.client_key == key);
        let terms = MeterRuleTerms {
            client_key: key,
            lifetime_rx: reading
                .as_ref()
                .map_or(terms.lifetime_rx, |r| r.lifetime_rx),
            lifetime_tx: reading
                .as_ref()
                .map_or(terms.lifetime_tx, |r| r.lifetime_tx),
            ..terms
        };
        Ok(self.meters.upsert_device_rule(terms)?)
    }

    pub fn remove_device_rule(&mut self, client_key: &str) -> Result<bool, RecorderError> {
        Ok(self.meters.remove_device_rule(client_key)?)
    }

    pub fn upsert_group(&mut self, group: DeviceGroup, now_ms: i64) -> Result<(), RecorderError> {
        let readings = self.meter_readings();
        Ok(self.meters.upsert_group(group, &readings, now_ms)?)
    }

    pub fn remove_group(&mut self, group_id: &str) -> Result<bool, RecorderError> {
        Ok(self.meters.remove_group(group_id)?)
    }

    pub fn restart_rule(
        &mut self,
        client_key: &str,
        group_id: Option<&str>,
        now_ms: i64,
    ) -> Result<bool, RecorderError> {
        Ok(self.meters.restart(client_key, group_id, now_ms)?)
    }

    /// Lifetime counters for every device the odometer knows.
    pub fn meter_readings(&self) -> Vec<crate::data_meter::MeterReading> {
        self.odometer
            .core()
            .lifetime_readings()
            .iter()
            .map(crate::meter_store::LifetimeReading::meter_reading)
            .collect()
    }

    /// Fold the odometer's current counters into the rules. Reads only what
    /// the recorder already holds (SPEC-RULE-008).
    pub fn evaluate_meters(&mut self, now_ms: i64) -> Result<Vec<MeterTransition>, RecorderError> {
        let readings = self.meter_readings();
        let odometer = self.odometer.core();
        Ok(self
            .meters
            .evaluate(&readings, |key| odometer.resolve_key(key), now_ms)?)
    }

    /// Fold one odometer record into another (spec 0176). Rules follow the
    /// alias on their next evaluation.
    pub fn merge_clients(&mut self, from: &str, to: &str) -> bool {
        let merged = self.odometer.core_mut().merge(from, to);
        if merged {
            let _ = self.odometer.snapshot();
        }
        merged
    }

    pub fn reject_merge(&mut self, a: &str, b: &str) -> bool {
        let rejected = self.odometer.core_mut().reject_merge(a, b);
        if rejected {
            let _ = self.odometer.snapshot();
        }
        rejected
    }

    pub fn reset_client_total(&mut self, client_key: &str, at_ms: i64) -> bool {
        let res = self.odometer.core_mut().reset(client_key, at_ms);
        if res {
            let _ = self.odometer.snapshot();
        }
        res
    }

    pub fn remove_client_total(&mut self, client_key: &str) -> Result<bool, RecorderError> {
        let limit_removed = self.meters.remove_device(client_key)?;
        let res = self.odometer.core_mut().remove(client_key);
        if res {
            let _ = self.odometer.snapshot();
        }
        Ok(limit_removed || res)
    }

    pub fn clear_client_totals(&mut self) -> Result<(), RecorderError> {
        self.meters.clear()?;
        self.odometer.core_mut().clear();
        let _ = self.odometer.snapshot();
        Ok(())
    }

    /// Write the rolling sample snapshot now (shutdown, or after a poll that
    /// crossed the minute boundary).
    pub fn write_snapshot(&mut self, now_ms: i64) -> StoreResult<()> {
        if !self.samples.samples().is_empty() {
            self.samples_store.write(self.samples.samples())?;
        }
        let _ = self.odometer.snapshot();
        let _ = self
            .client_window
            .snapshot(&self.data_dir.join("client-samples.json"), now_ms);
        self.last_snapshot_ms = now_ms;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;
    use crate::calendar::UtcCalendar;
    use crate::telemetry::{DishEvent, EventLog, U64Value};

    // 2026-06-15T12:00:30Z
    const NOW_MS: i64 = 1_781_524_830_000;

    fn history_with_samples(count: usize) -> DishHistory {
        DishHistory {
            current: Some(U64Value(count as u64)),
            pop_ping_latency_ms: vec![20.0; count],
            pop_ping_drop_rate: vec![0.0; count],
            downlink_throughput_bps: vec![1_000.0; count],
            uplink_throughput_bps: vec![500.0; count],
            power_in: vec![50.0; count],
            ..DishHistory::default()
        }
    }

    fn recorder(directory: &tempfile::TempDir, now_ms: i64) -> Recorder {
        Recorder::open_with(directory.path(), now_ms, Box::new(UtcCalendar)).expect("open recorder")
    }

    // SPEC-RECORDER-001: the recorder keeps the historian's five-second cadence.
    #[test]
    fn historian_cadence_is_five_seconds() {
        assert_eq!(crate::HISTORIAN_POLL_INTERVAL, Duration::from_secs(5));
    }

    #[test]
    fn only_completed_minutes_are_written_and_re_polls_do_not_duplicate() {
        let directory = tempdir().expect("temp dir");
        let mut recorder = recorder(&directory, NOW_MS);
        let history = history_with_samples(120);

        let first = recorder
            .record_poll(&history, NOW_MS, RouterReadings::default())
            .expect("first poll");
        let current_minute = NOW_MS.div_euclid(60_000) * 60;
        assert_eq!(
            first.energy_minutes_written,
            vec![current_minute - 120, current_minute - 60]
        );
        assert_eq!(first.latency_minutes_written, first.energy_minutes_written);
        assert_eq!(
            recorder
                .energy()
                .read_range(0, i64::MAX)
                .expect("energy")
                .len(),
            2
        );
        // The API sees the open minute too, so "today" is not one minute stale.
        assert_eq!(
            recorder
                .energy_buckets_in_range(0, i64::MAX)
                .expect("merged")
                .len(),
            3
        );
        assert_eq!(recorder.samples().len(), 120);

        let second = recorder
            .record_poll(&history, NOW_MS + 5_000, RouterReadings::default())
            .expect("second poll");
        assert!(second.energy_minutes_written.is_empty());
        assert_eq!(
            recorder
                .energy()
                .read_range(0, i64::MAX)
                .expect("energy")
                .len(),
            2
        );

        // Cross the 12:01 boundary: the in-progress 12:00 minute is completed.
        let third = recorder
            .record_poll(&history, NOW_MS + 35_000, RouterReadings::default())
            .expect("third poll");
        assert_eq!(third.energy_minutes_written, vec![current_minute]);
        assert_eq!(
            recorder
                .energy()
                .read_range(0, i64::MAX)
                .expect("energy")
                .len(),
            3
        );
        assert_eq!(
            recorder
                .latency()
                .read_range(0, i64::MAX)
                .expect("latency")
                .len(),
            3
        );
    }

    #[test]
    fn a_poll_records_events_and_periodically_snapshots_samples() {
        let directory = tempdir().expect("temp dir");
        let mut recorder = recorder(&directory, NOW_MS);
        let mut history = history_with_samples(10);
        history.event_log = Some(EventLog {
            events: vec![DishEvent {
                severity: Some("EVENT_SEVERITY_WARNING".into()),
                reason: Some("EVENT_REASON_OUTAGE_NO_PINGS".into()),
                start_timestamp_ns: Some(U64Value(((NOW_MS - 5_000) as u64) * 1_000_000)),
                duration_ns: Some(U64Value(1_000_000_000)),
                client_switching_band_metadata: None,
            }],
        });

        let first = recorder
            .record_poll(&history, NOW_MS, RouterReadings::default())
            .expect("first");
        assert_eq!(first.new_events, 1);
        assert!(!first.snapshot_written);
        assert_eq!(recorder.events().all(NOW_MS).len(), 1);

        let second = recorder
            .record_poll(
                &history,
                NOW_MS + SNAPSHOT_EVERY_MS,
                RouterReadings::default(),
            )
            .expect("second");
        assert!(second.snapshot_written);
        assert_eq!(
            recorder
                .samples_store
                .read(NOW_MS + SNAPSHOT_EVERY_MS, SAMPLE_WINDOW_MS)
                .expect("snapshot")
                .len(),
            10
        );
    }

    #[test]
    fn thermal_edges_open_and_close_episodes() {
        let directory = tempdir().expect("temp dir");
        let mut recorder = recorder(&directory, NOW_MS);
        let mut alerts = BTreeMap::new();

        alerts.insert("thermalThrottle".to_string(), true);
        recorder.record_thermal(&alerts, NOW_MS).expect("open");
        assert!(recorder.thermal().is_open("thermalThrottle"));

        // Still firing: no duplicate episode.
        recorder
            .record_thermal(&alerts, NOW_MS + 5_000)
            .expect("still open");
        assert_eq!(recorder.thermal().all(NOW_MS + 5_000).len(), 1);

        alerts.insert("thermalThrottle".to_string(), false);
        recorder
            .record_thermal(&alerts, NOW_MS + 10_000)
            .expect("close");
        assert!(!recorder.thermal().is_open("thermalThrottle"));
        assert_eq!(
            recorder.thermal().all(NOW_MS + 10_000)[0].end_ms,
            Some(NOW_MS + 10_000)
        );
    }

    #[test]
    fn a_second_recorder_cannot_open_the_same_directory() {
        let directory = tempdir().expect("temp dir");
        let _first = recorder(&directory, NOW_MS);
        assert!(matches!(
            Recorder::open_with(directory.path(), NOW_MS, Box::new(UtcCalendar)),
            Err(RecorderError::Lock(_))
        ));
    }
}
