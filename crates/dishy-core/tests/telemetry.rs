//! Telemetry behaviour, checked against the expected outputs in
//! `fixtures/telemetry/` where a fixture covers it, and against the documented
//! edge cases where the fixture cannot reach.

mod support;

use dishy_core::energy::{
    MinuteBucket, add_minute_bucket, fold_minutes_to_months, fold_samples_to_minutes,
};
use dishy_core::latency::{
    LATENCY_BIN_COUNT, LatencyMinuteBucket, add_latency_month_bucket, bin_index_for,
    fold_latency_minutes_to_months, fold_samples_to_latency_minutes, replace_latency_minute_bucket,
};
use dishy_core::telemetry::{
    DishHistory, DishOutage, DishStatus, HistoryWindow, ProtoF64, RouterReadings, SampleCursor,
    TelemetryAccumulator, TelemetrySample, U64Value, canonical_cause, decode_history_window,
    decode_outage_events, is_starlink_outage, known_event_kind, outage_event_kind,
    read_router_latency_ms, read_router_ping_success_percent, select_fresh_samples,
};
use dishy_core::throughput::{ByteCounters, ThroughputRates, ThroughputTracker};
use dishy_proto::Schema;
use serde::Deserialize;
use support::assert_json_value_matches;

const HISTORY_RESPONSE: &[u8] = include_bytes!("../fixtures/grpc/history.response.bin");
const STATUS_RESPONSE: &[u8] = include_bytes!("../fixtures/grpc/status.response.bin");

const WINDOW_GOLDEN: &str = include_str!("../fixtures/telemetry/window.json");
const EVENT_LOG_GOLDEN: &str = include_str!("../fixtures/telemetry/outages-event-log.json");
const GPS_GOLDEN: &str = include_str!("../fixtures/telemetry/outages-gps.json");
const FRESH_GOLDEN: &str = include_str!("../fixtures/telemetry/fresh-samples.json");
const LATENCY_GOLDEN: &str = include_str!("../fixtures/telemetry/latency-minutes.json");
const ENERGY_GOLDEN: &str = include_str!("../fixtures/telemetry/energy-minutes.json");
const THROUGHPUT_GOLDEN: &str = include_str!("../fixtures/telemetry/throughput.json");

fn captured_response(pointer: &str) -> serde_json::Value {
    let schema = Schema::bundled().expect("schema");
    let response = schema
        .decode_response_json(HISTORY_RESPONSE)
        .expect("history response");
    response
        .get(pointer)
        .cloned()
        .unwrap_or_else(|| panic!("{pointer} in the captured response"))
}

fn captured_history() -> DishHistory {
    serde_json::from_value(captured_response("dishGetHistory")).expect("typed history")
}

fn sample(timestamp_ms: i64) -> TelemetrySample {
    TelemetrySample {
        timestamp_ms,
        ..TelemetrySample::default()
    }
}

// SPEC-TELEMETRY-001 and SPEC-RECORDER-002: the ring unrolls to absolute
// time, with real readings
// kept and sentinels turned into nulls.
#[test]
fn ring_buffer_unrolls_to_the_typescript_window() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        now_ms: i64,
        window: HistoryWindow,
    }

    let fixture: Fixture = serde_json::from_str(WINDOW_GOLDEN).expect("window fixture");
    let actual = decode_history_window(&captured_history(), fixture.now_ms);
    assert_eq!(actual.samples.len(), 900);
    assert_json_value_matches(
        &serde_json::to_value(&actual).expect("window"),
        &serde_json::to_value(&fixture.window).expect("golden window"),
    );
}

#[test]
fn ring_unroll_handles_wraparound_and_keeps_the_newest_sample_at_now() {
    // Longer than one ring: absolute counters 7..=11 land at indices 2,3,4,0,1.
    let history = DishHistory {
        current: Some(U64Value(12)),
        pop_ping_latency_ms: vec![10.0, 20.0, 30.0, 40.0, 50.0],
        ..DishHistory::default()
    };
    let window = decode_history_window(&history, 100_000);
    let latencies: Vec<_> = window
        .samples
        .iter()
        .map(|sample| sample.latency_ms)
        .collect();
    assert_eq!(
        latencies,
        vec![Some(30.0), Some(40.0), Some(50.0), Some(10.0), Some(20.0)]
    );
    assert_eq!(window.samples[0].timestamp_ms, 96_000);
    assert_eq!(window.samples[4].timestamp_ms, 100_000);
}

