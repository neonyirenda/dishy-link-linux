//! What a dashboard tile's detail shows: averages over its own window, the
//! chart, the latency distribution, the router chart, and window energy.
//!
//! Port of upstream `components/dashboard/StatDetailPanel.tsx` and upstream `lib/statDetails.ts`
//! (spec 0172). Pure, so every figure is testable without a display. It reads
//! only the recorder's samples and summaries.

use dishy_core::telemetry::TelemetrySample;
use dishy_core::usage::UsageSummary;

use crate::dashboard::{
    ChartSeries, DOWN_COLOR, LATENCY_COLOR, POWER_COLOR, Reduction, UP_COLOR, bucket_series,
    format_latency, format_ping_success, format_throughput, format_watts, is_real_latency,
};
use crate::format::to_fixed;
use crate::i18n::Translator;

/// The windows a detail offers, in minutes.
pub const DETAIL_WINDOWS: [u32; 3] = [15, 60, 360];
/// The router line's colour, the dashboard's warm orange.
const ROUTER_COLOR: (f64, f64, f64) = (1.0, 0.69, 0.25);
/// A step longer than this is an outage, not covered time.
const COVERAGE_GAP_MS: i64 = 5_000;
/// 2 ms bins from 0–100 ms plus one overflow bar.
pub const HISTOGRAM_BINS: usize = 51;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Download,
    Upload,
    Latency,
    Power,
    PingSuccess,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowCoverage {
    None,
    Full,
    Partial { covered_minutes: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistogramPanel {
    pub label: String,
    pub color: (f64, f64, f64),
    pub bins: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecondaryChart {
    pub title: String,
    pub note: String,
    /// `None` when the window holds no router readings; `empty_note` says why.
    pub figures: Option<Vec<(String, (String, String))>>,
    pub empty_note: String,
    pub series: Vec<ChartSeries>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetailView {
    pub title: String,
    pub average: (String, String),
    pub current: (String, String),
    pub series: Vec<ChartSeries>,
    pub end_ms: i64,
    pub histogram: Vec<HistogramPanel>,
    pub secondary: Option<SecondaryChart>,
    /// "0.42 kWh" and its coverage note, for the power detail.
    pub energy: Option<(String, String)>,
    pub explainer_title: String,
    pub explainer: String,
}

/// Mean of the finite readings; zero for a window with none.
pub fn average_of(
    samples: &[TelemetrySample],
    value: impl Fn(&TelemetrySample) -> Option<f64>,
) -> f64 {
    let values: Vec<f64> = samples
        .iter()
        .filter_map(value)
        .filter(|value| value.is_finite())
        .collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Steps between consecutive readings that are short enough to be covered
/// time rather than an outage.
fn covered_steps(samples: &[TelemetrySample]) -> impl Iterator<Item = (&TelemetrySample, i64)> {
    samples.windows(2).filter_map(|pair| {
        let step = pair[1].timestamp_ms - pair[0].timestamp_ms;
        (step > 0 && step <= COVERAGE_GAP_MS).then_some((&pair[0], step))
    })
}

/// How much of a window was observed, from steps of 5 s or less.
pub fn window_coverage(samples: &[TelemetrySample], window_minutes: u32) -> WindowCoverage {
    if samples.is_empty() {
        return WindowCoverage::None;
    }
    let covered_ms: i64 = covered_steps(samples).map(|(_, step)| step).sum();
    let window_ms = i64::from(window_minutes) * 60_000;
    if covered_ms as f64 >= window_ms as f64 * 0.95 {
        WindowCoverage::Full
    } else {
        WindowCoverage::Partial {
            covered_minutes: covered_ms as f64 / 60_000.0,
        }
    }
}

/// Energy in kWh from ΣW·Δt, counting only steps short enough to be readings.
pub fn energy_kwh(samples: &[TelemetrySample]) -> f64 {
    let watt_ms: f64 = covered_steps(samples)
        .map(|(sample, step)| {
            let watts = if sample.power_w.is_finite() {
                sample.power_w
            } else {
                0.0
            };
            watts * step as f64
        })
        .sum();
    watt_ms / 3_600_000_000.0
}

/// Percent of readings per 2 ms bin, with a final ≥100 ms bin.
pub fn bin_latencies(values: &[f64]) -> Vec<f64> {
    let mut bins = vec![0.0; HISTOGRAM_BINS];
    if values.is_empty() {
        return bins;
    }
    for value in values {
        let index = ((value / 2.0).floor().max(0.0) as usize).min(HISTOGRAM_BINS - 1);
        bins[index] += 1.0;
    }
    let total = values.len() as f64;
    bins.iter_mut()
        .for_each(|count| *count = *count / total * 100.0);
    bins
}

pub fn coverage_note(coverage: WindowCoverage, translator: &Translator) -> String {
    match coverage {
        WindowCoverage::None => translator.text("metrics.energyUsedNothing"),
        WindowCoverage::Full => translator.text("metrics.energyUsedFull"),
        WindowCoverage::Partial { covered_minutes } if covered_minutes >= 1.0 => translator
            .text_with(
                "metrics.energyUsedMinutes",
                &[("count", &format!("{}", covered_minutes.round()))],
            ),
        WindowCoverage::Partial { .. } => translator.text("metrics.energyUsedUnderMinute"),
    }
}

/// The note under energy taken from a recorder summary.
pub fn summary_note(fraction: f64, translator: &Translator) -> String {
    if fraction >= 0.95 {
        translator.text("metrics.energyUsedFull")
    } else {
        translator.text_with(
            "metrics.energyUsedPercent",
            &[("percent", &format!("{}", (fraction * 100.0).round()))],
        )
    }
}

/// One line of a detail's chart: what it reads, how it buckets, and its name.
struct Line {
    id: &'static str,
    label: String,
    color: (f64, f64, f64),
    reduction: Reduction,
    value: fn(&TelemetrySample) -> Option<f64>,
}

fn dish_latency(sample: &TelemetrySample) -> Option<f64> {
    is_real_latency(sample.latency_ms)
}

fn router_latency(sample: &TelemetrySample) -> Option<f64> {
    is_real_latency(sample.router_latency_ms)
}

fn ping_success(sample: &TelemetrySample) -> Option<f64> {
    sample
        .drop_rate
        .is_finite()
        .then_some((1.0 - sample.drop_rate) * 100.0)
}

fn lines(kind: DetailKind, translator: &Translator) -> Vec<Line> {
    let line = |id, key: &str, color, reduction, value| Line {
        id,
        label: translator.text(key),
        color,
        reduction,
        value,
    };
    match kind {
        DetailKind::Download => vec![line(
            "down",
            "metrics.download",
            DOWN_COLOR,
            Reduction::Average,
            |s| Some(s.downlink_bps),
        )],
        DetailKind::Upload => vec![line(
            "up",
            "metrics.upload",
            UP_COLOR,
            Reduction::Average,
            |s| Some(s.uplink_bps),
        )],
        DetailKind::Latency => vec![
            line(
                "latency",
                "common.starlink",
                LATENCY_COLOR,
                Reduction::Max,
                dish_latency,
            ),
            line(
                "router-latency",
                "common.router",
                ROUTER_COLOR,
                Reduction::Average,
                router_latency,
            ),
        ],
        DetailKind::Power => vec![line(
            "power",
            "metrics.powerDraw",
            POWER_COLOR,
            Reduction::Average,
            |s| {
                s.power_w
                    .is_finite()
                    .then_some(s.power_w)
                    .filter(|watts| *watts > 0.0)
            },
        )],
        DetailKind::PingSuccess => vec![line(
            "ping-success",
            "metrics.pingSuccess",
            LATENCY_COLOR,
            Reduction::Min,
            ping_success,
        )],
    }
}

fn chart(
    lines: &[Line],
    samples: &[TelemetrySample],
    end_ms: i64,
    window_minutes: u32,
) -> Vec<ChartSeries> {
    lines
        .iter()
        .map(|line| {
            let (points, bucket_ms) =
                bucket_series(samples, end_ms, window_minutes, line.value, line.reduction);
            ChartSeries {
                id: line.id,
                label: line.label.clone(),
                color: line.color,
                points,
                bucket_ms,
            }
        })
        .collect()
}

fn format_big(kind: DetailKind, value: f64) -> (String, String) {
    match kind {
        DetailKind::Download | DetailKind::Upload => format_throughput(value),
        DetailKind::Latency => format_latency(Some(value)),
        DetailKind::Power => format_watts(value),
        DetailKind::PingSuccess => format_ping_success(value),
    }
}

fn window(samples: &[TelemetrySample], end_ms: i64, window_minutes: u32) -> Vec<TelemetrySample> {
    let start_ms = end_ms - i64::from(window_minutes) * 60_000;
    samples
        .iter()
        .filter(|sample| (start_ms..=end_ms).contains(&sample.timestamp_ms))
        .cloned()
        .collect()
}

/// Everything a detail shows for one window. `current` is the tile's own
/// figure; `energy_summary` is the recorder's summary for the window, when
/// one was fetched.
#[allow(clippy::too_many_arguments)]
pub fn detail_view(
    kind: DetailKind,
    samples: &[TelemetrySample],
    current: f64,
    window_minutes: u32,
    now_ms: i64,
    power_end_ms: i64,
    energy_summary: Option<&UsageSummary>,
    translator: &Translator,
) -> DetailView {
    let lines = lines(kind, translator);
    // Power steps with the tile's settled 5 s bucket instead of sliding.
    let end_ms = if kind == DetailKind::Power {
        power_end_ms
    } else {
        now_ms
    };
    let windowed = window(samples, now_ms, window_minutes);
    let primary = lines[0].value;
    let (title_key, explainer_title_key, explainer_key) = match kind {
        DetailKind::Download => (
            "metrics.download",
            "metrics.whatIsDownload",
            "metrics.downloadExplainer",
        ),
        DetailKind::Upload => (
            "metrics.upload",
            "metrics.whatIsUpload",
            "metrics.uploadExplainer",
        ),
        DetailKind::Latency => (
            "metrics.latency",
            "metrics.whatIsLatency",
            "metrics.latencyExplainer",
        ),
        DetailKind::Power => (
            "metrics.powerDraw",
            "metrics.whatIsPowerDraw",
            "metrics.powerExplainer",
        ),
        DetailKind::PingSuccess => (
            "metrics.starlinkPingSuccess",
            "metrics.whatIsPingSuccess",
            "metrics.pingExplainer",
        ),
    };

    let histogram = if kind == DetailKind::Latency {
        lines
            .iter()
            .filter_map(|line| {
                let values: Vec<f64> = windowed.iter().filter_map(line.value).collect();
                (!values.is_empty()).then(|| HistogramPanel {
                    label: line.label.clone(),
                    color: line.color,
                    bins: bin_latencies(&values),
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    let secondary = (kind == DetailKind::PingSuccess).then(|| {
        let router = |sample: &TelemetrySample| sample.router_ping_success_percent;
        let has_data = windowed
            .iter()
            .any(|sample| router(sample).is_some_and(f64::is_finite));
        let current = samples
            .iter()
            .rev()
            .find_map(|sample| router(sample).filter(|value| value.is_finite()));
        let figures = has_data.then(|| {
            let mut figures = vec![(
                translator.text("metrics.average"),
                format_ping_success(average_of(&windowed, router)),
            )];
            if let Some(current) = current {
                figures.push((
                    translator.text("metrics.current"),
                    format_ping_success(current),
                ));
            }
            figures
        });
        let router_line = [Line {
            id: "router-ping-success",
            label: translator.text("common.router"),
            color: ROUTER_COLOR,
            reduction: Reduction::Average,
            value: |sample| sample.router_ping_success_percent,
        }];
        SecondaryChart {
            title: translator.text("metrics.routerPingSuccess"),
            note: translator.text("metrics.routerPingNote"),
            figures,
            empty_note: translator.text("metrics.routerPingEmpty"),
            series: chart(&router_line, samples, end_ms, window_minutes),
        }
    });

    let energy = (kind == DetailKind::Power).then(|| {
        let historian = energy_summary.filter(|_| matches!(window_minutes, 60 | 360));
        let (kwh, note) = match historian {
            Some(summary) => (
                summary.total_kwh,
                summary_note(summary.coverage.fraction, translator),
            ),
            None => (
                energy_kwh(&windowed),
                coverage_note(window_coverage(&windowed, window_minutes), translator),
            ),
        };
        (format!("{kwh:.2} kWh"), note)
    });

    DetailView {
        title: translator.text(title_key),
        average: format_big(kind, average_of(&windowed, primary)),
        current: format_big(kind, current),
        series: chart(&lines, samples, end_ms, window_minutes),
        end_ms,
        histogram,
        secondary,
        energy,
        explainer_title: translator.text(explainer_title_key),
        explainer: translator.text(explainer_key),
    }
}

/// What the latency Quality view shows for one range.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityView {
    pub score: String,
    pub grade: Option<char>,
    pub figures: Vec<(String, (String, String))>,
    pub collected: String,
    /// One bar per slot: its start and p95, `None` where nothing was recorded.
    pub bars: Vec<(i64, Option<f64>)>,
}

pub fn quality_view(
    summary: &dishy_core::latency_summary::LatencySummary,
    translator: &Translator,
) -> QualityView {
    let figure = |label: String, value: Option<f64>, unit: &str| {
        let shown = match value {
            Some(value) => (
                to_fixed(value, if unit == "%" { 1 } else { 0 }),
                unit.to_owned(),
            ),
            None => ("—".to_owned(), String::new()),
        };
        (label, shown)
    };
    let dish = &summary.dish;
    let mut figures = vec![
        figure("p95".into(), dish.p95, "ms"),
        figure("p99".into(), dish.p99, "ms"),
        figure(translator.text("latencyQuality.jitter"), dish.jitter, "ms"),
        figure(
            translator.text("latencyQuality.packetLoss"),
            dish.drop_pct,
            "%",
        ),
    ];
    if dish.spread.is_some() {
        figures.push(figure(
            translator.text("latencyQuality.spread"),
            dish.spread,
            "ms",
        ));
    }
    let percent = (summary.coverage_fraction * 100.0).round();
    let collected_key = if percent < 95.0 {
        "latencyQuality.collectedPartial"
    } else {
        "latencyQuality.collected"
    };
    QualityView {
        score: summary.score.to_string(),
        grade: (dish.count > 0).then_some(summary.grade),
        figures,
        collected: translator.text_with(collected_key, &[("percent", &format!("{percent}"))]),
        bars: summary.bars.iter().map(|bar| (bar.t, bar.p95)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    const NOW: i64 = 1_800_000_000_000;

    fn sample(
        offset_s: i64,
        latency: Option<f64>,
        router: Option<f64>,
        power: f64,
    ) -> TelemetrySample {
        TelemetrySample {
            timestamp_ms: NOW - offset_s * 1_000,
            latency_ms: latency,
            drop_rate: 0.0,
            downlink_bps: 50e6,
            uplink_bps: 10e6,
            power_w: power,
            router_latency_ms: router,
            router_ping_success_percent: None,
        }
    }

    fn english() -> Translator {
        Translator::new(Locale::En)
    }

    // SPEC-DETAIL-002: averages skip missing readings; coverage counts only
    // short steps and words itself as Electron does.
    #[test]
    fn averages_and_coverage_follow_the_electron_rules() {
        let samples: Vec<_> = (0..10)
            .rev()
            .map(|s| sample(s, if s == 3 { None } else { Some(20.0) }, None, 50.0))
            .collect();
        assert_eq!(average_of(&samples, |s| s.latency_ms), 20.0);
        assert_eq!(average_of(&[], |s| s.latency_ms), 0.0);

        assert_eq!(window_coverage(&[], 15), WindowCoverage::None);
        let full: Vec<_> = (0..900)
            .rev()
            .map(|s| sample(s, Some(20.0), None, 50.0))
            .collect();
        assert_eq!(window_coverage(&full, 15), WindowCoverage::Full);
        match window_coverage(&samples, 15) {
            WindowCoverage::Partial { covered_minutes } => {
                assert!((covered_minutes - 0.15).abs() < 1e-9)
            }
            other => panic!("{other:?}"),
        }
        let translator = english();
        assert_eq!(
            coverage_note(WindowCoverage::None, &translator),
            "energy used nothing recorded in this window"
        );
        assert_eq!(
            coverage_note(WindowCoverage::Full, &translator),
            "energy used over the selected window"
        );
        assert_eq!(
            coverage_note(
                WindowCoverage::Partial {
                    covered_minutes: 0.4
                },
                &translator
            ),
            "energy used recorded < 1 min of this window"
        );
        assert_eq!(
            coverage_note(
                WindowCoverage::Partial {
                    covered_minutes: 7.6
                },
                &translator
            ),
            "energy used recorded 8 min of this window"
        );
        assert_eq!(
            summary_note(0.97, &translator),
            "energy used over the selected window"
        );
        assert_eq!(
            summary_note(0.5, &translator),
            "energy used recorded 50% of this window"
        );

        // 3600 s at 1000 W = 1 kWh; a gap longer than 5 s adds nothing.
        let hour: Vec<_> = (0..=3600)
            .rev()
            .map(|s| sample(s, None, None, 1000.0))
            .collect();
        assert!((energy_kwh(&hour) - 1.0).abs() < 1e-9);
        let gapped = [
            sample(100, None, None, 1000.0),
            sample(0, None, None, 1000.0),
        ];
        assert_eq!(energy_kwh(&gapped), 0.0);
    }

    // SPEC-DETAIL-006: the Quality view's score, figures, and coverage line use
    // the catalog's words and Electron's decimals.
    #[test]
    fn the_quality_view_words_the_summary() {
        use dishy_core::latency_summary::{LatencyBarSummary, LatencyStatMetrics, LatencySummary};
        use dishy_core::usage::UsageRange;
        let translator = english();
        let summary = LatencySummary {
            range: UsageRange::OneHour,
            sampled_seconds: 1_800,
            expected_seconds: 3_600,
            coverage_fraction: 0.5,
            score: 87,
            grade: 'B',
            dish: LatencyStatMetrics {
                count: 1_800,
                mean: Some(30.0),
                p50: Some(28.0),
                p95: Some(41.4),
                p99: Some(62.0),
                jitter: Some(6.6),
                spread: Some(34.0),
                drop_pct: Some(0.25),
            },
            router: None,
            bars: vec![LatencyBarSummary {
                t: 10,
                expected_seconds: 300,
                sampled_seconds: 300,
                p95: Some(41.0),
                p99: None,
                jitter: None,
                drop_pct: None,
            }],
        };
        let view = quality_view(&summary, &translator);
        assert_eq!((view.score.as_str(), view.grade), ("87", Some('B')));
        assert_eq!(
            view.figures[0],
            ("p95".to_string(), ("41".to_string(), "ms".to_string()))
        );
        assert_eq!(
            view.figures[3],
            (
                "Packet loss".to_string(),
                ("0.3".to_string(), "%".to_string())
            )
        );
        assert_eq!(view.figures[4].0, "p99 − p50");
        assert!(view.collected.starts_with("collected 50% of this period —"));
        assert_eq!(view.bars, [(10, Some(41.0))]);

        let empty = LatencySummary {
            score: 0,
            dish: LatencyStatMetrics {
                count: 0,
                p95: None,
                spread: None,
                ..summary.dish.clone()
            },
            ..summary
        };
        let view = quality_view(&empty, &translator);
        assert_eq!(view.figures[0].1, ("—".to_string(), String::new()));
        assert_eq!(view.figures.len(), 4, "no spread figure without one");
    }

    // SPEC-DETAIL-003: 2 ms bins to 100 ms plus overflow, as percentages.
    #[test]
    fn latency_bins_are_percentages_with_an_overflow_bar() {
        let bins = bin_latencies(&[1.0, 3.0, 3.5, 250.0]);
        assert_eq!(bins.len(), HISTOGRAM_BINS);
        assert_eq!((bins[0], bins[1], bins[50]), (25.0, 50.0, 25.0));
        assert!(bin_latencies(&[]).iter().all(|pct| *pct == 0.0));
    }

    // SPEC-DETAIL-001, SPEC-DETAIL-003, SPEC-DETAIL-004, SPEC-DETAIL-005: each
    // detail carries its figures, its
    // series, and its catalog copy; latency overlays the router; ping success
    // explains an empty router window; power shows window energy.
    #[test]
    fn details_carry_their_figures_series_and_copy() {
        let translator = english();
        let samples: Vec<_> = (0..120)
            .rev()
            .map(|s| {
                sample(
                    s,
                    Some(20.0 + (s % 4) as f64),
                    if s < 60 { Some(30.0) } else { None },
                    60.0,
                )
            })
            .collect();

        let download = detail_view(
            DetailKind::Download,
            &samples,
            50e6,
            15,
            NOW,
            NOW,
            None,
            &translator,
        );
        assert_eq!(download.title, "Download");
        assert_eq!(download.explainer_title, "What is download?");
        assert_eq!(download.series.len(), 1);
        assert!(download.histogram.is_empty());

        let latency = detail_view(
            DetailKind::Latency,
            &samples,
            21.0,
            15,
            NOW,
            NOW,
            None,
            &translator,
        );
        assert_eq!(latency.series.len(), 2, "dish and router on one axis");
        assert_eq!(latency.histogram.len(), 2);
        assert_eq!(latency.current, ("21".to_string(), "ms".to_string()));

        let ping = detail_view(
            DetailKind::PingSuccess,
            &samples,
            100.0,
            15,
            NOW,
            NOW,
            None,
            &translator,
        );
        assert_eq!(ping.title, "Starlink ping success");
        let router = ping.secondary.expect("router chart");
        assert_eq!(router.title, "Router ping success");
        assert!(
            router.figures.is_none(),
            "no router readings in this window"
        );
        assert!(router.empty_note.starts_with("nothing recorded"));

        let power = detail_view(
            DetailKind::Power,
            &samples,
            60.0,
            15,
            NOW,
            NOW,
            None,
            &translator,
        );
        let (kwh, note) = power.energy.expect("window energy");
        assert!(kwh.ends_with("kWh"), "{kwh}");
        assert!(note.starts_with("energy used recorded"), "{note}");
    }
}
