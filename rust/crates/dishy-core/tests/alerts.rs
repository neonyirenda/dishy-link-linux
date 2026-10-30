use std::collections::BTreeMap;

use dishy_core::alert::{
    AlertEngine, AlertSource, AlertStore, AlertTransitionKind, RouterPresence, Severity,
};
use dishy_core::meter::ClientMonthlyLimit;
use dishy_core::odometer::ClientTotal;
use tempfile::tempdir;

const NOW: i64 = 1_800_000_000_000;

fn alerts(entries: &[(&str, bool)]) -> BTreeMap<String, bool> {
    entries
        .iter()
        .map(|(key, active)| ((*key).to_owned(), *active))
        .collect()
}

fn client_total(client_id: i64, rx_bytes: u64, tx_bytes: u64, name: &str) -> ClientTotal {
    ClientTotal {
        client_id: Some(client_id),
        mac_address: "10:00:20:11:22:33".into(),
        name: Some(name.into()),
        ip_address: None,
        rx_bytes,
        tx_bytes,
        since_ms: NOW,
        last_seen_ms: NOW,
        months: None,
    }
}

// SPEC-ALERT-001: known flags produce source-specific transitions only on edges.
#[test]
fn alert_flags_emit_firing_and_cleared_edges() {
    let mut engine = AlertEngine::default();
    let first = engine.observe_dish(Some(&alerts(&[("thermalShutdown", true)])), None, None, NOW);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].source, AlertSource::Dish);
    assert_eq!(first[0].key, "thermalShutdown");
    assert_eq!(first[0].kind, AlertTransitionKind::Fired);
    assert_eq!(first[0].severity, Severity::Critical);

    assert!(
        engine
            .observe_dish(
                Some(&alerts(&[("thermalShutdown", true)])),
                None,
                None,
                NOW + 5_000
            )
            .is_empty()
    );
    let cleared = engine.observe_dish(Some(&BTreeMap::new()), None, None, NOW + 10_000);
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0].kind, AlertTransitionKind::Cleared);
    assert_eq!(cleared[0].at_ms, NOW + 10_000);
}

// SPEC-ALERT-001: the same firmware key from two devices is separate state.
#[test]
fn device_alerts_with_overlapping_keys_remain_independent() {
    let mut engine = AlertEngine::default();
    engine.observe_dish(Some(&alerts(&[("thermalThrottle", true)])), None, None, NOW);
    engine.observe_router(Some(&alerts(&[("thermalThrottle", true)])), NOW + 1);
    assert_eq!(
        engine
            .active_alerts()
            .iter()
            .filter(|alert| alert.key == "thermalThrottle")
            .count(),
        2
    );
    engine.observe_dish(Some(&BTreeMap::new()), None, None, NOW + 2);
    assert_eq!(
        engine
            .active_alerts()
            .iter()
            .filter(|alert| alert.key == "thermalThrottle")
            .map(|alert| alert.source)
            .collect::<Vec<_>>(),
        vec![AlertSource::Router]
    );
}

// SPEC-ALERT-002: a failed read marks silence without fabricating an all-clear.
#[test]
fn missing_device_replies_hold_checks_and_recovery_resolves_unreachability() {
    let mut engine = AlertEngine::default();
    engine.observe_dish(
        Some(&alerts(&[("dishWaterDetected", true)])),
        None,
        None,
        NOW,
    );
    let unreachable = engine.observe_dish(None, None, None, NOW + 5_000);
    assert!(unreachable.iter().any(|transition| {
        transition.key == "dishUnreachable" && transition.kind == AlertTransitionKind::Fired
    }));
    assert!(
        engine
            .active_alerts()
            .iter()
            .any(|alert| alert.key == "dishWaterDetected")
    );

    let recovered = engine.observe_dish(Some(&BTreeMap::new()), None, None, NOW + 10_000);
    assert!(recovered.iter().any(|transition| {
        transition.key == "dishUnreachable" && transition.kind == AlertTransitionKind::Cleared
    }));
    assert!(engine.active_alerts().is_empty());
}