#[test]
fn non_positive_and_non_finite_latency_is_not_a_reading() {
    let history = DishHistory {
        current: Some(U64Value(4)),
        pop_ping_latency_ms: vec![0.0, -1.0, f64::NAN, 12.5],
        ..DishHistory::default()
    };
    let window = decode_history_window(&history, 0);
    assert_eq!(window.samples[0].latency_ms, None);
    assert_eq!(window.samples[1].latency_ms, None);
    assert_eq!(window.samples[2].latency_ms, None);
    assert_eq!(window.samples[3].latency_ms, Some(12.5));
}

#[test]
fn dish_status_serializes_proto_special_values_and_large_counters() {
    let status = DishStatus {
        pop_ping_latency_ms: ProtoF64(f64::NAN),
        device_state: Some(dishy_core::telemetry::DeviceState {
            uptime_s: Some(U64Value(u64::MAX)),
        }),
        ..DishStatus::default()
    };
    let value = serde_json::to_value(status).expect("serializable diagnostics status");
    assert_eq!(value["popPingLatencyMs"], "NaN");
    assert_eq!(value["deviceState"]["uptimeS"], u64::MAX.to_string());
}

#[test]
fn an_empty_or_unstarted_ring_yields_no_samples() {
    let empty = decode_history_window(&DishHistory::default(), 1);
    assert!(empty.samples.is_empty());
    let unstarted = decode_history_window(
        &DishHistory {
            current: Some(U64Value(0)),
            pop_ping_latency_ms: vec![1.0],
            ..DishHistory::default()
        },
        1,
    );
    assert!(unstarted.samples.is_empty());
}

// SPEC-TELEMETRY-002 and SPEC-RECORDER-003: outage timestamps are converted
// from the GPS epoch; the event log is already Unix. SPEC-TELEMETRY-003: the
// event log wins.
#[test]
fn captured_outages_decode_from_both_epochs() {
    let history = captured_history();
    let actual = decode_outage_events(&history);
    assert_json_value_matches(
        &serde_json::to_value(&actual).expect("event log outages"),
        &serde_json::from_str(EVENT_LOG_GOLDEN).expect("golden"),
    );

    let gps_only = DishHistory {
        event_log: Some(Default::default()),
        ..history
    };
    let actual = decode_outage_events(&gps_only);
    assert_json_value_matches(
        &serde_json::to_value(&actual).expect("gps outages"),
        &serde_json::from_str(GPS_GOLDEN).expect("golden"),
    );
}

#[test]
fn gps_epoch_conversion_is_exact() {
    let history = DishHistory {
        outages: vec![DishOutage {
            cause: Some("BOOTING".into()),
            start_timestamp_ns: Some(U64Value(1_000_000_000)),
            duration_ns: Some(U64Value(2_000_000_000)),
            did_switch: None,
        }],
        ..DishHistory::default()
    };
    let events = decode_outage_events(&history);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].start_ms, 315_964_783_000);
    assert_eq!(events[0].duration_ms, 2_000);
    assert_eq!(events[0].severity, dishy_core::telemetry::Severity::Warning);
}

#[test]
fn event_log_prefers_unix_timestamps_and_maps_severity() {
    use dishy_core::telemetry::{DishEvent, EventLog, Severity};

    let event = DishEvent {
        severity: Some("EVENT_SEVERITY_CRITICAL".into()),
        reason: Some("EVENT_REASON_OUTAGE_NO_PINGS".into()),
        start_timestamp_ns: Some(U64Value(1_784_081_136_126_000_000)),
        duration_ns: Some(U64Value(38_433_000_000)),
        client_switching_band_metadata: None,
    };
    let history = DishHistory {
        event_log: Some(EventLog {
            events: vec![event],
        }),
        ..DishHistory::default()
    };
    let events = decode_outage_events(&history);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].start_ms, 1_784_081_136_126);
    assert_eq!(events[0].duration_ms, 38_433);
    assert_eq!(events[0].severity, Severity::Critical);
}

