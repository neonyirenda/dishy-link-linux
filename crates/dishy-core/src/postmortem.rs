//! Outage reports: what the link looked like in the five minutes before a
//! drop, why the dish says it dropped, and what the heater and thermal alerts
//! were doing.
//!
//! Port of upstream `core/postmortem.ts` and upstream `collector/postmortemStore.mts`
//! (spec 0174). Built from the samples, minute buckets, dish events, and alert
//! episodes the recorder already holds, the moment an outage episode closes:
//! the 1 s samples it needs age out within hours.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::energy::MinuteBucket;
use crate::jsonl::{
    JsonlResult, ensure_parent_directory, read_json_lines, write_json_lines_atomically,
};
use crate::telemetry::{
    EventKind, OutageEvent, TelemetrySample, canonical_cause, outage_event_kind,
};

pub const BEFORE_WINDOW_MS: i64 = 5 * 60_000;
pub const MATCH_TOLERANCE_MS: i64 = 30_000;
pub const HEATER_ALERT_KEY: &str = "isHeating";
pub const THERMAL_ALERT_KEYS: [&str; 3] = [
    "thermalThrottle",
    "thermalShutdown",
    "powerSupplyThermalThrottle",
];
const MIN_SAMPLE_COVERAGE_SECONDS: usize = 60;
const RETENTION_MS: i64 = 30 * 24 * 3_600_000;

