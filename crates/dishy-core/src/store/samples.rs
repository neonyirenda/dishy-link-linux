//! The rolling full-resolution telemetry snapshot.
//!
//! `samples.json` is a JSON array, not a JSON Lines log: it is rewritten whole
//! every minute. Its mtime is also a heartbeat — a boot that finds it stale
//! knows the recorder was off in between.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::jsonl::{JsonlResult, write_json_atomically};
use crate::telemetry::TelemetrySample;

#[derive(Debug)]
pub struct SamplesStore {
    file_path: PathBuf,
}

impl SamplesStore {
    pub fn new(file_path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: file_path.into(),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Snapshot rows newer than `now_ms - retention_ms`. A missing file is an
    /// empty snapshot; an unreadable one is an error the caller may choose to
    /// treat as "start fresh".
    pub fn read(&self, now_ms: i64, retention_ms: i64) -> JsonlResult<Vec<TelemetrySample>> {
        let text = match fs::read_to_string(&self.file_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let cutoff = now_ms - retention_ms;
        let samples: Vec<TelemetrySample> = serde_json::from_str(&text)?;
        Ok(samples
            .into_iter()
            .filter(|sample| sample.timestamp_ms >= cutoff)
            .collect())
    }

    pub fn write(&self, samples: &[TelemetrySample]) -> JsonlResult<()> {
        let compacted: Vec<_> = samples.iter().map(compact_sample).collect();
        write_json_atomically(&self.file_path, &compacted)
    }
}

/// Round for the snapshot/API payload — chart precision, not lab precision.
pub fn compact_sample(sample: &TelemetrySample) -> TelemetrySample {
    fn round(value: f64, factor: f64) -> f64 {
        (value * factor).round() / factor
    }

    TelemetrySample {
        timestamp_ms: sample.timestamp_ms,
        latency_ms: sample.latency_ms.map(|value| round(value, 10.0)),
        drop_rate: round(sample.drop_rate, 1_000.0),
        downlink_bps: round(sample.downlink_bps, 1.0),
        uplink_bps: round(sample.uplink_bps, 1.0),
        power_w: round(sample.power_w, 10.0),
        // Finite-checked rather than null-checked: a snapshot written before a
        // field existed restores it as absent, which must not become NaN.
        router_latency_ms: sample
            .router_latency_ms
            .filter(|value| value.is_finite())
            .map(|value| round(value, 10.0)),
        router_ping_success_percent: sample
            .router_ping_success_percent
            .filter(|value| value.is_finite())
            .map(|value| round(value, 100.0)),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    const NOW_MS: i64 = 1_781_524_800_000;
    const RETENTION_MS: i64 = 6 * 3_600_000;

    #[test]
    fn a_snapshot_round_trips_and_drops_stale_samples() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("samples.json");
        let store = SamplesStore::new(&path);

        let sample = TelemetrySample {
            timestamp_ms: NOW_MS,
            latency_ms: Some(12.345),
            drop_rate: 0.123_45,
            downlink_bps: 1_000.6,
            uplink_bps: 500.4,
            power_w: 45.67,
            router_latency_ms: Some(23.456),
            router_ping_success_percent: Some(99.876),
        };
        let stale = TelemetrySample {
            timestamp_ms: NOW_MS - RETENTION_MS - 1,
            ..sample.clone()
        };
        store.write(&[stale, sample.clone()]).expect("write");

        let read = store.read(NOW_MS, RETENTION_MS).expect("read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].latency_ms, Some(12.3));
        assert_eq!(read[0].drop_rate, 0.123);
        assert_eq!(read[0].downlink_bps, 1_001.0);
        assert_eq!(read[0].power_w, 45.7);
        assert_eq!(read[0].router_latency_ms, Some(23.5));
        assert_eq!(read[0].router_ping_success_percent, Some(99.88));
    }

    #[test]
    fn a_missing_snapshot_is_empty() {
        let directory = tempdir().expect("temp dir");
        let store = SamplesStore::new(directory.path().join("absent.json"));
        assert!(store.read(NOW_MS, RETENTION_MS).expect("read").is_empty());
    }
}