// SPEC-TELEMETRY-004: cause tokens and kinds are stable across firmware labels.
#[test]
fn causes_canonicalise_and_kinds_default_to_info() {
    use dishy_core::telemetry::EventKind;

    assert_eq!(canonical_cause("EVENT_REASON_OUTAGE_NO_PINGS"), "NO_PINGS");
    assert_eq!(
        canonical_cause("outage-high downlink-packet-loss"),
        "HIGH_DOWNLINK_PACKET_LOSS"
    );
    assert_eq!(
        canonical_cause("  UT_ALERT_ROUTER_POWER_CYCLE  "),
        "ROUTER_POWER_CYCLE"
    );
    assert_eq!(canonical_cause("EVENT_REASON_OUTAGE_BOOTING"), "BOOTING");

    assert_eq!(
        known_event_kind("EVENT_REASON_OUTAGE_BOOTING"),
        Some(EventKind::Outage)
    );
    assert_eq!(
        known_event_kind("ROUTER_POWER_CYCLE"),
        Some(EventKind::Info)
    );
    assert_eq!(known_event_kind("A_NEW_FIRMWARE_THING"), None);
    assert_eq!(
        outage_event_kind("EVENT_REASON_OUTAGE_NEW_THING"),
        EventKind::Outage
    );
    assert_eq!(outage_event_kind("A_NEW_FIRMWARE_THING"), EventKind::Info);
}

// SPEC-TELEMETRY-005: a cursor trusts the dish counter when it can, and falls
// back to timestamps after a reboot.
#[test]
fn captured_cursors_select_the_same_fresh_samples() {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        window_now_ms: i64,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        cursor: SampleCursor,
        timestamps: Vec<i64>,
    }

    let fixture: Fixture = serde_json::from_str(FRESH_GOLDEN).expect("fresh fixture");
    let window = decode_history_window(&captured_history(), fixture.window_now_ms);
    for case in fixture.cases {
        let actual: Vec<_> = select_fresh_samples(&window, case.cursor)
            .into_iter()
            .map(|sample| sample.timestamp_ms)
            .collect();
        assert_eq!(actual, case.timestamps);
    }
}

#[test]
fn cursor_falls_back_to_time_when_the_counter_restarted() {
    let window = HistoryWindow {
        samples: (0..10).map(|i| sample(i * 1_000)).collect(),
        newest_counter: 100,
    };

    assert_eq!(
        select_fresh_samples(&window, SampleCursor::default()).len(),
        10
    );
    assert_eq!(
        select_fresh_samples(
            &window,
            SampleCursor {
                counter: 95,
                newest_sample_ms: 0,
            }
        )
        .len(),
        5
    );
    assert!(
        select_fresh_samples(
            &window,
            SampleCursor {
                counter: 100,
                newest_sample_ms: 0,
            }
        )
        .is_empty()
    );
    assert_eq!(
        select_fresh_samples(
            &window,
            SampleCursor {
                counter: 101,
                newest_sample_ms: 0,
            }
        )
        .len(),
        10
    );
    // 500 ms of grace absorbs sub-second jitter in stamped times.
    let after_five = select_fresh_samples(
        &window,
        SampleCursor {
            counter: 0,
            newest_sample_ms: 5_000,
        },
    );
    assert_eq!(after_five.len(), 4);
    assert_eq!(after_five[0].timestamp_ms, 6_000);
}

// SPEC-TELEMETRY-006: the accumulator stitches polls, stamps router readings,
// and trims to a span of real time.
#[test]
fn accumulator_seeds_once_and_trims_from_the_newest_sample() {
    let mut accumulator = TelemetryAccumulator::new(6_000);
    let persisted: Vec<_> = (0..10).map(|i| sample(i * 1_000)).collect();
    assert_eq!(accumulator.seed(&persisted).len(), 7);
    assert_eq!(accumulator.samples()[0].timestamp_ms, 3_000);

    // A later seed is a no-op: live data is authoritative.
    accumulator.seed(&[sample(99_000)]);
    assert_eq!(accumulator.samples().len(), 7);
}

