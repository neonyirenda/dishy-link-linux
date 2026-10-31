//! Device classification, OUI lookup, and odometer conformance tests against
//! the golden outputs in `fixtures/telemetry/`.

mod support;

use dishy_core::device_kind::classify_device;
use dishy_core::odometer::{ClientReading, ClientTotalsCore, Snapshot};
use dishy_core::oui::{OuiRegistry, is_private_mac};
use serde::Deserialize;
use serde_json::Value;
use support::assert_json_value_matches;

const GOLDEN_JSON: &str = include_str!("../fixtures/telemetry/device-odometer-golden.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoldenData {
    classification_cases: Vec<ClassificationCase>,
    oui_cases: Vec<OuiCase>,
    scenario: Vec<StepRecord>,
    final_snapshot: Value,
    final_totals: Value,
}

#[derive(Deserialize)]
struct ClassificationCase {
    input: String,
    expected: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OuiCase {
    mac: String,
    is_private: bool,
    expected_vendor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepRecord {
    at_ms: i64,
    entries: Vec<StepEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepEntry {
    client_id: Option<i64>,
    mac_address: String,
    rx_bytes: u64,
    tx_bytes: u64,
    name: Option<String>,
}

// SPEC-DEV-001: OUI vendor and private MAC detection.
#[test]
fn oui_vendor_matches_the_reference_implementation() {
    let golden: GoldenData = serde_json::from_str(GOLDEN_JSON).expect("golden JSON");
    let registry = OuiRegistry::bundled();

    for case in golden.oui_cases {
        assert_eq!(
            is_private_mac(&case.mac),
            case.is_private,
            "private MAC mismatch on {}",
            case.mac
        );
        let vendor = registry.vendor_for_mac(&case.mac);
        assert_eq!(
            vendor.map(str::to_string),
            case.expected_vendor,
            "vendor mismatch on {}",
            case.mac
        );
    }
}

// SPEC-DEV-002: device classification matches reference rules.
#[test]
fn device_classification_matches_the_reference_implementation() {
    let golden: GoldenData = serde_json::from_str(GOLDEN_JSON).expect("golden JSON");

    for case in golden.classification_cases {
        let kind = classify_device(Some(&case.input));
        assert_eq!(
            kind.as_str(),
            case.expected,
            "classification mismatch on {:?}",
            case.input
        );
    }
}

// SPEC-ODO-001, SPEC-ODO-002, SPEC-ODO-003, SPEC-ODO-004: odometer scenario conformance.
#[test]
fn odometer_scenario_matches_the_reference_implementation() {
    let golden: GoldenData = serde_json::from_str(GOLDEN_JSON).expect("golden JSON");
    let mut core = ClientTotalsCore::with_options(dishy_core::odometer::DEFAULT_MAX_GAP_MS, true);

    for step in golden.scenario {
        let entries_ref: Vec<(Option<i64>, &str)> = step
            .entries
            .iter()
            .map(|e| (e.client_id, e.mac_address.as_str()))
            .collect();
        let live_keys = core.note_poll(&entries_ref);

        for entry in step.entries {
            core.observe(
                ClientReading {
                    client_id: entry.client_id,
                    mac_address: &entry.mac_address,
                    rx_bytes: entry.rx_bytes,
                    tx_bytes: entry.tx_bytes,
                    at_ms: step.at_ms,
                    name: entry.name.as_deref(),
                    ip_address: None,
                },
                &live_keys,
            );
        }
    }

    let actual_snapshot = serde_json::to_value(core.to_snapshot()).expect("snapshot value");
    assert_json_value_matches(&actual_snapshot, &golden.final_snapshot);

    let actual_totals = serde_json::to_value(core.totals(None)).expect("totals value");
    assert_json_value_matches(&actual_totals, &golden.final_totals);
}

// SPEC-ODO-005: persistence round-trip.
#[test]
fn odometer_persistence_round_trips() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file_path = temp_dir.path().join("client-totals.json");

    let golden: GoldenData = serde_json::from_str(GOLDEN_JSON).expect("golden JSON");
    let snapshot: Snapshot =
        serde_json::from_value(golden.final_snapshot.clone()).expect("snapshot decode");

    {
        let mut store = dishy_core::odometer::ClientTotalsStore::open(&file_path);
        store.core_mut().load_snapshot(snapshot);
        store.snapshot().expect("snapshot save");
    }

    {
        let store = dishy_core::odometer::ClientTotalsStore::open(&file_path);
        let reloaded = serde_json::to_value(store.core().to_snapshot()).expect("snapshot value");
        assert_json_value_matches(&reloaded, &golden.final_snapshot);
    }
}

// SPEC-ODO-009: loading a v3 file upgrades its totals and keeps identity evidence.
#[test]
fn version_three_snapshots_migrate_to_version_four() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file_path = temp_dir.path().join("client-totals.json");
    let old_snapshot = serde_json::json!({
        "version": 3,
        "totals": [{
            "clientId": 42,
            "macAddress": "10:00:20:11:22:33",
            "name": "Laptop",
            "rxBytes": 1200,
            "txBytes": 300,
            "sinceMs": 1_781_524_800_000_i64,
            "lastSeenMs": 1_781_524_810_000_i64,
            "periodMonth": 24317,
            "prevRx": 1200,
            "prevTx": 300,
            "lastPollMs": 1_781_524_810_000_i64
        }],
        "sharedMacs": ["60:74:f4:aa:bb:01"],
        "aliases": [["41", "42"]],
        "rejectedPairs": [["42", "43"]]
    });
    std::fs::write(&file_path, serde_json::to_vec(&old_snapshot).unwrap()).unwrap();

    let store = dishy_core::odometer::ClientTotalsStore::open(&file_path);
    let snapshot = store.core().to_snapshot();
    let total = store.core().totals(None).pop().expect("migrated total");
    assert_eq!(snapshot.version, dishy_core::odometer::VERSION);
    assert_eq!(total.rx_bytes, 1_200);
    assert_eq!(total.tx_bytes, 300);
    assert_eq!(snapshot.shared_macs, ["60:74:f4:aa:bb:01"]);
    assert_eq!(snapshot.aliases, [("41".into(), "42".into())]);
    assert_eq!(snapshot.rejected_pairs, [("42".into(), "43".into())]);
}

// SPEC-ODO-008: metadata updates when present and remains when a field is absent.
#[test]
fn client_metadata_tracks_non_empty_values_without_erasing_them() {
    let mut core = dishy_core::odometer::ClientTotalsCore::with_options(15_000, true);
    let mac = "10:00:20:11:22:33";
    let at_ms = 1_781_524_800_000;
    let live = core.note_poll(&[(Some(42), mac)]);
    core.observe(
        ClientReading {
            client_id: Some(42),
            mac_address: mac,
            rx_bytes: 100,
            tx_bytes: 50,
            at_ms,
            name: Some("Laptop"),
            ip_address: Some("192.168.1.10"),
        },
        &live,
    );
    core.observe(
        ClientReading {
            client_id: Some(42),
            mac_address: mac,
            rx_bytes: 200,
            tx_bytes: 100,
            at_ms: at_ms + 5_000,
            name: Some("Work laptop"),
            ip_address: Some("192.168.1.20"),
        },
        &live,
    );
    core.observe(
        ClientReading {
            client_id: Some(42),
            mac_address: mac,
            rx_bytes: 300,
            tx_bytes: 150,
            at_ms: at_ms + 10_000,
            name: None,
            ip_address: None,
        },
        &live,
    );

    let total = core.totals(None).pop().expect("client total");
    assert_eq!(total.name.as_deref(), Some("Work laptop"));
    assert_eq!(total.ip_address.as_deref(), Some("192.168.1.20"));
    assert_eq!(total.rx_bytes, 200);
    assert_eq!(total.tx_bytes, 100);
}

mod merge {
    use dishy_core::odometer::{ClientReading, ClientTotalsCore};

