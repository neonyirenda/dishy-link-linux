//! Local data-usage summaries, checked against
//! `fixtures/telemetry/usage-summaries.json` (computed with `TZ=UTC`).

mod support;

use dishy_core::calendar::UtcCalendar;
use dishy_core::energy::MinuteBucket;
use dishy_core::usage::{UsageRange, summarize_usage};
use serde::Deserialize;
use serde_json::Value;
use support::assert_json_value_matches;

const USAGE_GOLDEN: &str = include_str!("../fixtures/telemetry/usage-summaries.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    now_ms: i64,
    minutes: Vec<MinuteBucket>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    range: UsageRange,
    summary: Value,
}

// SPEC-USAGE-001, SPEC-USAGE-002, SPEC-USAGE-003: the shared summary agrees on
// every range, including floating-point totals and null slots.
#[test]
fn summaries_match_the_typescript_implementation() {
    let fixture: Fixture = serde_json::from_str(USAGE_GOLDEN).expect("usage fixture");
    let calendar = UtcCalendar;
    assert_eq!(fixture.cases.len(), UsageRange::ALL.len());
    for case in &fixture.cases {
        let summary = summarize_usage(&fixture.minutes, case.range, fixture.now_ms, &calendar);
        assert_json_value_matches(
            &serde_json::to_value(&summary).expect("summary JSON"),
            &case.summary,
        );
    }
}
