//! Durable hourly snapshots of the dish's obstruction map.
//!
//! Recorded by the service, not a browser, so a day with the app closed leaves
//! no hole and every client reads one history. Cells are packed 2 bits per cell
//! as base64, the shape the TypeScript client's `unpackCells` reads.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::jsonl::{
    JsonlResult, ensure_parent_directory, read_json_lines_with, write_json_lines_atomically,
};

/// One snapshot an hour: obstruction maps change slowly.
pub const SNAPSHOT_INTERVAL_MS: i64 = 3_600_000;

/// A week of hourly frames: each 123×123 grid is about 5 kB of base64.
pub const OBSTRUCTION_RETENTION_MS: i64 = 7 * 24 * 3_600_000;

const OBSTRUCTED_FRACTION_FLOOR: f64 = 0.005;
const PARTIAL_FRACTION_CEILING: f64 = 0.25;

const CELL_UNMAPPED: u8 = 0;
const CELL_CLEAR: u8 = 1;
const CELL_PARTIAL: u8 = 2;
const CELL_OBSTRUCTED: u8 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObstructionSnapshot {
    pub taken_at_ms: i64,
    pub grid_size: u32,
    /// Base64 of 2 bits per cell: 0 unmapped, 1 clear, 2 partial, 3 obstructed.
    pub packed_cells: String,
    /// Half-angle of the sky cone this grid covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_theta_deg: Option<f64>,
}

impl ObstructionSnapshot {
    /// The packed cell bytes, or `None` when the base64 does not decode
    /// (spec 0179).
    pub fn packed_bytes(&self) -> Option<Vec<u8>> {
        STANDARD.decode(&self.packed_cells).ok()
    }
}

fn quantize_cell(fraction_usable: f64) -> u8 {
    if fraction_usable < 0.0 {
        return CELL_UNMAPPED;
    }
    let obstructed = 1.0 - fraction_usable;
    if obstructed <= OBSTRUCTED_FRACTION_FLOOR {
        CELL_CLEAR
    } else if obstructed <= PARTIAL_FRACTION_CEILING {
        CELL_PARTIAL
    } else {
        CELL_OBSTRUCTED
    }
}

pub fn pack_cells(grid: &[f64]) -> String {
    let mut packed = vec![0u8; grid.len().div_ceil(4)];
    for (index, fraction_usable) in grid.iter().enumerate() {
        packed[index >> 2] |= quantize_cell(*fraction_usable) << ((index & 3) * 2);
    }
    STANDARD.encode(packed)
}

#[derive(Debug)]
pub struct ObstructionStore {
    file_path: PathBuf,
    snapshots: Vec<ObstructionSnapshot>,
}

impl ObstructionStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        // A snapshot without a grid cannot be rendered, so drop it on read
        // rather than hand the scrubber a hole.
        let snapshots = read_json_lines_with(&file_path, |snapshot: &ObstructionSnapshot| {
            snapshot.grid_size > 0
        })?;
        Ok(Self {
            file_path,
            snapshots,
        })
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Newest last, which is the order the scrubber expects.
    pub fn list(&self) -> &[ObstructionSnapshot] {
        &self.snapshots
    }

    /// Is the newest snapshot old enough that another is due?
    pub fn is_due(&self, now_ms: i64) -> bool {
        self.snapshots
            .last()
            .is_none_or(|snapshot| now_ms - snapshot.taken_at_ms >= SNAPSHOT_INTERVAL_MS)
    }

    /// Append one and drop anything past the retention window.
    pub fn record(&mut self, snapshot: ObstructionSnapshot) -> JsonlResult<&[ObstructionSnapshot]> {
        let cutoff = snapshot.taken_at_ms - OBSTRUCTION_RETENTION_MS;
        self.snapshots
            .retain(|existing| existing.taken_at_ms >= cutoff);
        self.snapshots.push(snapshot);
        write_json_lines_atomically(&self.file_path, &self.snapshots)?;
        Ok(&self.snapshots)
    }
}

#[cfg(test)]
mod tests {

    // SPEC-LAPSE-001: packed cells round-trip through base64, and a snapshot
    // that does not decode is refused.
    #[test]
    fn snapshot_cells_decode_back_to_bytes() {
        let snapshot = ObstructionSnapshot {
            taken_at_ms: 0,
            grid_size: 2,
            packed_cells: pack_cells(&[1.0, 0.9, 0.5, 0.0]),
            max_theta_deg: None,
        };
        assert_eq!(snapshot.packed_bytes(), Some(vec![0xf9]));
        let broken = ObstructionSnapshot {
            packed_cells: "not base64!".into(),
            ..snapshot
        };
        assert_eq!(broken.packed_bytes(), None);
    }

    use tempfile::tempdir;

    use super::*;

    const NOW_MS: i64 = 1_781_524_800_000;

    // SPEC-RECORDER-004: obstruction cells keep the TypeScript store's packing.
    #[test]
    fn cells_pack_two_bits_each_in_type_script_order() {
        assert_eq!(pack_cells(&[1.0, 0.9, 0.5, 0.0]), "+Q==");
        assert_eq!(pack_cells(&[-1.0, 0.999, 0.99, 0.5]), "5A==");
        assert_eq!(pack_cells(&[]), "");
    }

    #[test]
    fn snapshots_are_due_hourly_and_retained_for_a_week() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("obstruction.ndjson");
        let mut store = ObstructionStore::open(&path).expect("open");
        assert!(store.is_due(NOW_MS));

        let snapshot = |taken_at_ms: i64| ObstructionSnapshot {
            taken_at_ms,
            grid_size: 123,
            packed_cells: "AAAA".into(),
            max_theta_deg: Some(60.0),
        };
        store.record(snapshot(NOW_MS)).expect("record");
        assert!(!store.is_due(NOW_MS + SNAPSHOT_INTERVAL_MS - 1));
        assert!(store.is_due(NOW_MS + SNAPSHOT_INTERVAL_MS));

        store
            .record(snapshot(NOW_MS + OBSTRUCTION_RETENTION_MS + 1))
            .expect("newer record");
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].taken_at_ms, NOW_MS + OBSTRUCTION_RETENTION_MS + 1);
    }
}