// SPEC-ALERT-002: expected router silence in bypass/absent states is not a fault.
#[test]
fn bypassed_and_absent_routers_do_not_raise_unreachable() {
    for presence in [RouterPresence::Bypassed, RouterPresence::Absent] {
        let mut engine = AlertEngine::default();
        engine.observe_dish(Some(&BTreeMap::new()), None, Some(presence), NOW);
        let transitions = engine.observe_router(None, NOW + 5_000);
        assert!(
            transitions
                .iter()
                .all(|transition| transition.key != "routerUnreachable")
        );
    }
}

// SPEC-ALERT-002: no status poll is not a failed status poll.
#[test]
fn an_unobserved_device_is_not_marked_unreachable() {
    let engine = AlertEngine::default();
    assert!(engine.active_alerts().is_empty());
}

// SPEC-ALERT-003: episodes survive restart, close on a later edge, and expire.
#[test]
fn alert_episode_store_restores_open_alerts_and_expires_closed_history() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("alerts.ndjson");
    let mut store = AlertStore::open(&path).unwrap();
    let mut engine = AlertEngine::default();
    let transition =
        engine.observe_dish(Some(&alerts(&[("thermalShutdown", true)])), None, None, NOW);
    store.record(&transition, NOW).unwrap();

    let open = store.open_episodes();
    assert_eq!(open.len(), 1);
    let mut restored = AlertEngine::new(&open);
    assert!(
        restored
            .observe_dish(
                Some(&alerts(&[("thermalShutdown", true)])),
                None,
                None,
                NOW + 5_000
            )
            .is_empty()
    );

    let clear = restored.observe_dish(Some(&BTreeMap::new()), None, None, NOW + 10_000);
    store.record(&clear, NOW + 10_000).unwrap();
    assert_eq!(store.all(NOW + 10_000)[0].end_ms, Some(NOW + 10_000));
    assert!(store.all(NOW + 48 * 60 * 60 * 1_000 + 10_001).is_empty());
}

// SPEC-ALERT-003: history is newest-first and open episodes are retained.
#[test]
fn alert_history_is_newest_first_and_keeps_open_episodes() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("alerts.ndjson");
    let mut store = AlertStore::open(&path).unwrap();
    let mut engine = AlertEngine::default();
    let older = engine.observe_dish(
        Some(&alerts(&[("thermalShutdown", true)])),
        None,
        None,
        NOW - 2_000,
    );
    store.record(&older, NOW).unwrap();
    let newer = engine.observe_router(Some(&alerts(&[("poeFuseBlown", true)])), NOW - 1_000);
    store.record(&newer, NOW).unwrap();
    let history = store.all(NOW);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].key, "poeFuseBlown");
    assert_eq!(history[1].key, "thermalShutdown");
}

// SPEC-METER-ALERT-001, SPEC-METER-ALERT-002: monthly cap episodes open and
// close only on observed threshold edges; a missing total is not zero usage.
#[test]
fn monthly_limit_alerts_open_once_and_clear_on_below_cap_or_cap_removal() {
    let limit = ClientMonthlyLimit::new("42", 100).unwrap();
    let mut engine = AlertEngine::default();

    let fired = engine.observe_monthly_limits(
        std::slice::from_ref(&limit),
        &[client_total(42, 60, 40, "Phone")],
        NOW,
    );
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].source, AlertSource::System);
    assert_eq!(fired[0].key, "monthlyDataLimit:42");
    assert_eq!(fired[0].severity, Severity::Warning);
    assert_eq!(fired[0].kind, AlertTransitionKind::Fired);
    assert_eq!(fired[0].label.as_deref(), Some("Phone"));
    assert!(fired[0].is_notifiable());

    assert!(
        engine
            .observe_monthly_limits(
                std::slice::from_ref(&limit),
                &[client_total(42, 70, 30, "Phone")],
                NOW + 1,
            )
            .is_empty(),
        "repeated reached observations are not new transitions"
    );
    assert!(
        engine
            .observe_monthly_limits(std::slice::from_ref(&limit), &[], NOW + 2)
            .is_empty(),
        "a missing total is not an observed zero"
    );
    assert!(
        engine
            .active_alerts()
            .iter()
            .any(|alert| alert.key == "monthlyDataLimit:42")
    );

    let cleared = engine.observe_monthly_limits(
        std::slice::from_ref(&limit),
        &[client_total(42, 40, 50, "Phone")],
        NOW + 3,
    );
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0].kind, AlertTransitionKind::Cleared);
    assert_eq!(cleared[0].label.as_deref(), Some("Phone"));

    engine.observe_monthly_limits(
        std::slice::from_ref(&limit),
        &[client_total(42, 100, 0, "Phone")],
        NOW + 4,
    );
    let removed = engine.observe_monthly_limits(&[], &[], NOW + 5);
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].kind, AlertTransitionKind::Cleared);
    assert_eq!(removed[0].label.as_deref(), Some("Phone"));
}

