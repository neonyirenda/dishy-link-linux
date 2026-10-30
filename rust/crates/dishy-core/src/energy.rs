//! Per-minute energy and traffic buckets folded from per-second samples.
//!
//! The port of `core/energyBuckets.ts`. Both recorders fold a poll's samples the
//! same way, so a minute means the same thing on every host.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::telemetry::TelemetrySample;

/// What fraction of a reporting window actually has samples. Gaps are never
/// filled with last-known values, so this is the honesty check a total needs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub sampled_seconds: i64,
    pub expected_seconds: i64,
    pub fraction: f64,
}

/// Coverage is `sampled / expected`, capped at 1. An empty window has one
/// expected second, so a fresh store reports 0 rather than dividing by zero.
pub fn coverage(sampled_seconds: i64, expected_seconds: i64) -> Coverage {
    let sampled_seconds = sampled_seconds.max(0);
    let expected_seconds = expected_seconds.max(1);
    Coverage {
        sampled_seconds,
        expected_seconds,
        fraction: (sampled_seconds as f64 / expected_seconds as f64).min(1.0),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinuteBucket {
    pub minute: i64,
    pub watt_seconds: f64,
    pub samples: u32,
    /// Downlink/uplink volume in bits (absent on rows written before data-usage
    /// tracking). Always written by the fold.
    #[serde(default)]
    pub downlink_bits: f64,
    #[serde(default)]
    pub uplink_bits: f64,
}

/// Sum a minute's running total with a freshly folded delta for the same minute.
///
/// A minute can fill across several drains; the cursor guarantees each sample is
/// drained once, so adding is exact.
pub fn add_minute_bucket(base: Option<&MinuteBucket>, delta: &MinuteBucket) -> MinuteBucket {
    match base {
        None => delta.clone(),
        Some(base) => MinuteBucket {
            minute: delta.minute,
            watt_seconds: base.watt_seconds + delta.watt_seconds,
            samples: base.samples + delta.samples,
            downlink_bits: base.downlink_bits + delta.downlink_bits,
            uplink_bits: base.uplink_bits + delta.uplink_bits,
        },
    }
}

/// Group per-second samples into per-minute energy and traffic buckets.
pub fn fold_samples_to_minutes(samples: &[TelemetrySample]) -> BTreeMap<i64, MinuteBucket> {
    let mut buckets = BTreeMap::new();
    for sample in samples {
        let minute = sample.timestamp_ms.div_euclid(60_000) * 60;
        let bucket = buckets.entry(minute).or_insert_with(|| MinuteBucket {
            minute,
            ..MinuteBucket::default()
        });
        bucket.watt_seconds += sample.power_w;
        // A per-second sample: bps over one second ≈ bits transferred.
        bucket.downlink_bits += sample.downlink_bps;
        bucket.uplink_bits += sample.uplink_bps;
        bucket.samples += 1;
    }
    buckets
}

/// One archived calendar month, folded from that month's minutes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthBucket {
    /// Epoch seconds at the local start of the month.
    pub month: i64,
    pub watt_seconds: f64,
    /// Minutes actually recorded that month — the honesty check on a summary.
    pub samples: u32,
    pub downlink_bits: f64,
    pub uplink_bits: f64,
}

/// Sum a month's running total with a freshly folded delta for the same month.
pub fn add_month_bucket(base: Option<&MonthBucket>, delta: &MonthBucket) -> MonthBucket {
    match base {
        None => delta.clone(),
        Some(base) => MonthBucket {
            month: delta.month,
            watt_seconds: base.watt_seconds + delta.watt_seconds,
            samples: base.samples + delta.samples,
            downlink_bits: base.downlink_bits + delta.downlink_bits,
            uplink_bits: base.uplink_bits + delta.uplink_bits,
        },
    }
}

/// Group expired minute buckets into per-month archive rows. `month_of` supplies
/// the local calendar boundary, keeping this module free of a clock.
pub fn fold_minutes_to_months<F>(
    buckets: &[MinuteBucket],
    mut month_of: F,
) -> BTreeMap<i64, MonthBucket>
where
    F: FnMut(i64) -> i64,
{
    let mut months: BTreeMap<i64, MonthBucket> = BTreeMap::new();
    for bucket in buckets {
        let month = month_of(bucket.minute);
        let row = months.entry(month).or_insert_with(|| MonthBucket {
            month,
            ..MonthBucket::default()
        });
        row.watt_seconds += bucket.watt_seconds;
        row.samples += bucket.samples;
        row.downlink_bits += bucket.downlink_bits;
        row.uplink_bits += bucket.uplink_bits;
    }
    months
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-RECORDER-006: coverage is sampled / expected, never a fabricated fill.
    #[test]
    fn coverage_is_capped_and_never_divides_by_zero() {
        assert_eq!(
            coverage(1_800, 3_600),
            Coverage {
                sampled_seconds: 1_800,
                expected_seconds: 3_600,
                fraction: 0.5,
            }
        );
        assert_eq!(coverage(4_000, 3_600).fraction, 1.0);
        assert_eq!(coverage(0, 0).fraction, 0.0);
        assert_eq!(coverage(-10, 3_600).sampled_seconds, 0);
    }
}
