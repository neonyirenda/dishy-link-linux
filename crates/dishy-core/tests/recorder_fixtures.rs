//! Recorder fixtures: the store directories in `fixtures/recorder/` must read
//! and compact to their `expected/` copies.
//!
//! The fixtures were written with `TZ=UTC` and a fixed clock, so these cases
//! use `UtcCalendar`; production uses the system timezone through
//! `LocalCalendar`.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use dishy_core::calendar::UtcCalendar;
use dishy_core::energy::MinuteBucket;
use dishy_core::store::{
    EnergyStore, EventStore, LatencyStore, ObstructionSnapshot, ObstructionStore, ThermalStore,
    pack_cells,
};
use dishy_core::telemetry::OutageEvent;
use serde::Deserialize;
use support::assert_json_value_matches;
use tempfile::TempDir;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/recorder")
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination");
    for entry in fs::read_dir(source).expect("read source directory") {
        let entry = entry.expect("directory entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy file");
        }
    }
}

fn prepare(case: &str) -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp dir");
    let destination = temp.path().join(case);
    copy_directory(&fixtures().join(case).join("input"), &destination);
    (temp, destination)
}

fn json_lines(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture JSON"))
        .collect()
}

fn assert_file_matches(actual: &Path, expected: &Path) {
    let actual = serde_json::Value::Array(json_lines(actual));
    let expected = serde_json::Value::Array(json_lines(expected));
    assert_json_value_matches(&actual, &expected);
}

// SPEC-RECORDER-004: a directory the TypeScript store wrote reads and compacts
// to the same JSON Lines rows here.
#[test]
fn energy_fixture_compacts_and_appends_identically() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        now_ms: i64,
        append: MinuteBucket,
    }

    let (_temp, directory) = prepare("energy");
    let meta: Meta = serde_json::from_str(
        &fs::read_to_string(fixtures().join("energy/meta.json")).expect("meta"),
    )
    .expect("meta JSON");
    let mut store =
        EnergyStore::open_with(directory.join("energy.ndjson"), meta.now_ms, &UtcCalendar)
            .expect("open");
    store.append(&meta.append).expect("append");

    let expected = fixtures().join("energy/expected");
    assert_file_matches(
        &directory.join("energy.ndjson"),
        &expected.join("energy.ndjson"),
    );
    assert_file_matches(
        &directory.join("energy-monthly.ndjson"),
        &expected.join("energy-monthly.ndjson"),
    );
}

#[test]
fn latency_fixture_compacts_identically() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        now_ms: i64,
    }

    let (_temp, directory) = prepare("latency");
    let meta: Meta = serde_json::from_str(
        &fs::read_to_string(fixtures().join("latency/meta.json")).expect("meta"),
    )
    .expect("meta JSON");
    let _store =
        LatencyStore::open_with(directory.join("latency.ndjson"), meta.now_ms, &UtcCalendar)
            .expect("open");

    let expected = fixtures().join("latency/expected");
    assert_file_matches(
        &directory.join("latency.ndjson"),
        &expected.join("latency.ndjson"),
    );
    assert_file_matches(
        &directory.join("latency-monthly.ndjson"),
        &expected.join("latency-monthly.ndjson"),
    );
}

#[test]
fn events_fixture_upserts_and_expires_identically() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        now_ms: i64,
        incoming: Vec<OutageEvent>,
    }

    let (_temp, directory) = prepare("events");
    let meta: Meta = serde_json::from_str(
        &fs::read_to_string(fixtures().join("events/meta.json")).expect("meta"),
    )
    .expect("meta JSON");
    let mut store = EventStore::open(directory.join("events.ndjson")).expect("open");
    store.upsert(meta.incoming, meta.now_ms).expect("upsert");
    assert_file_matches(
        &directory.join("events.ndjson"),
        &fixtures().join("events/expected/events.ndjson"),
    );
}

#[test]
fn thermal_fixture_opens_and_closes_identically() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Action {
        alert_key: String,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        now_ms: i64,
        open: Action,
        close: Action,
    }

    let (_temp, directory) = prepare("thermal");
    let meta: Meta = serde_json::from_str(
        &fs::read_to_string(fixtures().join("thermal/meta.json")).expect("meta"),
    )
    .expect("meta JSON");
    let mut store = ThermalStore::open(directory.join("thermal.ndjson")).expect("open");
    store
        .open_episode(
            &meta.open.alert_key,
            meta.open.start_ms.expect("start"),
            meta.now_ms,
        )
        .expect("open");
    store
        .close_episode(
            &meta.close.alert_key,
            meta.close.end_ms.expect("end"),
            meta.now_ms,
        )
        .expect("close");
    assert_file_matches(
        &directory.join("thermal.ndjson"),
        &fixtures().join("thermal/expected/thermal.ndjson"),
    );
}

#[test]
fn obstruction_fixture_packs_and_retains_identically() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Record {
        taken_at_ms: i64,
        grid_size: u32,
        grid: Vec<f64>,
        max_theta_deg: Option<f64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        record: Record,
    }

    let (_temp, directory) = prepare("obstruction");
    let meta: Meta = serde_json::from_str(
        &fs::read_to_string(fixtures().join("obstruction/meta.json")).expect("meta"),
    )
    .expect("meta JSON");
    let mut store = ObstructionStore::open(directory.join("obstruction.ndjson")).expect("open");
    store
        .record(ObstructionSnapshot {
            taken_at_ms: meta.record.taken_at_ms,
            grid_size: meta.record.grid_size,
            packed_cells: pack_cells(&meta.record.grid),
            max_theta_deg: meta.record.max_theta_deg,
        })
        .expect("record");
    assert_file_matches(
        &directory.join("obstruction.ndjson"),
        &fixtures().join("obstruction/expected/obstruction.ndjson"),
    );
}