#[test]
fn accumulator_ingests_only_new_samples_and_stamps_the_router() {
    let mut accumulator = TelemetryAccumulator::new(60_000);
    let first = HistoryWindow {
        samples: (0..5).map(|i| sample(i * 1_000)).collect(),
        newest_counter: 5,
    };
    let fresh = accumulator.ingest_window(
        &first,
        RouterReadings {
            latency_ms: Some(21.0),
            ping_success_percent: Some(99.0),
        },
    );
    assert_eq!(fresh.len(), 5);
    assert!(
        fresh
            .iter()
            .all(|sample| sample.router_latency_ms == Some(21.0))
    );
    assert!(
        fresh
            .iter()
            .all(|sample| sample.router_ping_success_percent == Some(99.0))
    );

    let second = HistoryWindow {
        samples: vec![sample(4_000), sample(5_000)],
        newest_counter: 6,
    };
    let all = accumulator.ingest_window(&second, RouterReadings::default());
    assert_eq!(all.len(), 6);
    assert_eq!(all.last().expect("newest").timestamp_ms, 5_000);
    assert_eq!(all.last().expect("newest").router_latency_ms, None);

    // Re-seeing a counter adds nothing.
    let before = accumulator.samples().len();
    accumulator.ingest_window(&second, RouterReadings::default());
    assert_eq!(accumulator.samples().len(), before);
}

#[test]
fn accumulator_trim_uses_retention_not_the_poll_time() {
    let mut accumulator = TelemetryAccumulator::new(3_000);
    let window = HistoryWindow {
        samples: (0..10).map(|i| sample(i * 1_000)).collect(),
        newest_counter: 10,
    };
    accumulator.ingest_window(&window, RouterReadings::default());
    assert_eq!(accumulator.samples().len(), 4);
    assert_eq!(accumulator.samples()[0].timestamp_ms, 6_000);
    assert_eq!(accumulator.newest_counter(), 10);
}

// SPEC-TELEMETRY-007: an outage is eight consecutive fully-dropped samples.
#[test]
fn outage_needs_a_full_run_of_eight_dropped_samples() {
    let dropped = |count: usize| -> Vec<TelemetrySample> {
        (0..count)
            .map(|i| TelemetrySample {
                timestamp_ms: i as i64,
                drop_rate: 1.0,
                ..TelemetrySample::default()
            })
            .collect()
    };
    assert!(!is_starlink_outage(&dropped(7)));
    assert!(is_starlink_outage(&dropped(8)));
    let mut nine = dropped(9);
    nine[0].drop_rate = 0.0;
    assert!(is_starlink_outage(&nine));
}

// SPEC-TELEMETRY-008: only a real positive reading reaches the chart; an absent
// five-minute drop rate means no drops seen, not unknown.
#[test]
fn router_readings_reject_sentinels() {
    assert_eq!(read_router_latency_ms(None), None);
    assert_eq!(read_router_latency_ms(Some(0.0)), None);
    assert_eq!(read_router_latency_ms(Some(-1.0)), None);
    assert_eq!(read_router_latency_ms(Some(f64::NAN)), None);
    assert_eq!(read_router_latency_ms(Some(14.5)), Some(14.5));

    assert_eq!(read_router_ping_success_percent(None), Some(100.0));
    assert_eq!(read_router_ping_success_percent(Some(0.0)), Some(100.0));
    assert_eq!(read_router_ping_success_percent(Some(0.25)), Some(75.0));
    assert_eq!(read_router_ping_success_percent(Some(1.0)), Some(0.0));
    assert_eq!(read_router_ping_success_percent(Some(1.5)), None);
    assert_eq!(read_router_ping_success_percent(Some(f64::NAN)), None);
}

