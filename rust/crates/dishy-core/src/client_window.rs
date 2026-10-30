//! Per-device throughput over the last six hours, from the rates each roster
//! read already computes (spec 0175).
//!
//! Port of `collector/clientWindow.mts`, with the window widened to the 6 h the
//! Electron detail chart retains. Only the last 30 minutes persist, in the
//! TypeScript `client-samples.json` shape, so a restart keeps the chart.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const WINDOW_MS: i64 = 6 * 3_600_000;
pub const PERSIST_MS: i64 = 30 * 60_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSample {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub mac_address: String,
    pub at_ms: i64,
    pub down_mbps: f64,
    pub up_mbps: f64,
}

/// One device's reading from a roster poll.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientRate {
    pub key: String,
    pub mac_address: String,
    pub down_mbps: f64,
    pub up_mbps: f64,
}

#[derive(Debug, Default)]
pub struct ClientWindow {
    series: HashMap<String, Vec<ClientSample>>,
}

/// Three decimals is about 1 kbps, finer than any chart can show.
fn round(value: f64) -> f64 {
    if value.is_finite() {
        (value * 1_000.0).round() / 1_000.0
    } else {
        0.0
    }
}

impl ClientWindow {
    /// Restore what a previous run persisted; an unreadable file is empty.
    pub fn restore(path: &Path, now_ms: i64) -> Self {
        let mut window = Self::default();
        let Some(samples) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<ClientSample>>(&text).ok())
        else {
            return window;
        };
        let cutoff = now_ms - WINDOW_MS;
        for sample in samples.into_iter().filter(|sample| sample.at_ms >= cutoff) {
            let key = sample
                .key
                .clone()
                .unwrap_or_else(|| sample.mac_address.clone());
            window.series.entry(key).or_default().push(sample);
        }
        for series in window.series.values_mut() {
            series.sort_by_key(|sample| sample.at_ms);
        }
        window
    }

    /// Record one poll's readings and drop whatever aged out.
    pub fn ingest(&mut self, readings: &[ClientRate], now_ms: i64) {
        let cutoff = now_ms - WINDOW_MS;
        for reading in readings {
            self.series
                .entry(reading.key.clone())
                .or_default()
                .push(ClientSample {
                    key: Some(reading.key.clone()),
                    mac_address: reading.mac_address.clone(),
                    at_ms: now_ms,
                    down_mbps: round(reading.down_mbps),
                    up_mbps: round(reading.up_mbps),
                });
        }
        for series in self.series.values_mut() {
            series.retain(|sample| sample.at_ms >= cutoff);
        }
        self.series.retain(|_, series| !series.is_empty());
    }

    /// One device's samples after `since_ms`, oldest first.
    pub fn samples(&self, key: &str, since_ms: i64) -> Vec<ClientSample> {
        self.series
            .get(key)
            .map(|series| {
                series
                    .iter()
                    .filter(|sample| sample.at_ms > since_ms)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Persist the last 30 minutes atomically.
    pub fn snapshot(&self, path: &Path, now_ms: i64) -> std::io::Result<()> {
        let cutoff = now_ms - PERSIST_MS;
        let mut recent: Vec<&ClientSample> = self
            .series
            .values()
            .flatten()
            .filter(|sample| sample.at_ms > cutoff)
            .collect();
        if recent.is_empty() {
            return Ok(());
        }
        recent.sort_by_key(|sample| sample.at_ms);
        let temporary = path.with_extension("json.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&recent).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn rate(key: &str, down: f64) -> ClientRate {
        ClientRate {
            key: key.into(),
            mac_address: format!("aa:bb:cc:00:00:{key}"),
            down_mbps: down,
            up_mbps: 1.0,
        }
    }

    // SPEC-NETDETAIL-001: samples round to three decimals, expire after six
    // hours, and a device with none left is dropped.
    #[test]
    fn the_window_rounds_and_expires() {
        let mut window = ClientWindow::default();
        window.ingest(
            &[rate("1", 12.345_678), rate("2", 3.0)],
            NOW - WINDOW_MS - 1_000,
        );
        window.ingest(&[rate("1", 5.0)], NOW);
        let one = window.samples("1", 0);
        assert_eq!(one.len(), 1, "the old sample aged out");
        assert_eq!(one[0].down_mbps, 5.0);
        assert!(
            window.samples("2", 0).is_empty(),
            "device 2 has nothing left"
        );

        let mut fresh = ClientWindow::default();
        fresh.ingest(&[rate("1", 12.345_678)], NOW);
        assert_eq!(fresh.samples("1", 0)[0].down_mbps, 12.346);
        assert!(fresh.samples("1", NOW).is_empty(), "since is exclusive");
    }

    // SPEC-NETDETAIL-001: the last 30 minutes persist in the TypeScript shape
    // and come back on restore.
    #[test]
    fn the_last_half_hour_persists() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("client-samples.json");
        let mut window = ClientWindow::default();
        window.ingest(&[rate("1", 1.0)], NOW - 3_600_000);
        window.ingest(&[rate("1", 2.0)], NOW - 60_000);
        window.snapshot(&path, NOW).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["macAddress"], "aa:bb:cc:00:00:1");
        assert_eq!(json[0]["downMbps"], 2.0);
        let restored = ClientWindow::restore(&path, NOW);
        assert_eq!(restored.samples("1", 0).len(), 1);
        std::fs::write(&path, "not json").unwrap();
        assert!(ClientWindow::restore(&path, NOW).samples("1", 0).is_empty());
    }
}
