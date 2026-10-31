//! Per-minute latency aggregates folded from per-second telemetry samples.
//!
//! The port of upstream `core/latencyBuckets.ts`. A minute keeps, per source (the dish's
//! pop-ping and the router's pop-ping), a 16-bin histogram plus the sums needed
//! for a mean and a standard deviation, so percentiles survive without keeping
//! every 1 Hz sample.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::telemetry::TelemetrySample;

/// 10 ms steps across the band latency normally occupies.
const TAIL_EDGES_MS: [f64; 5] = [150.0, 200.0, 300.0, 500.0, 1000.0];

/// Upper edge (exclusive) of each bin, in ms; the final bin is the >1 s overflow.
pub const LATENCY_BIN_UPPER_EDGES_MS: [f64; 15] = [
    10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 150.0, 200.0, 300.0, 500.0, 1000.0,
];

/// One histogram bin per edge, plus the final overflow bin.
pub const LATENCY_BIN_COUNT: usize = LATENCY_BIN_UPPER_EDGES_MS.len() + 1;

/// The finite top of the distribution, used when a percentile lands in overflow.
pub const LATENCY_OVERFLOW_EDGE_MS: f64 = TAIL_EDGES_MS[TAIL_EDGES_MS.len() - 1];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencySourceStats {
    /// Samples with a real reading this minute (finite, > 0).
    pub count: u32,
    pub sum_ms: f64,
    pub sum_sq_ms: f64,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Count per histogram bin, indexed by [`LATENCY_BIN_UPPER_EDGES_MS`].
    pub bins: Vec<u32>,
}

impl Default for LatencySourceStats {
    fn default() -> Self {
        Self {
            count: 0,
            sum_ms: 0.0,
            sum_sq_ms: 0.0,
            min_ms: None,
            max_ms: None,
            bins: vec![0; LATENCY_BIN_COUNT],
        }
    }
}

impl LatencySourceStats {
    fn add_reading(&mut self, latency_ms: f64) {
        self.bins[bin_index_for(latency_ms)] += 1;
        self.count += 1;
        self.sum_ms += latency_ms;
        self.sum_sq_ms += latency_ms * latency_ms;
        self.min_ms = Some(self.min_ms.map_or(latency_ms, |min| min.min(latency_ms)));
        self.max_ms = Some(self.max_ms.map_or(latency_ms, |max| max.max(latency_ms)));
    }

    fn add(&self, other: &Self) -> Self {
        let mut bins = self.bins.clone();
        for (index, value) in bins.iter_mut().enumerate() {
            *value += other.bins[index];
        }
        Self {
            count: self.count + other.count,
            sum_ms: self.sum_ms + other.sum_ms,
            sum_sq_ms: self.sum_sq_ms + other.sum_sq_ms,
            min_ms: match (self.min_ms, other.min_ms) {
                (None, other) => other,
                (this, None) => this,
                (Some(a), Some(b)) => Some(a.min(b)),
            },
            max_ms: match (self.max_ms, other.max_ms) {
                (None, other) => other,
                (this, None) => this,
                (Some(a), Some(b)) => Some(a.max(b)),
            },
            bins,
        }
    }
}

