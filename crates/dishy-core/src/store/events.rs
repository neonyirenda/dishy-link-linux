//! Durable log of the dish's outage/event entries.
//!
//! The dish's list rolls and clears on reboot, so an outage from last week is
//! gone whether or not anyone saw it. Events are few, and an in-progress outage
//! restates itself with a longer duration across polls, so this keeps the whole
//! log in memory and rewrites the file on change rather than appending.

use std::path::{Path, PathBuf};

use crate::jsonl::{
    JsonlResult, ensure_parent_directory, read_json_lines, write_json_lines_atomically,
};
use crate::telemetry::{OutageEvent, canonical_cause};

/// The event log is a "what happened recently" panel, not an archive. Two days
/// matches the thermal log beside it.
pub const EVENT_RETENTION_MS: i64 = 48 * 3_600_000;

#[derive(Debug)]
pub struct EventStore {
    file_path: PathBuf,
    events: Vec<OutageEvent>,
}

impl EventStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        // The log is keyed by outage, the same way the TypeScript Map is. A file
        // written by an older build may hold two rows for one key; the later row
        // wins without moving the first row's position.
        let mut events: Vec<OutageEvent> = Vec::new();
        for event in read_json_lines::<OutageEvent>(&file_path)? {
            let key = key_of(&event);
            if let Some(existing) = events.iter_mut().find(|existing| key_of(existing) == key) {
                *existing = event;
            } else {
                events.push(event);
            }
        }
        Ok(Self { file_path, events })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Fold in what a device currently reports. Returns how many entries were
    /// new or changed, so a quiet poll costs no disk write.
    pub fn upsert(
        &mut self,
        incoming: impl IntoIterator<Item = OutageEvent>,
        now_ms: i64,
    ) -> JsonlResult<usize> {
        let mut changed = 0;
        for event in incoming {
            let key = key_of(&event);
            if let Some(existing) = self
                .events
                .iter_mut()
                .find(|existing| key_of(existing) == key)
            {
                // A known outage whose duration has grown is the same outage,
                // still running. An entry missing its band-switch details takes
                // them from a later sighting.
                let gains_details = existing.band_switch.is_none() && event.band_switch.is_some();
                if existing.duration_ms >= event.duration_ms && !gains_details {
                    continue;
                }
                *existing = event;
            } else {
                self.events.push(event);
            }
            changed += 1;
        }
        if changed > 0 {
            self.flush(now_ms)?;
        }
        Ok(changed)
    }

    /// Events newest-first, for the API. The cutoff is applied here as well as
    /// in flush, which only runs when an event arrives: a quiet stretch or a
    /// restart would otherwise serve rows past the window.
    pub fn all(&self, now_ms: i64) -> Vec<OutageEvent> {
        let cutoff = now_ms - EVENT_RETENTION_MS;
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|event| event.start_ms >= cutoff)
            .cloned()
            .collect();
        events.sort_by_key(|item| std::cmp::Reverse(item.start_ms));
        events
    }

    fn flush(&mut self, now_ms: i64) -> JsonlResult<()> {
        let cutoff = now_ms - EVENT_RETENTION_MS;
        self.events.retain(|event| event.start_ms >= cutoff);
        // Oldest first on disk, so the log reads chronologically.
        self.events.sort_by_key(|event| event.start_ms);
        write_json_lines_atomically(&self.file_path, &self.events)
    }
}