// SPEC-BUCKET-001: the histogram edges are fixed and the overflow bin is last.
#[test]
fn latency_bins_follow_the_shared_edges() {
    assert_eq!(LATENCY_BIN_COUNT, 16);
    assert_eq!(bin_index_for(0.0), 0);
    assert_eq!(bin_index_for(9.999), 0);
    assert_eq!(bin_index_for(10.0), 1);
    assert_eq!(bin_index_for(99.999), 9);
    assert_eq!(bin_index_for(100.0), 10);
    assert_eq!(bin_index_for(149.999), 10);
    assert_eq!(bin_index_for(150.0), 11);
    assert_eq!(bin_index_for(999.999), 14);
    assert_eq!(bin_index_for(1_000.0), 15);
    assert_eq!(bin_index_for(5_000.0), 15);
}

// SPEC-BUCKET-002: minute folds count every sample, but only real readings
// enter the latency histogram.
#[test]
fn captured_latency_minutes_match_the_typescript_fold() {
    let now_ms = 1_800_000_000_000;
    let samples = decode_history_window(&captured_history(), now_ms).samples;
    let actual: Vec<_> = fold_samples_to_latency_minutes(&samples)
        .into_values()
        .collect();
    assert_json_value_matches(
        &serde_json::to_value(&actual).expect("latency minutes"),
        &serde_json::from_str(LATENCY_GOLDEN).expect("golden"),
    );
}

#[test]
fn latency_minute_fold_keeps_drop_stats_even_without_a_ping() {
    let samples = vec![
        TelemetrySample {
            timestamp_ms: 130_000,
            latency_ms: Some(20.0),
            router_latency_ms: Some(30.0),
            drop_rate: 0.5,
            ..TelemetrySample::default()
        },
        TelemetrySample {
            timestamp_ms: 131_000,
            latency_ms: None,
            router_latency_ms: None,
            drop_rate: 1.0,
            ..TelemetrySample::default()
        },
        TelemetrySample {
            timestamp_ms: 190_000,
            latency_ms: Some(120.0),
            ..TelemetrySample::default()
        },
    ];
    let minutes = fold_samples_to_latency_minutes(&samples);
    assert_eq!(minutes.len(), 2);
    let first = &minutes[&120];
    assert_eq!(first.samples, 2);
    assert_eq!(first.dish.count, 1);
    assert_eq!(first.dish.sum_ms, 20.0);
    assert_eq!(first.dish.min_ms, Some(20.0));
    assert_eq!(first.router.count, 1);
    assert_eq!(first.drop_sum, 1.5);
    assert_eq!(first.drop_count, 2);
    let second = &minutes[&180];
    assert_eq!(second.dish.bins[10], 1);
}

// SPEC-BUCKET-003: re-seeing the in-progress minute replaces it.
#[test]
fn latency_minute_replace_overwrites_instead_of_accumulating() {
    let base = LatencyMinuteBucket {
        minute: 60,
        samples: 5,
        dish: Default::default(),
        router: Default::default(),
        drop_sum: 1.0,
        drop_count: 5,
    };
    let delta = LatencyMinuteBucket {
        minute: 60,
        samples: 2,
        dish: Default::default(),
        router: Default::default(),
        drop_sum: 0.5,
        drop_count: 2,
    };
    let replaced = replace_latency_minute_bucket(Some(&base), &delta);
    assert_eq!(replaced, delta);
}

// SPEC-BUCKET-005: expired minutes fold into calendar months through a
// caller-supplied boundary.
#[test]
fn latency_months_fold_and_merge_by_supplied_calendar() {
    let minute = |minute: i64, count: u32| LatencyMinuteBucket {
        minute,
        samples: count,
        dish: Default::default(),
        router: Default::default(),
        drop_sum: count as f64 * 0.5,
        drop_count: count,
    };
    let months = fold_latency_minutes_to_months(
        &[minute(3_600, 2), minute(3_660, 3), minute(3_700_000, 4)],
        |value| value / 100_000 * 100_000,
    );
    assert_eq!(months.len(), 2);
    assert_eq!(months[&0].samples, 5);
    assert_eq!(months[&0].drop_sum, 2.5);
    assert_eq!(months[&3_700_000].samples, 4);

    let merged = add_latency_month_bucket(Some(&months[&0]), &months[&3_700_000]);
    assert_eq!(merged.samples, 9);
    assert_eq!(merged.dish.count, 0);
}

