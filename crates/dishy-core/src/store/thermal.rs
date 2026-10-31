//! Durable log of dish thermal episodes.
//!
//! The dish reports thermal state as live booleans and keeps no history: once a
//! flag clears, the episode is gone. The recorder watches false→true edges and
//! writes them here. Open episodes survive retention regardless of age, because
//! an unresolved throttle is current state, not history.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::jsonl::{
    JsonlResult, ensure_parent_directory, read_json_lines, write_json_lines_atomically,
};

/// Matches the event store: both feed the same "Events & outages" panel.
pub const THERMAL_RETENTION_MS: i64 = 48 * 3_600_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThermalEpisode {
    /// `alerts` key that produced this, e.g. `thermalThrottle`.
    pub alert_key: String,
    pub start_ms: i64,
    /// `None` while the flag is still set.
    pub end_ms: Option<i64>,
}

#[derive(Debug)]
pub struct ThermalStore {
    file_path: PathBuf,
    episodes: Vec<ThermalEpisode>,
}

impl ThermalStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        let episodes = read_json_lines(&file_path)?;
        Ok(Self {
            file_path,
            episodes,
        })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Episodes newest-first, for the API. The cutoff is applied here as well as
    /// in flush, which only runs on an edge: a quiet stretch or a restart would
    /// otherwise serve episodes past the window.
    pub fn all(&self, now_ms: i64) -> Vec<ThermalEpisode> {
        let cutoff = now_ms - THERMAL_RETENTION_MS;
        let mut episodes: Vec<_> = self
            .episodes
            .iter()
            .filter(|episode| episode.end_ms.is_none() || episode.start_ms >= cutoff)
            .cloned()
            .collect();
        episodes.sort_by_key(|item| std::cmp::Reverse(item.start_ms));
        episodes
    }

    pub fn is_open(&self, alert_key: &str) -> bool {
        self.episodes
            .iter()
            .any(|episode| episode.alert_key == alert_key && episode.end_ms.is_none())
    }

    pub fn open_episode(&mut self, alert_key: &str, start_ms: i64, now_ms: i64) -> JsonlResult<()> {
        if self.is_open(alert_key) {
            return Ok(());
        }
        self.episodes.push(ThermalEpisode {
            alert_key: alert_key.to_string(),
            start_ms,
            end_ms: None,
        });
        self.flush(now_ms)
    }

    pub fn close_episode(&mut self, alert_key: &str, end_ms: i64, now_ms: i64) -> JsonlResult<()> {
        let mut closed = false;
        for episode in self
            .episodes
            .iter_mut()
            .filter(|episode| episode.alert_key == alert_key && episode.end_ms.is_none())
        {
            episode.end_ms = Some(end_ms);
            closed = true;
        }
        if !closed {
            return Ok(());
        }
        self.flush(now_ms)
    }

    fn flush(&mut self, now_ms: i64) -> JsonlResult<()> {
        let cutoff = now_ms - THERMAL_RETENTION_MS;
        self.episodes
            .retain(|episode| episode.end_ms.is_none() || episode.start_ms >= cutoff);
        write_json_lines_atomically(&self.file_path, &self.episodes)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    const NOW_MS: i64 = 1_781_524_800_000;

    #[test]
    fn an_episode_opens_once_and_closes_once() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("thermal.ndjson");
        let mut store = ThermalStore::open(&path).expect("open");
        let start = NOW_MS - 10_000;

        store
            .open_episode("thermalThrottle", start, NOW_MS)
            .expect("open");
        store
            .open_episode("thermalThrottle", start + 1_000, NOW_MS)
            .expect("duplicate open");
        assert_eq!(store.all(NOW_MS).len(), 1);
        assert!(store.is_open("thermalThrottle"));

        store
            .close_episode("thermalThrottle", NOW_MS, NOW_MS)
            .expect("close");
        store
            .close_episode("thermalThrottle", NOW_MS + 1_000, NOW_MS)
            .expect("duplicate close");
        let episodes = store.all(NOW_MS);
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].end_ms, Some(NOW_MS));
        assert!(!store.is_open("thermalThrottle"));
    }

    #[test]
    fn an_open_episode_is_kept_past_retention() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("thermal.ndjson");
        let old = ThermalEpisode {
            alert_key: "thermalShutdown".into(),
            start_ms: NOW_MS - THERMAL_RETENTION_MS - 1,
            end_ms: None,
        };
        let closed = ThermalEpisode {
            alert_key: "thermalThrottle".into(),
            start_ms: NOW_MS - THERMAL_RETENTION_MS - 1,
            end_ms: Some(NOW_MS - THERMAL_RETENTION_MS),
        };
        write_json_lines_atomically(&path, &[old.clone(), closed]).expect("seed");

        let store = ThermalStore::open(&path).expect("open");
        assert_eq!(store.all(NOW_MS), vec![old]);
    }
}