/// The two system alerts an outage report is written for.
pub fn is_outage_source_key(key: &str) -> bool {
    matches!(key, "starlinkOutage" | "dishUnreachable")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BeforeDropSource {
    Samples,
    MinuteBuckets,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeDropStats {
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub coverage_seconds: i64,
    pub latency_avg_ms: Option<f64>,
    pub downlink_avg_bps: Option<f64>,
    pub uplink_avg_bps: Option<f64>,
    pub drop_rate_avg: Option<f64>,
    pub heater_active: bool,
    pub source: BeforeDropSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportAlertEpisode {
    pub alert_key: String,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutageReport {
    pub id: String,
    pub source: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_ms: i64,
    pub cause: Option<String>,
    pub before_drop: BeforeDropStats,
    pub thermal: Vec<ReportAlertEpisode>,
    pub generated_at_ms: i64,
}

pub struct OutageReportInput<'a> {
    pub source: &'a str,
    pub start_ms: i64,
    pub end_ms: i64,
    pub dish_events: &'a [OutageEvent],
    pub samples: &'a [TelemetrySample],
    pub minute_buckets: &'a [MinuteBucket],
    pub dish_alerts: &'a [ReportAlertEpisode],
    pub generated_at_ms: i64,
}

fn overlaps(a_start: i64, a_end: i64, b_start: i64, b_end: i64, tolerance_ms: i64) -> bool {
    a_start - tolerance_ms <= b_end && b_start - tolerance_ms <= a_end
}

fn event_end(event: &OutageEvent) -> i64 {
    event.start_ms + event.duration_ms.max(0)
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0_usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    (count > 0).then(|| sum / count as f64)
}

/// A `starlinkOutage` start walked back over the contiguous run of
/// fully-dropped samples; a `dishUnreachable` start is the episode's own.
pub fn outage_start_ms(source: &str, episode_start_ms: i64, samples: &[TelemetrySample]) -> i64 {
    if source != "starlinkOutage" {
        return episode_start_ms;
    }
    let mut start = episode_start_ms;
    for sample in samples.iter().rev() {
        if sample.timestamp_ms > episode_start_ms {
            continue;
        }
        if sample.drop_rate < 1.0 {
            break;
        }
        start = sample.timestamp_ms;
    }
    start
}

/// The nearest dish event overlapping the outage, canonicalised.
fn cause_of(events: &[OutageEvent], start_ms: i64, end_ms: i64) -> Option<String> {
    events
        .iter()
        .filter(|event| {
            overlaps(
                event.start_ms,
                event_end(event),
                start_ms,
                end_ms,
                MATCH_TOLERANCE_MS,
            )
        })
        .min_by_key(|event| (event.start_ms - start_ms).abs())
        .map(|event| canonical_cause(&event.cause))
}

fn before_from_samples(
    samples: &[TelemetrySample],
    start_ms: i64,
    heater_active: bool,
) -> BeforeDropStats {
    let window_start_ms = start_ms - BEFORE_WINDOW_MS;
    let in_window: Vec<&TelemetrySample> = samples
        .iter()
        .filter(|sample| (window_start_ms..start_ms).contains(&sample.timestamp_ms))
        .collect();
    BeforeDropStats {
        window_start_ms,
        window_end_ms: start_ms,
        coverage_seconds: in_window.len() as i64,
        latency_avg_ms: mean(in_window.iter().filter_map(|sample| sample.latency_ms)),
        downlink_avg_bps: mean(in_window.iter().map(|sample| sample.downlink_bps)),
        uplink_avg_bps: mean(in_window.iter().map(|sample| sample.uplink_bps)),
        drop_rate_avg: mean(in_window.iter().map(|sample| sample.drop_rate)),
        heater_active,
        source: BeforeDropSource::Samples,
    }
}

/// Minute buckets keep throughput but not latency or loss, so those stay
/// unknown rather than invented.
fn before_from_buckets(
    buckets: &[MinuteBucket],
    start_ms: i64,
    heater_active: bool,
) -> BeforeDropStats {
    let window_start_ms = start_ms - BEFORE_WINDOW_MS;
    let window_start_sec = window_start_ms.div_euclid(1_000);
    let drop_minute_sec = start_ms.div_euclid(60_000) * 60;
    let in_window: Vec<&MinuteBucket> = buckets
        .iter()
        .filter(|bucket| bucket.minute >= window_start_sec && bucket.minute < drop_minute_sec)
        .collect();
    let coverage: i64 = in_window
        .iter()
        .map(|bucket| i64::from(bucket.samples))
        .sum();
    let per_second = |bits: f64| (coverage > 0).then(|| bits / coverage as f64);
    BeforeDropStats {
        window_start_ms,
        window_end_ms: start_ms,
        coverage_seconds: coverage,
        latency_avg_ms: None,
        downlink_avg_bps: per_second(in_window.iter().map(|bucket| bucket.downlink_bits).sum()),
        uplink_avg_bps: per_second(in_window.iter().map(|bucket| bucket.uplink_bits).sum()),
        drop_rate_avg: None,
        heater_active,
        source: BeforeDropSource::MinuteBuckets,
    }
}

pub fn build_outage_report(input: &OutageReportInput<'_>) -> OutageReport {
    let start_ms = outage_start_ms(input.source, input.start_ms, input.samples);
    let window_start_ms = start_ms - BEFORE_WINDOW_MS;
    let episode_end =
        |episode: &ReportAlertEpisode| episode.end_ms.unwrap_or(input.generated_at_ms);
    let heater_active = input.dish_alerts.iter().any(|episode| {
        episode.alert_key == HEATER_ALERT_KEY
            && overlaps(
                episode.start_ms,
                episode_end(episode),
                window_start_ms,
                start_ms,
                0,
            )
    });
    let sample_coverage = input
        .samples
        .iter()
        .filter(|sample| (window_start_ms..start_ms).contains(&sample.timestamp_ms))
        .count();
    OutageReport {
        id: format!("system:{}:{}", input.source, input.start_ms),
        source: input.source.to_owned(),
        start_ms,
        end_ms: input.end_ms,
        duration_ms: input.end_ms - start_ms,
        cause: cause_of(input.dish_events, start_ms, input.end_ms),
        before_drop: if sample_coverage >= MIN_SAMPLE_COVERAGE_SECONDS {
            before_from_samples(input.samples, start_ms, heater_active)
        } else {
            before_from_buckets(input.minute_buckets, start_ms, heater_active)
        },
        thermal: input
            .dish_alerts
            .iter()
            .filter(|episode| {
                THERMAL_ALERT_KEYS.contains(&episode.alert_key.as_str())
                    && overlaps(
                        episode.start_ms,
                        episode_end(episode),
                        window_start_ms,
                        input.end_ms,
                        0,
                    )
            })
            .cloned()
            .collect(),
        generated_at_ms: input.generated_at_ms,
    }
}

/// Each report matched to at most one dish outage event, nearest start first.
/// Returns event index → report index, and the reports nothing claimed.
pub fn assign_reports(
    reports: &[OutageReport],
    events: &[OutageEvent],
) -> (HashMap<usize, usize>, Vec<usize>) {
    let mut candidates: Vec<(i64, usize, usize)> = Vec::new();
    for (report_index, report) in reports.iter().enumerate() {
        for (event_index, event) in events.iter().enumerate() {
            if outage_event_kind(&event.cause) == EventKind::Outage
                && overlaps(
                    report.start_ms,
                    report.end_ms,
                    event.start_ms,
                    event_end(event),
                    MATCH_TOLERANCE_MS,
                )
            {
                candidates.push((
                    (report.start_ms - event.start_ms).abs(),
                    report_index,
                    event_index,
                ));
            }
        }
    }
    candidates.sort_unstable();
    let mut by_event = HashMap::new();
    let mut assigned = HashSet::new();
    for (_, report_index, event_index) in candidates {
        if assigned.contains(&report_index) || by_event.contains_key(&event_index) {
            continue;
        }
        by_event.insert(event_index, report_index);
        assigned.insert(report_index);
    }
    let unclaimed = (0..reports.len())
        .filter(|index| !assigned.contains(index))
        .collect();
    (by_event, unclaimed)
}

#[derive(Debug)]
pub struct PostmortemStore {
    file_path: PathBuf,
    reports: Vec<OutageReport>,
}

impl PostmortemStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        let reports = read_json_lines::<OutageReport>(&file_path)?;
        Ok(Self { file_path, reports })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Persist a report. `false` if one with this id already exists: an
    /// episode closes once, and the first frozen report stands.
    pub fn add(&mut self, report: OutageReport, now_ms: i64) -> JsonlResult<bool> {
        if self.reports.iter().any(|held| held.id == report.id) {
            return Ok(false);
        }
        let cutoff = now_ms - RETENTION_MS;
        let mut next: Vec<OutageReport> = self
            .reports
            .iter()
            .filter(|held| held.end_ms >= cutoff)
            .cloned()
            .collect();
        next.push(report);
        next.sort_by_key(|held| held.end_ms);
        write_json_lines_atomically(&self.file_path, &next)?;
        self.reports = next;
        Ok(true)
    }

    /// Reports inside the retention window, newest end first.
    pub fn all(&self, now_ms: i64) -> Vec<OutageReport> {
        let cutoff = now_ms - RETENTION_MS;
        let mut reports: Vec<OutageReport> = self
            .reports
            .iter()
            .filter(|report| report.end_ms >= cutoff)
            .cloned()
            .collect();
        reports.sort_by_key(|report| std::cmp::Reverse(report.end_ms));
        reports
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::telemetry::Severity;

    const START: i64 = 1_800_000_600_000;

    fn sample(at_ms: i64, drop_rate: f64, latency: Option<f64>) -> TelemetrySample {
        TelemetrySample {
            timestamp_ms: at_ms,
            latency_ms: latency,
            drop_rate,
            downlink_bps: 40e6,
            uplink_bps: 5e6,
            power_w: 45.0,
            router_latency_ms: None,
            router_ping_success_percent: None,
        }
    }

    fn event(start_ms: i64, duration_ms: i64, cause: &str) -> OutageEvent {
        OutageEvent {
            start_ms,
            duration_ms,
            cause: cause.into(),
            severity: Severity::Critical,
            band_switch: None,
        }
    }

    fn input<'a>(
        source: &'a str,
        samples: &'a [TelemetrySample],
        buckets: &'a [MinuteBucket],
        events: &'a [OutageEvent],
        alerts: &'a [ReportAlertEpisode],
    ) -> OutageReportInput<'a> {
        OutageReportInput {
            source,
            start_ms: START,
            end_ms: START + 20_000,
            dish_events: events,
            samples,
            minute_buckets: buckets,
            dish_alerts: alerts,
            generated_at_ms: START + 21_000,
        }
    }

    // SPEC-OUTAGE-002: the start walks back over the dropped run; the cause is
    // the nearest overlapping dish event; five minutes of samples feed the
    // before-drop stats; heater and thermal episodes are picked by overlap.
    #[test]
    fn reports_walk_back_the_start_and_read_the_minutes_before() {
        let mut samples: Vec<_> = (1..=300)
            .rev()
            .map(|seconds| sample(START - 3_000 - seconds * 1_000, 0.0, Some(30.0)))
            .collect();
        samples.extend((0..=3).map(|seconds| sample(START - 3_000 + seconds * 1_000, 1.0, None)));
        assert_eq!(
            outage_start_ms("starlinkOutage", START, &samples),
            START - 3_000
        );
        assert_eq!(outage_start_ms("dishUnreachable", START, &samples), START);

        let events = [
            event(START - 50_000, 1_000, "EVENT_REASON_OBSTRUCTED"),
            event(START - 2_000, 15_000, "EVENT_REASON_NO_PINGS"),
        ];
        let alerts = [
            ReportAlertEpisode {
                alert_key: HEATER_ALERT_KEY.into(),
                start_ms: START - 120_000,
                end_ms: Some(START - 60_000),
            },
            ReportAlertEpisode {
                alert_key: "thermalThrottle".into(),
                start_ms: START + 5_000,
                end_ms: None,
            },
            ReportAlertEpisode {
                alert_key: "thermalThrottle".into(),
                start_ms: START - 900_000,
                end_ms: Some(START - 800_000),
            },
        ];
        let report = build_outage_report(&input("starlinkOutage", &samples, &[], &events, &alerts));
        assert_eq!(report.id, format!("system:starlinkOutage:{START}"));
        assert_eq!(report.start_ms, START - 3_000);
        assert_eq!(report.duration_ms, START + 20_000 - (START - 3_000));
        assert_eq!(report.cause.as_deref(), Some("NO_PINGS"));
        assert_eq!(report.before_drop.source, BeforeDropSource::Samples);
        assert_eq!(report.before_drop.coverage_seconds, 300);
        assert_eq!(report.before_drop.latency_avg_ms, Some(30.0));
        assert_eq!(report.before_drop.drop_rate_avg, Some(0.0));
        assert!(report.before_drop.heater_active);
        assert_eq!(report.thermal.len(), 1);
        assert_eq!(report.thermal[0].start_ms, START + 5_000);
    }

    // SPEC-OUTAGE-002: too few samples falls back to minute buckets, which
    // give throughput but no latency or loss.
    #[test]
    fn sparse_samples_fall_back_to_minute_buckets() {
        let minute = (START / 60_000) * 60 - 120;
        let buckets = [MinuteBucket {
            minute,
            samples: 60,
            watt_seconds: 2_700.0,
            downlink_bits: 60.0 * 8e6,
            uplink_bits: 60.0 * 1e6,
        }];
        let report = build_outage_report(&input("dishUnreachable", &[], &buckets, &[], &[]));
        assert_eq!(report.before_drop.source, BeforeDropSource::MinuteBuckets);
        assert_eq!(report.before_drop.coverage_seconds, 60);
        assert_eq!(report.before_drop.downlink_avg_bps, Some(8e6));
        assert_eq!(report.before_drop.latency_avg_ms, None);
        assert_eq!(report.cause, None);
        assert!(!report.before_drop.heater_active);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["beforeDrop"]["source"], "minute-buckets");
    }

    // SPEC-OUTAGE-003: one report per id, atomic and kept 30 days.
    #[test]
    fn the_store_keeps_the_first_report_for_thirty_days() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("postmortems.ndjson");
        let mut store = PostmortemStore::open(&path).unwrap();
        let report = build_outage_report(&input("dishUnreachable", &[], &[], &[], &[]));
        assert!(store.add(report.clone(), START).unwrap());
        assert!(!store.add(report.clone(), START).unwrap());
        let reopened = PostmortemStore::open(&path).unwrap();
        assert_eq!(reopened.all(START), std::slice::from_ref(&report));
        assert!(reopened.all(START + 31 * 24 * 3_600_000).is_empty());
    }

    // SPEC-OUTAGE-004: each report claims at most one outage event, nearest
    // first; leftovers stand as their own rows.
    #[test]
    fn reports_are_assigned_to_the_nearest_outage_event() {
        let near = build_outage_report(&input("starlinkOutage", &[], &[], &[], &[]));
        let mut far = near.clone();
        far.id = "system:dishUnreachable:1".into();
        far.start_ms = START + 3_600_000;
        far.end_ms = START + 3_610_000;
        let events = [
            event(START + 1_000, 10_000, "EVENT_REASON_NO_PINGS"),
            event(START + 2_000, 10_000, "EVENT_REASON_OBSTRUCTED"),
            event(START - 500_000, 1_000, "EVENT_REASON_NO_PINGS"),
        ];
        let (by_event, unclaimed) = assign_reports(&[near, far], &events);
        assert_eq!(by_event.get(&0), Some(&0));
        assert_eq!(by_event.len(), 1);
        assert_eq!(unclaimed, [1]);
    }
}