// SPEC-BUCKET-004: energy and traffic fold per minute, and additions are exact.
#[test]
fn captured_energy_minutes_match_the_typescript_fold() {
    let samples = decode_history_window(&captured_history(), 1_800_000_000_000).samples;
    let actual: Vec<_> = fold_samples_to_minutes(&samples).into_values().collect();
    assert_json_value_matches(
        &serde_json::to_value(&actual).expect("energy minutes"),
        &serde_json::from_str(ENERGY_GOLDEN).expect("golden"),
    );
}

#[test]
fn energy_buckets_add_across_drains() {
    let base = MinuteBucket {
        minute: 60,
        watt_seconds: 10.0,
        samples: 2,
        downlink_bits: 100.0,
        uplink_bits: 50.0,
    };
    let delta = MinuteBucket {
        minute: 60,
        watt_seconds: 5.0,
        samples: 1,
        downlink_bits: 20.0,
        uplink_bits: 5.0,
    };
    let sum = add_minute_bucket(Some(&base), &delta);
    assert_eq!(sum.watt_seconds, 15.0);
    assert_eq!(sum.samples, 3);
    assert_eq!(sum.downlink_bits, 120.0);
    assert_eq!(sum.uplink_bits, 55.0);
    assert_eq!(add_minute_bucket(None, &delta), delta);
}

#[test]
fn energy_fold_groups_by_minute_and_sums_traffic() {
    let samples = vec![
        TelemetrySample {
            timestamp_ms: 60_000,
            power_w: 10.0,
            downlink_bps: 100.0,
            uplink_bps: 20.0,
            ..TelemetrySample::default()
        },
        TelemetrySample {
            timestamp_ms: 61_000,
            power_w: 30.0,
            downlink_bps: 200.0,
            uplink_bps: 40.0,
            ..TelemetrySample::default()
        },
        TelemetrySample {
            timestamp_ms: 120_000,
            power_w: 50.0,
            ..TelemetrySample::default()
        },
    ];
    let minutes = fold_samples_to_minutes(&samples);
    assert_eq!(minutes.len(), 2);
    assert_eq!(minutes[&60].watt_seconds, 40.0);
    assert_eq!(minutes[&60].downlink_bits, 300.0);
    assert_eq!(minutes[&60].samples, 2);
    assert_eq!(minutes[&120].watt_seconds, 50.0);
}

// SPEC-BUCKET-005: the energy side of the same month fold.
#[test]
fn energy_months_fold_by_supplied_calendar() {
    let minute = |minute: i64| MinuteBucket {
        minute,
        watt_seconds: 60.0,
        samples: 1,
        downlink_bits: 1_000.0,
        uplink_bits: 100.0,
    };
    let months = fold_minutes_to_months(
        &[minute(3_600), minute(3_660), minute(3_700_000)],
        |value| value / 100_000 * 100_000,
    );
    assert_eq!(months.len(), 2);
    assert_eq!(months[&0].watt_seconds, 120.0);
    assert_eq!(months[&0].samples, 2);
    assert_eq!(months[&0].downlink_bits, 2_000.0);
}

// SPEC-THROUGHPUT-001: first reading seeds and returns the published average.
// SPEC-THROUGHPUT-002: an observed edge is exact traffic over 1005 ms per
// interval, never smoothed over our poll gap.
// SPEC-THROUGHPUT-003: backwards counters and long polling gaps reseed rather
// than inventing a rate.
// SPEC-THROUGHPUT-004: two unchanged refresh periods report zero, so idle does
// not hold its last rate.
// SPEC-THROUGHPUT-005: between edges the last rate holds; absent counters fall
// back.
// The captured poll sequence must reproduce the TypeScript tracker exactly.
#[test]
fn captured_throughput_steps_match_the_typescript_tracker() {
    #[derive(Deserialize)]
    struct Fixture {
        mac: String,
        steps: Vec<Step>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Step {
        at_ms: f64,
        counters: Option<ByteCounters>,
        fallback: ThroughputRates,
        result: ThroughputRates,
    }

    let fixture: Fixture = serde_json::from_str(THROUGHPUT_GOLDEN).expect("throughput fixture");
    let mut tracker = ThroughputTracker::new();
    for step in &fixture.steps {
        let actual = tracker.rates(&fixture.mac, step.counters, step.at_ms, step.fallback);
        assert_json_value_matches(
            &serde_json::to_value(actual).expect("rates"),
            &serde_json::to_value(step.result).expect("golden rates"),
        );
    }
}

#[test]
fn first_reading_seeds_and_returns_the_router_average() {
    let mut tracker = ThroughputTracker::new();
    let fallback = ThroughputRates {
        down_mbps: 1.0,
        up_mbps: 2.0,
    };
    let result = tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 1_000.0,
            tx_bytes: 500.0,
        }),
        0.0,
        fallback,
    );
    assert_eq!(result, fallback);
}