// SPEC-METER-ALERT-003: device labels accompany durable episodes and restore
// into active state after a process restart.
#[test]
fn monthly_limit_alert_label_survives_alert_store_restart() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("alerts.ndjson");
    let mut store = AlertStore::open(&path).unwrap();
    let mut engine = AlertEngine::default();
    let transitions = engine.observe_monthly_limits(
        &[ClientMonthlyLimit::new("42", 100).unwrap()],
        &[client_total(42, 120, 0, "Office laptop")],
        NOW,
    );
    store.record(&transitions, NOW).unwrap();

    let open = store.open_episodes();
    assert_eq!(open[0].label.as_deref(), Some("Office laptop"));
    let mut restored = AlertEngine::new(&open);
    let active = restored
        .active_alerts()
        .into_iter()
        .find(|alert| alert.key == "monthlyDataLimit:42")
        .expect("monthly-limit alert restored");
    assert_eq!(active.label.as_deref(), Some("Office laptop"));

    let clear = restored.observe_monthly_limits(&[], &[], NOW + 1);
    store.record(&clear, NOW + 1).unwrap();
    let closed = store
        .all(NOW + 1)
        .into_iter()
        .find(|episode| episode.key == "monthlyDataLimit:42")
        .expect("closed episode retained");
    assert_eq!(closed.label.as_deref(), Some("Office laptop"));
    assert_eq!(closed.end_ms, Some(NOW + 1));
}

// SPEC-METER-ALERT-003: alert rows written before labels were added still load.
#[test]
fn alert_episodes_without_a_label_remain_readable() {
    let episode: dishy_core::alert::AlertEpisode = serde_json::from_value(serde_json::json!({
        "source": "system",
        "key": "dishUnreachable",
        "severity": "critical",
        "startMs": NOW,
        "endMs": null
    }))
    .unwrap();
    assert_eq!(episode.label, None);
}

// SPEC-RULE-009: a rule other than a plain monthly limit announces through
// `dataLimit:<subject>`; it opens when reached and closes when the
// announcement retires, and a plain limit raises nothing here.
#[test]
fn rule_announcements_open_and_retire_under_their_subject() {
    use dishy_core::data_meter::{
        METER_ALERT_HOLD_MS, MeterCycle, MeterReading, MeterRuleTerms, create_rule, evaluate_meters,
    };

    let read = |rx: u64| {
        vec![MeterReading {
            client_key: "42".into(),
            lifetime_rx: rx,
            lifetime_tx: 0,
        }]
    };
    let daily = create_rule(MeterRuleTerms::new("42", 100, MeterCycle::Daily, 0, 0, NOW));
    let mut engine = AlertEngine::default();
    let label = |_: &dishy_core::data_meter::MeterRule| Some("Phone".to_string());

    let (rules, reached) = evaluate_meters(&[daily], &read(150), NOW + 1);
    let fired = engine.observe_meter_transitions(&reached, label, NOW + 1);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].key, "dataLimit:42");
    assert_eq!(fired[0].kind, AlertTransitionKind::Fired);
    assert_eq!(fired[0].label.as_deref(), Some("Phone"));
    assert!(fired[0].is_notifiable(), "a reached rule notifies");

    let (_, expired) = evaluate_meters(&rules, &read(150), NOW + 1 + METER_ALERT_HOLD_MS);
    let cleared = engine.observe_meter_transitions(&expired, label, NOW + 1 + METER_ALERT_HOLD_MS);
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0].kind, AlertTransitionKind::Cleared);
    assert!(
        !cleared[0].is_notifiable(),
        "retiring on a timer is not news"
    );

    let limit = create_rule(MeterRuleTerms::new(
        "42",
        100,
        MeterCycle::Monthly { day: 1 },
        0,
        0,
        NOW,
    ));
    let (_, reached) = evaluate_meters(&[limit], &read(150), NOW + 1);
    assert!(
        engine
            .observe_meter_transitions(&reached, label, NOW + 1)
            .is_empty()
    );
}

