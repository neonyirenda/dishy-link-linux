//! Latency quality over a range: percentiles, jitter, loss, and a grade.
//!
//! Port of `core/latencySummary.ts`. Reads the latency minute store the
//! recorder already writes; it adds no device read (spec 0172).

use serde::Serialize;

use crate::calendar::Calendar;
#[cfg(test)]
use crate::latency::LATENCY_BIN_COUNT;
use crate::latency::{
    LATENCY_BIN_UPPER_EDGES_MS, LATENCY_OVERFLOW_EDGE_MS, LatencyMinuteBucket, LatencySourceStats,
};
use crate::usage::{RangeSlots, UsageRange};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyStatMetrics {
    pub count: u32,
    pub mean: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    /// Standard deviation of the readings.
    pub jitter: Option<f64>,
    /// p99 − p50.
    pub spread: Option<f64>,
    pub drop_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyBarSummary {
    pub t: i64,
    pub expected_seconds: i64,
    pub sampled_seconds: i64,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub jitter: Option<f64>,
    pub drop_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencySummary {
    pub range: UsageRange,
    pub sampled_seconds: i64,
    pub expected_seconds: i64,
    pub coverage_fraction: f64,
    pub score: u32,
    pub grade: char,
    pub dish: LatencyStatMetrics,
    pub router: Option<LatencyStatMetrics>,
    pub bars: Vec<LatencyBarSummary>,
}

/// Interpolated percentile from the histogram. A percentile landing in the
/// overflow bin saturates at the finite top edge.
pub fn percentile(bins: &[u32], p: f64) -> Option<f64> {
    let total: u64 = bins.iter().map(|count| u64::from(*count)).sum();
    if total == 0 {
        return None;
    }
    let target = p * total as f64;
    let mut before = 0.0;
    for (index, count) in bins.iter().enumerate() {
        let count = f64::from(*count);
        let after = before + count;
        if count > 0.0 && target <= after {
            let lower = if index == 0 {
                0.0
            } else {
                LATENCY_BIN_UPPER_EDGES_MS[index - 1]
            };
            let upper = LATENCY_BIN_UPPER_EDGES_MS
                .get(index)
                .copied()
                .unwrap_or(LATENCY_OVERFLOW_EDGE_MS);
            return Some(lower + (target - before) / count * (upper - lower));
        }
        before = after;
    }
    Some(LATENCY_OVERFLOW_EDGE_MS)
}

/// 1 at or under `good`, 0 at or over `bad`, linear between.
fn band_score(value: f64, good: f64, bad: f64) -> f64 {
    if value <= good {
        1.0
    } else if value >= bad {
        0.0
    } else {
        (bad - value) / (bad - good)
    }
}

pub fn quality_score(metrics: &LatencyStatMetrics) -> u32 {
    if metrics.count == 0 {
        return 0;
    }
    let latency = metrics.p95.map_or(0.0, |ms| band_score(ms, 20.0, 250.0));
    let tail = metrics.p99.map_or(0.0, |ms| band_score(ms, 50.0, 400.0));
    let jitter = metrics.jitter.map_or(0.0, |ms| band_score(ms, 5.0, 60.0));
    let loss = metrics
        .drop_pct
        .map_or(1.0, |pct| band_score(pct, 0.0, 5.0));
    let score = 100.0 * (0.4 * latency + 0.3 * jitter + 0.2 * tail + 0.1 * loss);
    score.clamp(0.0, 100.0).round() as u32
}

pub fn grade_for(score: u32) -> char {
    match score {
        90.. => 'A',
        75..=89 => 'B',
        60..=74 => 'C',
        40..=59 => 'D',
        _ => 'F',
    }
}

struct Merged {
    dish: LatencySourceStats,
    router: LatencySourceStats,
    drop_sum: f64,
    drop_count: u32,
    samples: i64,
}

fn merge<'a>(buckets: impl IntoIterator<Item = &'a LatencyMinuteBucket>) -> Merged {
    let mut merged = Merged {
        dish: LatencySourceStats::default(),
        router: LatencySourceStats::default(),
        drop_sum: 0.0,
        drop_count: 0,
        samples: 0,
    };
    for bucket in buckets {
        for (into, from) in [
            (&mut merged.dish, &bucket.dish),
            (&mut merged.router, &bucket.router),
        ] {
            for (bin, count) in into.bins.iter_mut().zip(&from.bins) {
                *bin += count;
            }
            into.count += from.count;
            into.sum_ms += from.sum_ms;
            into.sum_sq_ms += from.sum_sq_ms;
        }
        merged.drop_sum += bucket.drop_sum;
        merged.drop_count += bucket.drop_count;
        merged.samples += i64::from(bucket.samples);
    }
    merged
}

fn metrics_of(stats: &LatencySourceStats, drop_sum: f64, drop_count: u32) -> LatencyStatMetrics {
    let drop_pct = (drop_count > 0).then(|| drop_sum / f64::from(drop_count) * 100.0);
    if stats.count == 0 {
        return LatencyStatMetrics {
            count: 0,
            mean: None,
            p50: None,
            p95: None,
            p99: None,
            jitter: None,
            spread: None,
            drop_pct,
        };
    }
    let count = f64::from(stats.count);
    let mean = stats.sum_ms / count;
    let variance = (stats.sum_sq_ms / count - mean * mean).max(0.0);
    let (p50, p95, p99) = (
        percentile(&stats.bins, 0.5),
        percentile(&stats.bins, 0.95),
        percentile(&stats.bins, 0.99),
    );
    LatencyStatMetrics {
        count: stats.count,
        mean: Some(mean),
        p50,
        p95,
        p99,
        jitter: Some(variance.sqrt()),
        spread: p50.zip(p99).map(|(p50, p99)| p99 - p50),
        drop_pct,
    }
}

pub fn summarize_latency(
    buckets: &[LatencyMinuteBucket],
    range: UsageRange,
    now_ms: i64,
    calendar: &dyn Calendar,
) -> LatencySummary {
    let slots = RangeSlots::new(range, now_ms, calendar);
    let in_range: Vec<&LatencyMinuteBucket> = buckets
        .iter()
        .filter(|bucket| slots.contains(bucket.minute))
        .collect();
    let mut by_slot: std::collections::BTreeMap<i64, Vec<&LatencyMinuteBucket>> =
        std::collections::BTreeMap::new();
    for bucket in &in_range {
        by_slot
            .entry(slots.key(bucket.minute))
            .or_default()
            .push(bucket);
    }
    let bars = slots
        .slots()
        .into_iter()
        .map(|(t, expected_seconds)| {
            let Some(group) = by_slot.get(&t) else {
                return LatencyBarSummary {
                    t,
                    expected_seconds,
                    sampled_seconds: 0,
                    p95: None,
                    p99: None,
                    jitter: None,
                    drop_pct: None,
                };
            };
            let merged = merge(group.iter().copied());
            let metrics = metrics_of(&merged.dish, merged.drop_sum, merged.drop_count);
            LatencyBarSummary {
                t,
                expected_seconds,
                sampled_seconds: merged.samples,
                p95: metrics.p95,
                p99: metrics.p99,
                jitter: metrics.jitter,
                drop_pct: metrics.drop_pct,
            }
        })
        .collect();
    let merged = merge(in_range.iter().copied());
    let dish = metrics_of(&merged.dish, merged.drop_sum, merged.drop_count);
    let router = (merged.router.count > 0).then(|| metrics_of(&merged.router, 0.0, 0));
    let score = quality_score(&dish);
    let expected_seconds = (slots.end_sec - slots.start_sec).max(1);
    LatencySummary {
        range,
        sampled_seconds: merged.samples,
        expected_seconds,
        coverage_fraction: (merged.samples as f64 / expected_seconds as f64).min(1.0),
        score,
        grade: grade_for(score),
        dish,
        router,
        bars,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::UtcCalendar;
    use crate::latency::bin_index_for;

    fn stats(latencies: &[f64]) -> LatencySourceStats {
        let mut stats = LatencySourceStats::default();
        for ms in latencies {
            stats.bins[bin_index_for(*ms)] += 1;
            stats.count += 1;
            stats.sum_ms += ms;
            stats.sum_sq_ms += ms * ms;
        }
        stats
    }

    fn bucket(minute: i64, latencies: &[f64], drop_rate: f64) -> LatencyMinuteBucket {
        LatencyMinuteBucket {
            minute,
            samples: latencies.len() as u32,
            dish: stats(latencies),
            router: LatencySourceStats::default(),
            drop_sum: drop_rate * latencies.len() as f64,
            drop_count: latencies.len() as u32,
        }
    }

    fn metrics(p95: f64, p99: f64, jitter: f64, drop_pct: f64) -> LatencyStatMetrics {
        LatencyStatMetrics {
            count: 100,
            mean: Some(p95),
            p50: Some(p95),
            p95: Some(p95),
            p99: Some(p99),
            jitter: Some(jitter),
            spread: Some(p99 - p95),
            drop_pct: Some(drop_pct),
        }
    }

    // SPEC-DETAIL-006: interpolated percentiles, empty and overflow edges.
    #[test]
    fn percentiles_interpolate_within_bins() {
        assert_eq!(percentile(&[0; LATENCY_BIN_COUNT], 0.95), None);
        let uniform = stats(&(1..=100).map(f64::from).collect::<Vec<_>>());
        assert!((percentile(&uniform.bins, 0.5).unwrap() - 51.0).abs() < 1.0);
        assert!((percentile(&uniform.bins, 0.95).unwrap() - 96.0).abs() < 1.0);
        let mut overflow = vec![0; LATENCY_BIN_COUNT];
        overflow[LATENCY_BIN_COUNT - 1] = 100;
        assert_eq!(percentile(&overflow, 0.95), Some(LATENCY_OVERFLOW_EDGE_MS));
    }

    // SPEC-DETAIL-006: the weighted score and its grade bands.
    #[test]
    fn the_score_rewards_fast_steady_links() {
        let empty = LatencyStatMetrics {
            count: 0,
            mean: None,
            p50: None,
            p95: None,
            p99: None,
            jitter: None,
            spread: None,
            drop_pct: None,
        };
        assert_eq!(quality_score(&empty), 0);
        let good = quality_score(&metrics(35.0, 45.0, 4.0, 0.0));
        assert!(good >= 90);
        assert_eq!(grade_for(good), 'A');
        let bad = quality_score(&metrics(320.0, 480.0, 70.0, 6.0));
        assert!(bad < 40);
        assert_eq!(grade_for(bad), 'F');
        assert_eq!(
            [grade_for(75), grade_for(60), grade_for(40)],
            ['B', 'C', 'D']
        );
    }

    // SPEC-DETAIL-006: the range summary merges minutes, bars every slot, and
    // reads empty when nothing falls in range.
    #[test]
    fn ranges_merge_minutes_and_bar_every_slot() {
        let now_ms = 1_800_000_000_000_i64;
        let minute = (now_ms / 1_000 / 60) * 60 - 600;
        let buckets = [
            bucket(minute, &[20.0, 25.0, 30.0], 0.0),
            bucket(minute + 60, &[20.0, 25.0, 30.0], 0.1),
        ];
        let summary = summarize_latency(&buckets, UsageRange::OneHour, now_ms, &UtcCalendar);
        assert_eq!(summary.dish.count, 6);
        assert!(summary.dish.p50.is_some() && summary.dish.p95.is_some());
        assert!(summary.dish.jitter.is_some());
        assert!((summary.dish.drop_pct.unwrap() - 5.0).abs() < 1e-9);
        assert!(summary.score > 0);
        assert_eq!(summary.sampled_seconds, 6);
        assert_eq!(summary.bars.len(), 13, "5-minute bars over an hour");
        assert!(summary.bars.iter().any(|bar| bar.p95.is_some()));
        assert_eq!(summary.router, None);

        let empty = summarize_latency(&[], UsageRange::OneHour, now_ms, &UtcCalendar);
        assert_eq!(
            (empty.score, empty.dish.count, empty.dish.p95),
            (0, 0, None)
        );
    }
}
