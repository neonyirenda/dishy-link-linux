//! Reading selections shared by the dashboard tiles and sparklines.
//!
//! The port of upstream `src/lib/readings.ts`. Every span is cut from wall-clock `now`
//! rather than from a count of trailing samples: a dish that stops answering
//! stops appending, and a fixed count would keep presenting the last healthy
//! minute as the current one.

use dishy_core::telemetry::TelemetrySample;

/// Averages and "is this recent?" are measured over the last minute of clock.
pub const RECENT_AVERAGE_MS: i64 = 60_000;
/// Each stat tile's trace covers the last 90 seconds.
pub const SPARKLINE_MS: i64 = 90_000;
/// The dashboard stops calling a feed live after this long without a sample.
pub const STALE_AFTER_MS: i64 = 6_000;
/// The current power figure settles into completed five-second buckets.
pub const POWER_BUCKET_MS: i64 = 5_000;
/// How far back the power figure reaches for a completed bucket with a reading.
const POWER_BUCKET_FALLBACK_MS: i64 = 60_000;
/// The latency-quality caption is always over the last hour.
const LATENCY_QUALITY_MS: i64 = 3_600_000;

/// Whether the last minute holds any reading at all.
///
/// Ping success is inverted from the drop rate, so an empty minute averages to
/// zero drops and renders as 100% answered — the most reassuring number on the
/// dashboard, shown at the moment the dish is unreachable.
pub fn has_recent_readings(samples: &[TelemetrySample], now_ms: i64) -> bool {
    samples
        .last()
        .is_some_and(|sample| sample.timestamp_ms >= now_ms - RECENT_AVERAGE_MS)
}