/// Index of the bin holding `latency_ms`.
pub fn bin_index_for(latency_ms: f64) -> usize {
    for (index, edge) in LATENCY_BIN_UPPER_EDGES_MS.iter().enumerate() {
        if latency_ms < *edge {
            return index;
        }
    }
    LATENCY_BIN_COUNT - 1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyMinuteBucket {
    /// Epoch seconds at the minute's start.
    pub minute: i64,
    pub samples: u32,
    pub dish: LatencySourceStats,
    pub router: LatencySourceStats,
    /// Sum of the dish's per-sample drop rate (0–1); divide by `drop_count` for %.
    pub drop_sum: f64,
    pub drop_count: u32,
}

impl LatencyMinuteBucket {
    fn empty(minute: i64) -> Self {
        Self {
            minute,
            samples: 0,
            dish: LatencySourceStats::default(),
            router: LatencySourceStats::default(),
            drop_sum: 0.0,
            drop_count: 0,
        }
    }
}

/// A sample's latency is a real measurement, not a sentinel.
fn is_reading(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

/// Group per-second samples into per-minute latency buckets.
pub fn fold_samples_to_latency_minutes(
    samples: &[TelemetrySample],
) -> BTreeMap<i64, LatencyMinuteBucket> {
    let mut buckets = BTreeMap::new();
    for sample in samples {
        let minute = sample.timestamp_ms.div_euclid(60_000) * 60;
        let bucket = buckets
            .entry(minute)
            .or_insert_with(|| LatencyMinuteBucket::empty(minute));
        // `samples` is seconds recorded, not seconds up: an outage second still
        // rode a poll and counts toward coverage, matching the energy fold.
        bucket.samples += 1;
        if let Some(latency) = is_reading(sample.latency_ms) {
            bucket.dish.add_reading(latency);
        }
        if let Some(latency) = is_reading(sample.router_latency_ms) {
            bucket.router.add_reading(latency);
        }
        if sample.drop_rate.is_finite() {
            bucket.drop_sum += sample.drop_rate;
            bucket.drop_count += 1;
        }
    }
    buckets
}

/// Replace, not add: the historian recomputes the in-progress minute from the
/// dish's ring each poll, so re-seeing it must overwrite, never accumulate.
pub fn replace_latency_minute_bucket(
    _base: Option<&LatencyMinuteBucket>,
    delta: &LatencyMinuteBucket,
) -> LatencyMinuteBucket {
    delta.clone()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyMonthBucket {
    /// Epoch seconds at the local start of the month.
    pub month: i64,
    pub samples: u32,
    pub dish: LatencySourceStats,
    pub router: LatencySourceStats,
    pub drop_sum: f64,
    pub drop_count: u32,
}

impl LatencyMonthBucket {
    fn empty(month: i64) -> Self {
        Self {
            month,
            samples: 0,
            dish: LatencySourceStats::default(),
            router: LatencySourceStats::default(),
            drop_sum: 0.0,
            drop_count: 0,
        }
    }

    fn add_minute(&self, from: &LatencyMinuteBucket) -> Self {
        Self {
            month: self.month,
            samples: self.samples + from.samples,
            dish: self.dish.add(&from.dish),
            router: self.router.add(&from.router),
            drop_sum: self.drop_sum + from.drop_sum,
            drop_count: self.drop_count + from.drop_count,
        }
    }
}

/// Merge a freshly folded month onto whatever its row already holds.
pub fn add_latency_month_bucket(
    base: Option<&LatencyMonthBucket>,
    delta: &LatencyMonthBucket,
) -> LatencyMonthBucket {
    match base {
        None => delta.clone(),
        Some(base) => {
            let mut merged = base.clone();
            merged.samples += delta.samples;
            merged.dish = base.dish.add(&delta.dish);
            merged.router = base.router.add(&delta.router);
            merged.drop_sum += delta.drop_sum;
            merged.drop_count += delta.drop_count;
            merged
        }
    }
}

/// Group expired minute buckets into per-month archive rows. `month_of` supplies
/// the local calendar boundary, keeping this module free of a clock.
pub fn fold_latency_minutes_to_months<F>(
    buckets: &[LatencyMinuteBucket],
    mut month_of: F,
) -> BTreeMap<i64, LatencyMonthBucket>
where
    F: FnMut(i64) -> i64,
{
    let mut months: BTreeMap<i64, LatencyMonthBucket> = BTreeMap::new();
    for bucket in buckets {
        let month = month_of(bucket.minute);
        let row = months
            .entry(month)
            .or_insert_with(|| LatencyMonthBucket::empty(month));
        *row = row.add_minute(bucket);
    }
    months
}