    const DAY: i64 = 86_400_000;
    const T: i64 = 1_800_000_000_000;

    fn see(core: &mut ClientTotalsCore, id: i64, mac: &str, name: &str, rx: u64, at_ms: i64) {
        let live = core.note_poll(&[(Some(id), mac)]);
        core.observe(
            ClientReading {
                client_id: Some(id),
                mac_address: mac,
                rx_bytes: rx,
                tx_bytes: 0,
                at_ms,
                name: Some(name),
                ip_address: None,
            },
            &live,
        );
    }

    fn two_records() -> ClientTotalsCore {
        let mut core = ClientTotalsCore::with_options(60_000, true);
        see(&mut core, 1, "aa:00:00:00:00:01", "Laptop", 0, T - DAY);
        see(
            &mut core,
            1,
            "aa:00:00:00:00:01",
            "Laptop",
            1_000,
            T - DAY + 1_000,
        );
        see(&mut core, 2, "aa:00:00:00:00:02", " laptop ", 0, T - 1_000);
        see(&mut core, 2, "aa:00:00:00:00:02", " laptop ", 500, T);
        core
    }

    // SPEC-MERGE-001: an idle record with the same name as a more recent one is
    // proposed, with the outcome stated.
    #[test]
    fn same_name_idle_records_are_proposed() {
        let core = two_records();
        let candidates = core.merge_candidates(T);
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(
            (candidate.from_key.as_str(), candidate.to_key.as_str()),
            ("1", "2")
        );
        assert_eq!(candidate.detail, "laptop");
        assert!(candidate.folds_bytes);
        assert_eq!(candidate.result_rx_bytes, 1_500);
        assert!(
            core.merge_candidates(T - DAY + 2_000).is_empty(),
            "the source is not idle yet"
        );
    }

    // SPEC-MERGE-002, SPEC-MERGE-003: accepting folds and aliases; declining is
    // remembered either way round; both survive a snapshot.
    #[test]
    fn answers_fold_or_are_remembered() {
        let mut core = two_records();
        assert!(core.reject_merge("2", "1"));
        assert!(core.merge_candidates(T).is_empty());
        let mut restored = ClientTotalsCore::with_options(60_000, true);
        restored.load_snapshot(core.to_snapshot());
        assert!(
            restored.merge_candidates(T).is_empty(),
            "the answer persists"
        );

        assert!(
            core.merge("1", "2"),
            "an explicit merge overrides the earlier no"
        );
        assert_eq!(core.resolve_key("1"), "2");
        let survivor = core.totals(Some("2")).pop().unwrap();
        assert_eq!(survivor.rx_bytes, 1_500);
        assert!(
            core.totals(None)
                .iter()
                .all(|total| total.client_id != Some(1))
        );
        assert!(!core.merge("1", "2"), "already one record");
        assert!(!core.merge("9", "2"), "unknown key");
    }
}
