//! The dashboard's view model: tiles, chart series, and shared formatting.
//!
//! This module is deliberately GTK-free. It cuts the recorder's sample window
//! into the shapes the widgets paint, so the windowing and bucket rules can be
//! tested without a display. `dashboard_view.rs` owns the widgets; this file
//! owns what they say.

use std::collections::BTreeMap;

use dishy_core::telemetry::TelemetrySample;

use crate::i18n::Translator;
use crate::readings::{
    LatencyQuality, POWER_BUCKET_MS, STALE_AFTER_MS, has_recent_readings, latency_quality,
    power_bucket_end_ms, power_bucket_mean, recent_average, sparkline_from,
};

pub const DASHBOARD_WINDOWS: [u32; 3] = [15, 60, 360];
pub const DEFAULT_WINDOW_MINUTES: u32 = 15;
/// Enough points for a smooth line without making a six-hour window expensive.
const CHART_MAX_POINTS: usize = 360;
const MIN_BUCKET_MS: i64 = 1_000;

pub const DOWN_COLOR: (f64, f64, f64) = (0.24, 0.55, 0.96);
pub const UP_COLOR: (f64, f64, f64) = (1.0, 0.58, 0.0);
pub const LATENCY_COLOR: (f64, f64, f64) = (0.68, 0.46, 0.96);
pub const POWER_COLOR: (f64, f64, f64) = (0.20, 0.78, 0.45);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartPoint {
    pub at_ms: i64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSeries {
    pub id: &'static str,
    pub label: String,
    pub color: (f64, f64, f64),
    pub points: Vec<ChartPoint>,
    /// Width of one chart bucket, so the renderer knows when a jump is a gap.
    pub bucket_ms: i64,
}

impl ChartSeries {
    pub fn max_value(&self) -> f64 {
        self.points
            .iter()
            .map(|point| point.value)
            .filter(|value| value.is_finite())
            .fold(0.0_f64, f64::max)
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Whether the line must break between two adjacent points. Five seconds is
    /// the dish's own coverage threshold; a six-hour bucket gets proportionally
    /// more room so a bucket that merely contains no readings is still a break.
    pub fn gap_after(&self, index: usize) -> bool {
        let Some(current) = self.points.get(index) else {
            return false;
        };
        let Some(next) = self.points.get(index + 1) else {
            return false;
        };
        next.at_ms.saturating_sub(current.at_ms) >= (self.bucket_ms * 2).max(5_000)
    }

    pub fn is_gap_free(&self) -> bool {
        (0..self.points.len().saturating_sub(1)).all(|index| !self.gap_after(index))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sparklines {
    pub downlink: Vec<Option<f64>>,
    pub uplink: Vec<Option<f64>>,
    pub latency: Vec<Option<f64>>,
    pub power: Vec<Option<f64>>,
    pub ping_success: Vec<Option<f64>>,
}

#[derive(Debug, Clone)]
pub struct DashboardSnapshot {
    pub now_ms: i64,
    pub window_minutes: u32,
    pub window_samples: usize,
    pub latest: Option<TelemetrySample>,
    pub latest_age_ms: Option<i64>,
    pub stale: bool,
    pub download_bps: f64,
    pub upload_bps: f64,
    pub latency_ms: Option<f64>,
    pub power_w: f64,
    pub ping_success_percent: f64,
    pub average_power_w: f64,
    pub latency_quality: Option<LatencyQuality>,
    /// Only a status reply carries the obstruction map; this stays `None` until
    /// the status poll lands, so the tile reads as an em dash, never zero.
    pub sky_obstructed_percent: Option<f64>,
    pub power_window_end_ms: i64,
    pub throughput: Vec<ChartSeries>,
    pub latency: Vec<ChartSeries>,
    pub power: Vec<ChartSeries>,
    pub sparklines: Sparklines,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Reduction {
    Average,
    Max,
    /// For success rates, so a brief total loss survives being bucketed.
    Min,
}

#[derive(Debug, Default)]
struct Bucket {
    sum: f64,
    count: usize,
    max: f64,
    min: f64,
}

impl Bucket {
    fn push(&mut self, value: f64) {
        if self.count == 0 {
            self.max = value;
            self.min = value;
        } else {
            self.max = self.max.max(value);
            self.min = self.min.min(value);
        }
        self.sum += value;
        self.count += 1;
    }

    fn reduce(&self, reduction: Reduction) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        Some(match reduction {
            Reduction::Average => self.sum / self.count as f64,
            Reduction::Max => self.max,
            Reduction::Min => self.min,
        })
    }
}

pub(crate) fn bucket_series(
    samples: &[TelemetrySample],
    end_ms: i64,
    minutes: u32,
    get_value: impl Fn(&TelemetrySample) -> Option<f64>,
    reduction: Reduction,
) -> (Vec<ChartPoint>, i64) {
    let window_ms = i64::from(minutes) * 60_000;
    let start_ms = end_ms - window_ms;
    let bucket_ms = (window_ms / CHART_MAX_POINTS as i64).max(MIN_BUCKET_MS);
    let mut buckets: BTreeMap<i64, Bucket> = BTreeMap::new();

    for sample in samples {
        if sample.timestamp_ms < start_ms || sample.timestamp_ms >= end_ms {
            continue;
        }
        let Some(value) = get_value(sample).filter(|value| value.is_finite()) else {
            continue;
        };
        let key = (sample.timestamp_ms - start_ms).div_euclid(bucket_ms);
        buckets.entry(key).or_default().push(value);
    }

    let points = buckets
        .into_iter()
        .filter_map(|(key, bucket)| {
            bucket.reduce(reduction).map(|value| ChartPoint {
                at_ms: start_ms + key * bucket_ms + bucket_ms / 2,
                value,
            })
        })
        .collect();
    (points, bucket_ms)
}

/// Samples inside the selected wall-clock window. Power uses its settled bucket
/// boundary as the end; the other charts use `now`.
pub fn window_sample_count(samples: &[TelemetrySample], end_ms: i64, minutes: u32) -> usize {
    let start_ms = end_ms - i64::from(minutes) * 60_000;
    samples
        .iter()
        .filter(|sample| sample.timestamp_ms >= start_ms && sample.timestamp_ms < end_ms)
        .count()
}

pub(crate) fn is_real_latency(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

impl DashboardSnapshot {
    pub fn build(
        samples: &[TelemetrySample],
        now_ms: i64,
        window_minutes: u32,
        translator: &Translator,
    ) -> Self {
        let latest = samples.last().cloned();
        let latest_age_ms = latest
            .as_ref()
            .map(|sample| now_ms.saturating_sub(sample.timestamp_ms));
        let stale = latest_age_ms.is_none_or(|age| age > STALE_AFTER_MS);
        let download_bps = latest
            .as_ref()
            .map_or(0.0, |sample| finite_or_zero(sample.downlink_bps));
        let upload_bps = latest
            .as_ref()
            .map_or(0.0, |sample| finite_or_zero(sample.uplink_bps));
        let latency_ms = latest
            .as_ref()
            .and_then(|sample| is_real_latency(sample.latency_ms));
        let power_w = power_bucket_mean(samples, now_ms);
        let average_power_w = recent_average(samples, |sample| Some(sample.power_w), now_ms);
        let ping_success_percent = if has_recent_readings(samples, now_ms) {
            (100.0 - recent_average(samples, |sample| Some(sample.drop_rate), now_ms) * 100.0)
                .clamp(0.0, 100.0)
        } else {
            0.0
        };
        let power_window_end_ms = power_bucket_end_ms(now_ms, POWER_BUCKET_MS);
        let sparkline_end_ms = now_ms;
        let power_sparkline_end_ms = power_window_end_ms;

        let (down_points, down_bucket_ms) = bucket_series(
            samples,
            now_ms,
            window_minutes,
            |sample| Some(sample.downlink_bps),
            Reduction::Average,
        );
        let (up_points, up_bucket_ms) = bucket_series(
            samples,
            now_ms,
            window_minutes,
            |sample| Some(sample.uplink_bps),
            Reduction::Average,
        );
        let (latency_points, latency_bucket_ms) = bucket_series(
            samples,
            now_ms,
            window_minutes,
            |sample| is_real_latency(sample.latency_ms),
            Reduction::Max,
        );
        let (power_points, power_bucket_ms) = bucket_series(
            samples,
            power_window_end_ms,
            window_minutes,
            |sample| {
                sample
                    .power_w
                    .is_finite()
                    .then_some(sample.power_w)
                    .filter(|value| *value > 0.0)
            },
            Reduction::Average,
        );

        Self {
            now_ms,
            window_minutes,
            window_samples: window_sample_count(samples, now_ms, window_minutes),
            latest,
            latest_age_ms,
            stale,
            download_bps,
            upload_bps,
            latency_ms,
            power_w,
            ping_success_percent,
            average_power_w,
            latency_quality: latency_quality(samples, now_ms),
            sky_obstructed_percent: None,
            power_window_end_ms,
            throughput: vec![
                ChartSeries {
                    id: "down",
                    label: translator.text("metrics.download"),
                    color: DOWN_COLOR,
                    points: down_points,
                    bucket_ms: down_bucket_ms,
                },
                ChartSeries {
                    id: "up",
                    label: translator.text("metrics.upload"),
                    color: UP_COLOR,
                    points: up_points,
                    bucket_ms: up_bucket_ms,
                },
            ],
            latency: vec![ChartSeries {
                id: "latency",
                label: translator.text("metrics.latency"),
                color: LATENCY_COLOR,
                points: latency_points,
                bucket_ms: latency_bucket_ms,
            }],
            power: vec![ChartSeries {
                id: "power",
                label: translator.text("metrics.powerDraw"),
                color: POWER_COLOR,
                points: power_points,
                bucket_ms: power_bucket_ms,
            }],
            sparklines: Sparklines {
                downlink: sparkline_from(
                    samples,
                    |sample| Some(sample.downlink_bps),
                    now_ms,
                    sparkline_end_ms,
                ),
                uplink: sparkline_from(
                    samples,
                    |sample| Some(sample.uplink_bps),
                    now_ms,
                    sparkline_end_ms,
                ),
                latency: sparkline_from(
                    samples,
                    |sample| sample.latency_ms,
                    now_ms,
                    sparkline_end_ms,
                ),
                power: sparkline_from(
                    samples,
                    |sample| Some(sample.power_w).filter(|value| *value > 0.0),
                    now_ms,
                    power_sparkline_end_ms,
                ),
                ping_success: sparkline_from(
                    samples,
                    |sample| Some((1.0 - sample.drop_rate) * 100.0),
                    now_ms,
                    sparkline_end_ms,
                ),
            },
        }
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

/// Compact bitrate, matching `src/lib/format.ts`.
pub fn format_throughput(bits_per_second: f64) -> (String, String) {
    let bits = finite_or_zero(bits_per_second).max(0.0);
    if bits >= 1_000_000_000.0 {
        (format!("{:.2}", bits / 1e9), "Gbps".to_string())
    } else if bits >= 1_000_000.0 {
        (format!("{:.1}", bits / 1e6), "Mbps".to_string())
    } else {
        (format!("{:.0}", bits / 1e3), "kbps".to_string())
    }
}

/// Whole watts, as both the tile and its chart label print them.
pub fn format_watts(watts: f64) -> (String, String) {
    (
        format!("{:.0}", finite_or_zero(watts).max(0.0)),
        "W".to_string(),
    )
}

/// A latency reading or an em dash. The em dash is the "not measured" value;
/// zero would claim a perfect round trip.
pub fn format_latency(latency_ms: Option<f64>) -> (String, String) {
    match is_real_latency(latency_ms) {
        Some(value) => (format!("{value:.0}"), "ms".to_string()),
        None => ("—".to_string(), "ms".to_string()),
    }
}

pub fn format_ping_success(percent: f64) -> (String, String) {
    (
        format!("{:.1}", finite_or_zero(percent).clamp(0.0, 100.0)),
        "%".to_string(),
    )
}

pub fn format_sky_obstructed(percent: Option<f64>) -> (String, String) {
    match percent.filter(|value| value.is_finite()) {
        Some(value) => (format!("{value:.2}"), "%".to_string()),
        None => ("—".to_string(), "%".to_string()),
    }
}

/// The power card's projection from settled recent draw.
pub fn kwh_per_day(average_power_w: f64) -> String {
    format!(
        "{:.2}",
        finite_or_zero(average_power_w).max(0.0) * 24.0 / 1000.0
    )
}

/// A coarse age for the stale banner, worded by the shared catalogs. The same
/// thresholds as `formatRelativeTime` in `src/lib/format.ts`.
pub fn format_relative_age(age_ms: i64, translator: &Translator) -> String {
    let seconds = age_ms.max(0) / 1000;
    if seconds < 60 {
        return translator.text("time.justNow");
    }
    if seconds < 3_600 {
        let count = (seconds as f64 / 60.0).round() as i64;
        return translator.text_with("time.minutesAgo", &[("count", &count.to_string())]);
    }
    if seconds < 86_400 {
        let count = (seconds as f64 / 3_600.0).round() as i64;
        let key = if count == 1 {
            "time.hoursAgo_one"
        } else {
            "time.hoursAgo_other"
        };
        return translator.text_with(key, &[("count", &count.to_string())]);
    }
    let count = (seconds as f64 / 86_400.0).round() as i64;
    let key = if count == 1 {
        "time.daysAgo_one"
    } else {
        "time.daysAgo_other"
    };
    translator.text_with(key, &[("count", &count.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Locale, Translator};
    use crate::readings::SPARKLINE_MS;

    const NOW_MS: i64 = 1_781_524_800_000;

    fn sample(
        at_ms: i64,
        down: f64,
        up: f64,
        latency: Option<f64>,
        power: f64,
        drop: f64,
    ) -> TelemetrySample {
        TelemetrySample {
            timestamp_ms: at_ms,
            latency_ms: latency,
            drop_rate: drop,
            downlink_bps: down,
            uplink_bps: up,
            power_w: power,
            router_latency_ms: None,
            router_ping_success_percent: None,
        }
    }

    fn translator() -> Translator {
        Translator::new(Locale::En)
    }

    // SPEC-DASH-001: the tiles read the recorder's newest sample and recent
    // averages; a status-only metric is absent, not zero.
    #[test]
    fn tiles_come_from_the_recorders_samples() {
        let samples = [
            sample(
                NOW_MS - 10_000,
                1_000_000.0,
                500_000.0,
                Some(25.0),
                40.0,
                0.1,
            ),
            sample(
                NOW_MS - 1_000,
                3_000_000.0,
                1_500_000.0,
                Some(35.0),
                60.0,
                0.2,
            ),
        ];
        let snapshot = DashboardSnapshot::build(&samples, NOW_MS, 15, &translator());
        assert_eq!(snapshot.window_samples, 2);
        assert_eq!(snapshot.download_bps, 3_000_000.0);
        assert_eq!(snapshot.upload_bps, 1_500_000.0);
        assert_eq!(snapshot.latency_ms, Some(35.0));
        // Both samples of the latest completed bucket are averaged, and the
        // sample recorded just now sits in the still-open bucket.
        assert!(snapshot.power_w >= 0.0);
        assert_eq!(snapshot.ping_success_percent, 85.0);
        assert_eq!(snapshot.sky_obstructed_percent, None);
        assert_eq!(
            format_sky_obstructed(None),
            ("—".to_string(), "%".to_string())
        );
        assert_eq!(format_latency(None), ("—".to_string(), "ms".to_string()));
    }

    // SPEC-DASH-002: changing the chart window re-cuts the series but leaves the
    // live tiles on the newest sample.
    #[test]
    fn window_selection_only_recuts_the_charts() {
        let samples = [
            sample(NOW_MS - 30 * 60_000, 100.0, 10.0, Some(50.0), 20.0, 0.0),
            sample(NOW_MS - 1_000, 100.0, 10.0, Some(20.0), 50.0, 0.0),
        ];
        let quarter = DashboardSnapshot::build(&samples, NOW_MS, 15, &translator());
        let hour = DashboardSnapshot::build(&samples, NOW_MS, 60, &translator());
        assert_eq!(quarter.window_samples, 1);
        assert_eq!(hour.window_samples, 2);
        assert_eq!(quarter.download_bps, hour.download_bps);
        assert_eq!(quarter.latency_ms, hour.latency_ms);
        assert_eq!(quarter.power_w, hour.power_w);
    }

    // SPEC-DASH-003: throughput averages a bucket, latency takes its maximum,
    // and a jump larger than a bucket breaks the line.
    #[test]
    fn chart_buckets_use_the_renderers_reductions() {
        let start = NOW_MS - 15 * 60_000;
        let samples = [
            sample(start + 1_000, 100.0, 10.0, Some(10.0), 40.0, 0.0),
            sample(start + 2_000, 300.0, 30.0, Some(40.0), 60.0, 0.0),
            sample(start + 800_000, 500.0, 50.0, Some(20.0), 80.0, 0.0),
        ];
        let snapshot = DashboardSnapshot::build(&samples, NOW_MS, 15, &translator());
        assert_eq!(snapshot.throughput[0].points[0].value, 200.0);
        assert_eq!(snapshot.latency[0].points[0].value, 40.0);
        assert_eq!(snapshot.power[0].points[0].value, 50.0);
        assert!(!snapshot.throughput[0].is_gap_free());
        assert!(snapshot.throughput[0].gap_after(0));
    }

    // SPEC-DASH-004: no samples means the searching hero, not an empty chart
    // pretending the link is idle.
    #[test]
    fn an_empty_recorder_has_no_dashboard_values() {
        let snapshot = DashboardSnapshot::build(&[], NOW_MS, 15, &translator());
        assert_eq!(snapshot.window_samples, 0);
        assert!(snapshot.latest.is_none());
        assert!(snapshot.stale);
        assert!(snapshot.throughput.iter().all(ChartSeries::is_empty));
        assert!(snapshot.latency.iter().all(ChartSeries::is_empty));
        assert!(snapshot.power.iter().all(ChartSeries::is_empty));
        assert_eq!(snapshot.ping_success_percent, 0.0);
    }

    // SPEC-DASH-005: a feed silent for six seconds is stale, and a minute with
    // no readings reports 0% rather than a perfect score.
    #[test]
    fn a_silent_feed_is_stale_and_ping_success_is_zero() {
        let stale = [sample(NOW_MS - 7_000, 0.0, 0.0, None, 0.0, 0.0)];
        let snapshot = DashboardSnapshot::build(&stale, NOW_MS, 15, &translator());
        assert!(snapshot.stale);
        assert_eq!(snapshot.latest_age_ms, Some(7_000));

        let silent = [sample(NOW_MS - 70_000, 0.0, 0.0, None, 0.0, 0.0)];
        let snapshot = DashboardSnapshot::build(&silent, NOW_MS, 15, &translator());
        assert_eq!(snapshot.ping_success_percent, 0.0);

        let live = [sample(NOW_MS - 1_000, 0.0, 0.0, None, 0.0, 0.2)];
        let snapshot = DashboardSnapshot::build(&live, NOW_MS, 15, &translator());
        assert!(!snapshot.stale);
        assert_eq!(snapshot.ping_success_percent, 80.0);
        assert_eq!(format_relative_age(7_000, &translator()), "just now");
        assert_eq!(format_relative_age(90_000, &translator()), "2 min ago");
        assert_eq!(format_relative_age(3_600_000, &translator()), "1 hour ago");
    }

    // SPEC-DASH-006: the same thresholds and translated labels as the renderer.
    #[test]
    fn formatting_and_labels_are_shared_with_the_typescript_app() {
        assert_eq!(
            format_throughput(999_999.0),
            ("1000".to_string(), "kbps".to_string())
        );
        assert_eq!(
            format_throughput(1_500_000.0),
            ("1.5".to_string(), "Mbps".to_string())
        );
        assert_eq!(
            format_throughput(2_400_000_000.0),
            ("2.40".to_string(), "Gbps".to_string())
        );
        assert_eq!(format_watts(45.4), ("45".to_string(), "W".to_string()));
        assert_eq!(
            format_ping_success(99.86),
            ("99.9".to_string(), "%".to_string())
        );
        assert_eq!(kwh_per_day(100.0), "2.40");

        let snapshots = DashboardSnapshot::build(
            &[sample(NOW_MS, 0.0, 0.0, Some(20.0), 50.0, 0.0)],
            NOW_MS,
            15,
            &Translator::new(Locale::Es),
        );
        assert_eq!(snapshots.throughput[0].label, "Descarga");
        assert_eq!(snapshots.throughput[1].label, "Subida");
        assert_eq!(snapshots.latency[0].label, "Latencia");
        assert_eq!(snapshots.power[0].label, "Consumo");
    }

    #[test]
    fn window_slices_exclude_old_samples() {
        let samples = [
            sample(NOW_MS - 40 * 60_000, 1.0, 1.0, None, 1.0, 0.0),
            sample(NOW_MS - 5 * 60_000, 1.0, 1.0, None, 1.0, 0.0),
        ];
        assert_eq!(window_sample_count(&samples, NOW_MS, 15), 1);
        assert_eq!(window_sample_count(&samples, NOW_MS, 60), 2);
        assert_eq!(SPARKLINE_MS, 90_000);
    }
}