#[test]
fn counter_going_backwards_reseeds() {
    let mut tracker = ThroughputTracker::new();
    let fallback = ThroughputRates {
        down_mbps: 1.0,
        up_mbps: 2.0,
    };
    tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 1_000.0,
            tx_bytes: 500.0,
        }),
        0.0,
        fallback,
    );
    let result = tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 10.0,
            tx_bytes: 5.0,
        }),
        1_000.0,
        fallback,
    );
    assert_eq!(result, fallback);
}

#[test]
fn polling_gap_longer_than_fifteen_seconds_reseeds() {
    let mut tracker = ThroughputTracker::new();
    let fallback = ThroughputRates {
        down_mbps: 1.0,
        up_mbps: 2.0,
    };
    tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 0.0,
            tx_bytes: 0.0,
        }),
        0.0,
        fallback,
    );
    let result = tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 1_000_000.0,
            tx_bytes: 1_000_000.0,
        }),
        15_001.0,
        fallback,
    );
    assert_eq!(result, fallback);
}

#[test]
fn missing_counters_and_absent_devices_are_handled() {
    let mut tracker = ThroughputTracker::new();
    let fallback = ThroughputRates {
        down_mbps: 3.0,
        up_mbps: 4.0,
    };
    assert_eq!(tracker.rates("aa", None, 0.0, fallback), fallback);

    tracker.rates(
        "aa",
        Some(ByteCounters {
            rx_bytes: 0.0,
            tx_bytes: 0.0,
        }),
        0.0,
        fallback,
    );
    tracker.retain(["bb"]);
    // Retained away, so the next reading is a first reading again.
    assert_eq!(
        tracker.rates(
            "aa",
            Some(ByteCounters {
                rx_bytes: 0.0,
                tx_bytes: 0.0,
            }),
            1_000.0,
            fallback
        ),
        fallback
    );
}

// SPEC-TELEMETRY-009: status decodes into a typed model, including proto3's
// string spelling of non-finite floats.
#[test]
fn captured_status_decodes_into_typed_fields() {
    let schema = Schema::bundled().expect("schema");
    let response = schema
        .decode_response_json(STATUS_RESPONSE)
        .expect("status response");
    let status: DishStatus =
        serde_json::from_value(response["dishGetStatus"].clone()).expect("typed status");

    let info = status.device_info.expect("device info");
    assert_eq!(info.id.as_deref(), Some("ut0158168c-42207c02-5946ca71"));
    assert_eq!(info.bootcount, Some(U64Value(12)));
    assert_eq!(info.software_version.as_deref(), Some("2026.07.06.mr81950"));

    let obstruction = status.obstruction_stats.expect("obstruction stats");
    assert!(
        (obstruction.fraction_obstructed.0 - 0.000_610_997_96).abs() < 1e-10,
        "fraction was {}",
        obstruction.fraction_obstructed.0
    );
    assert!(
        obstruction
            .avg_prolonged_obstruction_interval_s
            .unwrap()
            .0
            .is_nan()
    );
    assert!(status.gps_stats.expect("gps").gps_valid);
    assert_eq!(status.eth_speed_mbps.0, 1_000.0);
    assert!(status.ready_states.expect("ready").rf);
    assert_eq!(
        status.device_state.expect("state").uptime_s,
        Some(U64Value(15_065))
    );
    assert!(status.alerts.is_empty());
}