/// The same outage seen again across polls. Keyed on the canonical cause token,
/// not the raw spelling, so a row saved by an older build still folds in.
fn key_of(event: &OutageEvent) -> (i64, String) {
    (event.start_ms, canonical_cause(&event.cause))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::telemetry::{BandSwitch, Severity};

    // 2026-06-15T12:00:00Z
    const NOW_MS: i64 = 1_781_524_800_000;

    fn event(start_ms: i64, duration_ms: i64) -> OutageEvent {
        OutageEvent {
            start_ms,
            duration_ms,
            cause: "EVENT_REASON_OUTAGE_NO_PINGS".into(),
            severity: Severity::Warning,
            band_switch: None,
        }
    }

    #[test]
    fn a_growing_outage_updates_in_place_instead_of_duplicating() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("events.ndjson");
        let mut store = EventStore::open(&path).expect("open");

        let start = NOW_MS - 10_000;
        assert_eq!(
            store.upsert([event(start, 1_000)], NOW_MS).expect("first"),
            1
        );
        assert_eq!(
            store.upsert([event(start, 9_000)], NOW_MS).expect("grow"),
            1
        );
        assert_eq!(
            store.upsert([event(start, 5_000)], NOW_MS).expect("shrink"),
            0
        );

        let events = store.all(NOW_MS);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].duration_ms, 9_000);
        assert_eq!(
            read_json_lines::<OutageEvent>(&path).expect("read").len(),
            1
        );
    }

    #[test]
    fn a_later_sighting_can_add_band_switch_details() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("events.ndjson");
        let mut store = EventStore::open(&path).expect("open");
        let start = NOW_MS - 10_000;
        store.upsert([event(start, 5_000)], NOW_MS).expect("first");

        let mut detailed = event(start, 5_000);
        detailed.band_switch = Some(BandSwitch {
            client_id: 7,
            from_band: "RF_2GHZ".into(),
            to_band: "RF_5GHZ".into(),
        });
        assert_eq!(store.upsert([detailed.clone()], NOW_MS).expect("detail"), 1);
        assert_eq!(store.all(NOW_MS)[0].band_switch, detailed.band_switch);
    }

    #[test]
    fn the_retention_window_holds_on_read_and_write() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("events.ndjson");
        let old = event(NOW_MS - EVENT_RETENTION_MS - 1, 1_000);
        write_json_lines_atomically(&path, std::slice::from_ref(&old)).expect("seed");
        let mut store = EventStore::open(&path).expect("open");
        assert!(store.all(NOW_MS).is_empty());

        let fresh = event(NOW_MS - 1_000, 1_000);
        store
            .upsert([event(NOW_MS - 10_000, 1_000)], NOW_MS)
            .expect("upsert");
        let kept = read_json_lines::<OutageEvent>(&path).expect("read");
        assert!(
            kept.iter()
                .all(|event| event.start_ms >= NOW_MS - EVENT_RETENTION_MS)
        );
        assert!(
            kept.iter()
                .any(|event| event.start_ms == fresh.start_ms - 9_000)
        );
    }

    #[test]
    fn duplicate_keys_in_a_legacy_file_collapse_to_the_latest_row() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("events.ndjson");
        let start = NOW_MS - 1_000;
        let mut first = event(start, 1_000);
        let mut second = event(start, 2_000);
        first.band_switch = None;
        second.band_switch = Some(BandSwitch {
            client_id: 3,
            from_band: "RF_5GHZ".into(),
            to_band: "RF_2GHZ".into(),
        });
        write_json_lines_atomically(&path, &[first, second.clone()]).expect("seed");
        let store = EventStore::open(&path).expect("open");
        let events = store.all(NOW_MS);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], second);
    }

    #[test]
    fn events_read_back_oldest_first() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("events.ndjson");
        let mut store = EventStore::open(&path).expect("open");
        store
            .upsert(
                [
                    event(NOW_MS - 1_000, 1_000),
                    event(NOW_MS - 5_000, 1_000),
                    event(NOW_MS - 3_000, 1_000),
                ],
                NOW_MS,
            )
            .expect("upsert");
        let starts: Vec<_> = read_json_lines::<OutageEvent>(&path)
            .expect("read")
            .into_iter()
            .map(|event| event.start_ms)
            .collect();
        assert_eq!(starts, vec![NOW_MS - 5_000, NOW_MS - 3_000, NOW_MS - 1_000]);
    }
}