/// Mean over the last minute of clock. An empty span is zero: no traffic moved
/// and no ping came back.
pub fn recent_average(
    samples: &[TelemetrySample],
    get_value: impl Fn(&TelemetrySample) -> Option<f64>,
    now_ms: i64,
) -> f64 {
    let floor_ms = now_ms - RECENT_AVERAGE_MS;
    let mut sum = 0.0;
    let mut count = 0_usize;
    for sample in samples.iter().rev() {
        if sample.timestamp_ms < floor_ms {
            break;
        }
        if let Some(value) = get_value(sample).filter(|value| value.is_finite()) {
            sum += value;
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// The end of the latest completed five-second bucket.
pub fn power_bucket_end_ms(now_ms: i64, bucket_ms: i64) -> i64 {
    now_ms.div_euclid(bucket_ms) * bucket_ms
}

fn power_mean_in_window(samples: &[TelemetrySample], start_ms: i64, end_ms: i64) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0_usize;
    for sample in samples.iter().rev() {
        if sample.timestamp_ms < start_ms {
            break;
        }
        if sample.timestamp_ms >= end_ms {
            continue;
        }
        // A dropped ring entry decodes as zero; skip non-positive draws rather
        // than letting one drag the mean down.
        if sample.power_w.is_finite() && sample.power_w > 0.0 {
            sum += sample.power_w;
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

/// The settled current draw: the mean of the last completed five-second bucket,
/// reaching back up to a minute when that bucket holds nothing.
pub fn power_bucket_mean(samples: &[TelemetrySample], now_ms: i64) -> f64 {
    let latest_end = power_bucket_end_ms(now_ms, POWER_BUCKET_MS);
    let oldest_end = latest_end - POWER_BUCKET_FALLBACK_MS;
    let mut bucket_end = latest_end;
    while bucket_end > oldest_end {
        if let Some(mean) = power_mean_in_window(samples, bucket_end - POWER_BUCKET_MS, bucket_end)
        {
            return mean;
        }
        bucket_end -= POWER_BUCKET_MS;
    }
    0.0
}

/// The last 90 seconds of a series, for the sparkline on a tile.
///
/// `window_end_ms` caps the newest edge; the default leaves it open so the trace
/// runs to the freshest sample. `None` values become gaps rather than zeroes.
pub fn sparkline_from(
    samples: &[TelemetrySample],
    get_value: impl Fn(&TelemetrySample) -> Option<f64>,
    now_ms: i64,
    window_end_ms: i64,
) -> Vec<Option<f64>> {
    let floor_ms = now_ms - SPARKLINE_MS;
    samples
        .iter()
        .filter(|sample| sample.timestamp_ms >= floor_ms && sample.timestamp_ms < window_end_ms)
        .map(|sample| get_value(sample).filter(|value| value.is_finite()))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyQuality {
    pub score: u32,
    pub grade: &'static str,
    pub p95_ms: f64,
}

fn band_score(value: f64, good: f64, bad: f64) -> f64 {
    if value <= good {
        1.0
    } else if value >= bad {
        0.0
    } else {
        (bad - value) / (bad - good)
    }
}

/// Quality score over the last hour, using the same weights and bands as
/// upstream `core/latencySummary.ts`: latency tail, jitter, and packet loss.
pub fn latency_quality(samples: &[TelemetrySample], now_ms: i64) -> Option<LatencyQuality> {
    let floor_ms = now_ms - LATENCY_QUALITY_MS;
    let mut latencies = Vec::new();
    let mut drop_sum = 0.0;
    let mut drop_count = 0_usize;
    for sample in samples {
        if sample.timestamp_ms < floor_ms {
            continue;
        }
        if let Some(value) = sample
            .latency_ms
            .filter(|value| value.is_finite() && *value > 0.0)
        {
            latencies.push(value);
        }
        if sample.drop_rate.is_finite() {
            drop_sum += sample.drop_rate;
            drop_count += 1;
        }
    }
    if latencies.is_empty() {
        return None;
    }

    latencies.sort_by(|a, b| a.total_cmp(b));
    let percentile = |p: f64| -> f64 {
        let index = ((latencies.len() - 1) as f64 * p).round() as usize;
        latencies[index]
    };
    let p95 = percentile(0.95);
    let p99 = percentile(0.99);
    let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let variance = latencies
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / latencies.len() as f64;
    let jitter = variance.sqrt();
    let drop_pct = if drop_count == 0 {
        0.0
    } else {
        drop_sum / drop_count as f64 * 100.0
    };

    let latency = band_score(p95, 20.0, 250.0);
    let tail = band_score(p99, 50.0, 400.0);
    let jitter = band_score(jitter, 5.0, 60.0);
    let loss = band_score(drop_pct, 0.0, 5.0);
    let score = (100.0 * (0.4 * latency + 0.3 * jitter + 0.2 * tail + 0.1 * loss))
        .round()
        .clamp(0.0, 100.0) as u32;
    Some(LatencyQuality {
        score,
        grade: grade_for(score),
        p95_ms: p95,
    })
}

fn grade_for(score: u32) -> &'static str {
    match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_781_524_800_000;

    fn sample(
        at_ms: i64,
        latency_ms: Option<f64>,
        drop_rate: f64,
        power_w: f64,
    ) -> TelemetrySample {
        TelemetrySample {
            timestamp_ms: at_ms,
            latency_ms,
            drop_rate,
            downlink_bps: 0.0,
            uplink_bps: 0.0,
            power_w,
            router_latency_ms: None,
            router_ping_success_percent: None,
        }
    }

    // SPEC-DASH-005: a minute with no readings is empty, not a perfect score.
    #[test]
    fn recent_helpers_ignore_an_old_minute() {
        let samples = [
            sample(NOW_MS - 120_000, Some(25.0), 0.0, 50.0),
            sample(NOW_MS - 10_000, Some(30.0), 0.1, 60.0),
            sample(NOW_MS - 2_000, Some(20.0), 0.0, 70.0),
        ];
        assert!(has_recent_readings(&samples, NOW_MS));
        assert_eq!(
            recent_average(&samples, |sample| sample.latency_ms, NOW_MS),
            25.0
        );
        assert_eq!(
            recent_average(
                &samples,
                |sample| Some((1.0 - sample.drop_rate) * 100.0),
                NOW_MS
            ),
            95.0
        );

        let stale = [sample(NOW_MS - 120_000, Some(25.0), 0.0, 50.0)];
        assert!(!has_recent_readings(&stale, NOW_MS));
        assert_eq!(
            recent_average(&stale, |sample| sample.latency_ms, NOW_MS),
            0.0
        );
    }

    // SPEC-DASH-001: the power tile settles on the last completed five-second
    // bucket, and skips the still-open bucket.
    #[test]
    fn power_settles_on_completed_buckets() {
        let now_ms = NOW_MS + 3_000;
        let samples = [
            sample(now_ms - 7_000, None, 0.0, 40.0),
            sample(now_ms - 5_000, None, 0.0, 60.0),
            sample(now_ms - 1_000, None, 0.0, 900.0),
        ];
        assert_eq!(power_bucket_end_ms(now_ms, POWER_BUCKET_MS), NOW_MS);
        assert_eq!(power_bucket_mean(&samples, now_ms), 50.0);

        // Nothing in the latest completed bucket: reach back to the previous.
        let earlier = [
            sample(now_ms - 9_000, None, 0.0, 10.0),
            sample(now_ms - 5_000, None, 0.0, 0.0),
        ];
        assert_eq!(power_bucket_mean(&earlier, now_ms), 10.0);
    }

    // SPEC-DASH-002: a sparkline is the last ninety seconds, capped at a bucket
    // boundary when the caller names one.
    #[test]
    fn sparkline_is_the_last_ninety_seconds() {
        let samples = [
            sample(NOW_MS - 100_000, Some(1.0), 0.0, 1.0),
            sample(NOW_MS - 80_000, Some(2.0), 0.0, 2.0),
            sample(NOW_MS - 5_000, Some(3.0), 0.0, 3.0),
            sample(NOW_MS, Some(4.0), 0.0, 4.0),
        ];
        assert_eq!(
            sparkline_from(&samples, |sample| sample.latency_ms, NOW_MS, NOW_MS),
            vec![Some(2.0), Some(3.0)]
        );
    }

    // SPEC-DASH-001: latency quality is the same weighted score as the renderer.
    #[test]
    fn latency_quality_scores_the_last_hour() {
        let samples: Vec<_> = (0..100)
            .map(|index| sample(NOW_MS - index * 1_000, Some(20.0), 0.0, 50.0))
            .collect();
        let quality = latency_quality(&samples, NOW_MS).expect("quality");
        assert_eq!(quality.score, 100);
        assert_eq!(quality.grade, "A");
        assert_eq!(quality.p95_ms, 20.0);

        assert_eq!(
            latency_quality(
                &[sample(NOW_MS - 2 * 3_600_000, Some(20.0), 0.0, 50.0)],
                NOW_MS
            ),
            None
        );
    }
}