// SPEC-OUTAGE-001: starlinkOutage is active while the newest eight samples all
// dropped every ping; it is critical and notifies.
#[test]
fn starlink_outage_follows_the_dropped_sample_run() {
    use dishy_core::telemetry::TelemetrySample;
    let sample = |offset: i64, drop_rate: f64| TelemetrySample {
        timestamp_ms: NOW + offset * 1_000,
        latency_ms: None,
        drop_rate,
        downlink_bps: 0.0,
        uplink_bps: 0.0,
        power_w: 40.0,
        router_latency_ms: None,
        router_ping_success_percent: None,
    };
    let mut engine = AlertEngine::default();
    let seven: Vec<_> = (0..7).map(|offset| sample(offset, 1.0)).collect();
    assert!(engine.observe_link(&seven, NOW).is_empty());
    let eight: Vec<_> = (0..8).map(|offset| sample(offset, 1.0)).collect();
    let fired = engine.observe_link(&eight, NOW + 8_000);
    assert_eq!(fired.len(), 1);
    assert_eq!(
        (fired[0].key.as_str(), fired[0].severity),
        ("starlinkOutage", Severity::Critical)
    );
    assert!(fired[0].is_notifiable());
    let mut recovered = eight.clone();
    recovered.push(sample(8, 0.0));
    let cleared = engine.observe_link(&recovered, NOW + 9_000);
    assert_eq!(cleared[0].kind, AlertTransitionKind::Cleared);
    assert!(dishy_core::alert::known_status_keys(AlertSource::System).contains(&"starlinkOutage"));
}

// SPEC-OUTAGE-002, SPEC-OUTAGE-003: closing a dishUnreachable episode writes
// one report from what the recorder holds, keyed to the episode's start.
#[test]
fn closing_an_outage_episode_writes_one_report() {
    use dishy_core::calendar::UtcCalendar;
    use dishy_core::recorder::Recorder;

    let directory = tempdir().unwrap();
    let mut recorder = Recorder::open_with(directory.path(), NOW, Box::new(UtcCalendar)).unwrap();
    let mut engine = AlertEngine::default();
    let fired = engine.observe_dish(None, None, None, NOW);
    recorder.alerts_mut().record(&fired, NOW).unwrap();
    let cleared = engine.observe_dish(Some(&BTreeMap::new()), None, None, NOW + 40_000);
    assert_eq!(
        recorder
            .write_outage_reports(&cleared, NOW + 40_000)
            .unwrap(),
        1
    );
    recorder
        .alerts_mut()
        .record(&cleared, NOW + 40_000)
        .unwrap();
    let reports = recorder.outage_reports(NOW + 41_000);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, format!("system:dishUnreachable:{NOW}"));
    assert_eq!(reports[0].duration_ms, 40_000);
    assert_eq!(
        recorder
            .write_outage_reports(&cleared, NOW + 40_000)
            .unwrap(),
        0,
        "already closed"
    );
}
