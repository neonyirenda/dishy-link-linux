use chrono::{TimeZone, Utc};
use dishy_core::calendar::UtcCalendar;
use dishy_core::meter::{ClientMonthlyLimit, limit_progress};
use dishy_core::odometer::{ClientReading, ClientTotalsCore};
use dishy_core::recorder::Recorder;
use tempfile::tempdir;

// SPEC-METER-001: limits are keyed by stable client id and must be positive.
#[test]
fn limits_reject_empty_keys_and_zero_bytes() {
    assert!(ClientMonthlyLimit::new("client-7", 1).is_ok());
    assert!(ClientMonthlyLimit::new("client-7", 0).is_err());
    assert!(ClientMonthlyLimit::new("", 1).is_err());
}

// SPEC-METER-002: progress is measured from this month's total including both
// directions, and reaching the exact cap is already reached.
#[test]
fn progress_uses_current_month_bytes_and_reaches_at_the_cap() {
    let limit = ClientMonthlyLimit::new("client-7", 100).expect("valid limit");

    let under = limit_progress(&limit, 30, 69);
    assert_eq!(under.used_bytes, 99);
    assert!(!under.reached);

    let exact = limit_progress(&limit, 60, 40);
    assert_eq!(exact.used_bytes, 100);
    assert!(exact.reached);

    let over = limit_progress(&limit, u64::MAX, 1);
    assert_eq!(over.used_bytes, u64::MAX);
    assert!(over.reached);
}

// SPEC-METER-002: the limit follows the odometer's calendar-month roll.
#[test]
fn monthly_progress_restarts_when_the_odometer_enters_a_new_month() {
    let mut odometer = ClientTotalsCore::with_options(15_000, true);
    let key = "7";
    let mac = "10:00:20:11:22:33";
    let live_keys = odometer.note_poll(&[(Some(7), mac)]);
    let before_roll = Utc
        .with_ymd_and_hms(2026, 6, 30, 23, 59, 57)
        .single()
        .expect("valid timestamp")
        .timestamp_millis();
    odometer.observe(
        ClientReading {
            client_id: Some(7),
            mac_address: mac,
            rx_bytes: 0,
            tx_bytes: 0,
            at_ms: before_roll,
            name: Some("Phone"),
            ip_address: None,
        },
        &live_keys,
    );
    odometer.observe(
        ClientReading {
            client_id: Some(7),
            mac_address: mac,
            rx_bytes: 10,
            tx_bytes: 10,
            at_ms: before_roll + 1_000,
            name: Some("Phone"),
            ip_address: None,
        },
        &live_keys,
    );
    let limit = ClientMonthlyLimit::new(key, 20).expect("valid limit");
    let current = odometer.totals(Some(key)).pop().expect("current total");
    assert!(limit_progress(&limit, current.rx_bytes, current.tx_bytes).reached);

    let next_month = Utc
        .with_ymd_and_hms(2026, 7, 1, 0, 0, 1)
        .single()
        .expect("valid timestamp")
        .timestamp_millis();
    odometer.observe(
        ClientReading {
            client_id: Some(7),
            mac_address: mac,
            rx_bytes: 0,
            tx_bytes: 0,
            at_ms: next_month,
            name: Some("Phone"),
            ip_address: None,
        },
        &live_keys,
    );
    let current = odometer.totals(Some(key)).pop().expect("rolled total");
    let progress = limit_progress(&limit, current.rx_bytes, current.tx_bytes);
    assert_eq!(progress.used_bytes, 0);
    assert!(!progress.reached);
}

// SPEC-METER-003 / SPEC-RULE-007: the recorder owns the odometer and the rules,
// removes caps with device history, and keeps them across a restart.
#[test]
fn recorder_removes_limits_with_device_history_and_clears_them_together() {
    let directory = tempdir().expect("temporary directory");
    let mut recorder =
        Recorder::open_with(directory.path(), 1_800_000_000_000, Box::new(UtcCalendar))
            .expect("open recorder");
    recorder
        .set_client_monthly_limit(ClientMonthlyLimit::new("client-7", 5_000).expect("valid limit"))
        .expect("save limit");
    assert_eq!(recorder.client_monthly_limits().len(), 1);

    assert!(
        recorder
            .remove_client_total("client-7")
            .expect("remove device record")
    );
    assert!(recorder.client_monthly_limits().is_empty());

    recorder
        .set_client_monthly_limit(ClientMonthlyLimit::new("client-8", 8_000).expect("valid limit"))
        .expect("save limit");
    drop(recorder);
    let mut recorder =
        Recorder::open_with(directory.path(), 1_800_000_000_000, Box::new(UtcCalendar))
            .expect("reopen recorder");
    assert_eq!(recorder.client_monthly_limits().len(), 1);
    recorder
        .clear_client_totals()
        .expect("clear device records");
    assert!(recorder.client_monthly_limits().is_empty());
}

// SPEC-RULE-002: a device rule written from the GTK side (which knows no
// counters) starts at the device's current lifetime counter, not at zero.
#[test]
fn a_new_device_rule_measures_from_the_current_counter() {
    use dishy_core::data_meter::{MeterCycle, MeterRuleTerms, usage_bytes};

    let directory = tempdir().expect("temporary directory");
    let at_ms = 1_800_000_000_000;
    let mut recorder =
        Recorder::open_with(directory.path(), at_ms, Box::new(UtcCalendar)).expect("open recorder");
    let mac = "10:00:20:11:22:55";
    let core = recorder.odometer_mut().core_mut();
    let live_keys = core.note_poll(&[(Some(44), mac)]);
    for (offset, rx_bytes) in [(0, 0), (1_000, 5_000)] {
        core.observe(
            ClientReading {
                client_id: Some(44),
                mac_address: mac,
                rx_bytes,
                tx_bytes: 0,
                at_ms: at_ms + offset,
                name: None,
                ip_address: None,
            },
            &live_keys,
        );
    }
    recorder
        .upsert_device_rule(MeterRuleTerms::new(
            "44",
            1_000_000,
            MeterCycle::Daily,
            0,
            0,
            at_ms + 2_000,
        ))
        .expect("save rule");
    let rule = &recorder.meters().rules()[0];
    assert_eq!(rule.anchor_rx, 5_000);
    assert_eq!(usage_bytes(rule), 0);
}
