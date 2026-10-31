//! The Alerts page's Events list and its detail: dish events and outage
//! reports, matched, titled, and worded (spec 0174).
//!
//! Port of upstream `components/alerts/OutageLog.tsx`, upstream `eventDetail.ts`, and
//! upstream `BandSwitchDetail.tsx`. Pure, so every line is testable.

use chrono::{Local, TimeZone};
use dishy_core::postmortem::{OutageReport, assign_reports};
use dishy_core::telemetry::{
    EventKind, OUTAGE_SAMPLE_RUN, OutageEvent, Severity, canonical_cause, known_event_kind,
    outage_event_kind,
};

use crate::i18n::Translator;

/// One row: a dish event, or a report no event claimed, with its report.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub event: OutageEvent,
    pub report: Option<OutageReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub note: Option<String>,
}

/// Interruptions the dish files as outages whatever their length; one shorter
/// than the recorder's own outage threshold is a blip, not lost service.
const INTERRUPTION_CAUSES: [&str; 3] = ["NO_PINGS", "NO_DOWNLINK", "UNKNOWN"];

fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

/// Dish events and unmatched reports, newest first.
pub fn event_rows(
    events: &[OutageEvent],
    reports: &[OutageReport],
    translator: &Translator,
) -> Vec<EventRow> {
    let (by_event, unclaimed) = assign_reports(reports, events);
    let mut rows: Vec<EventRow> = events
        .iter()
        .enumerate()
        .map(|(index, event)| EventRow {
            event: event.clone(),
            report: by_event.get(&index).map(|report| reports[*report].clone()),
        })
        .collect();
    rows.extend(unclaimed.into_iter().map(|index| {
        let report = &reports[index];
        EventRow {
            event: OutageEvent {
                start_ms: report.start_ms,
                duration_ms: report.duration_ms,
                cause: translator.text(&format!("eventDetail.{}", report.source)),
                severity: Severity::Critical,
                band_switch: None,
            },
            report: Some(report.clone()),
        }
    }));
    rows.sort_by_key(|row| std::cmp::Reverse(row.event.start_ms));
    rows
}

/// The kind a row's mark and detail tag show. A row the recorder timed was an
/// outage by definition; a row titled in plain words falls back to severity.
pub fn event_kind(event: &OutageEvent, report: Option<&OutageReport>) -> EventKind {
    if report.is_some() {
        return EventKind::Outage;
    }
    if INTERRUPTION_CAUSES.contains(&canonical_cause(&event.cause).as_str())
        && event.duration_ms < OUTAGE_SAMPLE_RUN as i64 * 1_000
    {
        return EventKind::Degraded;
    }
    if let Some(kind) = known_event_kind(&event.cause) {
        return kind;
    }
    if is_token(&event.cause) {
        return outage_event_kind(&event.cause);
    }
    match event.severity {
        Severity::Critical => EventKind::Outage,
        Severity::Warning => EventKind::Degraded,
        Severity::Advisory => EventKind::Info,
    }
}

pub fn event_title(event: &OutageEvent, translator: &Translator) -> String {
    if event.cause.is_empty() {
        return translator.text("events.unknown");
    }
    let token = canonical_cause(&event.cause);
    let key = format!("events.{token}.label");
    let label = translator.text(&key);
    if label != key {
        return label;
    }
    let bare = event
        .cause
        .strip_prefix("EVENT_REASON_")
        .unwrap_or(&event.cause);
    if !is_token(bare) {
        return event.cause.clone();
    }
    let words = token.replace('_', " ").to_lowercase();
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn local_time(ms: i64, format: &str) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(String::new, |at| at.format(format).to_string())
}

/// The kind as the catalog and CSS name it: outage, degraded, or info.
pub fn event_kind_name(event: &OutageEvent, report: Option<&OutageReport>) -> &'static str {
    match event_kind(event, report) {
        EventKind::Outage => "outage",
        EventKind::Degraded => "degraded",
        EventKind::Info => "info",
    }
}

/// The start as a clock time, for list rows.
pub fn event_clock(event: &OutageEvent) -> String {
    local_time(event.start_ms, "%H:%M")
}

/// "23 Aug 2026 · 05:53 to 05:54", or just the start when it ends in the
/// minute it began: times show no seconds.
pub fn event_span(event: &OutageEvent, translator: &Translator) -> String {
    let start = local_time(event.start_ms, "%-d %b %Y · %H:%M");
    let end_ms = event.start_ms + event.duration_ms.max(0);
    let end = local_time(end_ms, "%H:%M");
    if end == local_time(event.start_ms, "%H:%M") {
        return start;
    }
    translator.text_with("eventDetail.span", &[("start", &start), ("end", &end)])
}

/// Whole seconds, rounded up so a blip reads "1s", as the official app shows.
pub fn event_duration(duration_ms: i64, translator: &Translator) -> String {
    if duration_ms <= 0 {
        return String::new();
    }
    let total = ((duration_ms as f64 / 1_000.0).round() as i64).max(1);
    let text = |n: i64| n.to_string();
    if total < 60 {
        return translator.text_with("time.spanSeconds", &[("count", &text(total))]);
    }
    let (minutes, seconds) = (total / 60, total % 60);
    if minutes < 60 {
        return if seconds > 0 {
            translator.text_with(
                "time.spanMinutesSeconds",
                &[("minutes", &text(minutes)), ("seconds", &text(seconds))],
            )
        } else {
            translator.text_with("time.spanMinutes", &[("count", &text(minutes))])
        };
    }
    let (hours, rest) = (minutes / 60, minutes % 60);
    if rest > 0 {
        translator.text_with(
            "time.spanHoursMinutes",
            &[("hours", &text(hours)), ("minutes", &text(rest))],
        )
    } else {
        translator.text_with("time.spanHours", &[("count", &text(hours))])
    }
}

pub fn band_label(band: &str, translator: &Translator) -> String {
    match band {
        "RF_2GHZ" => "2.4 GHz".into(),
        "RF_5GHZ" => "5 GHz".into(),
        "RF_5GHZ_HIGH" => translator.text("network.band5GhzHigh"),
        other => other.into(),
    }
}

fn thermal_label(alert_key: &str, translator: &Translator) -> String {
    let key = format!("events.{alert_key}Episode");
    let label = translator.text(&key);
    if label == key {
        alert_key.to_owned()
    } else {
        label
    }
}

pub fn report_sections(report: &OutageReport, translator: &Translator) -> Vec<DetailSection> {
    let before = &report.before_drop;
    let not_recorded = translator.text("eventDetail.notRecorded");
    let or_not = |value: Option<String>| value.unwrap_or_else(|| not_recorded.clone());
    let throughput = |bps: f64| {
        let (value, unit) = crate::dashboard::format_throughput(bps);
        format!("{value} {unit}")
    };
    let clock = |ms: i64| local_time(ms, "%H:%M");
    vec![
        DetailSection {
            title: translator.text("eventDetail.beforeDrop"),
            rows: vec![
                (
                    translator.text("eventDetail.latency"),
                    or_not(before.latency_avg_ms.map(|ms| format!("{} ms", ms.round()))),
                ),
                (
                    translator.text("eventDetail.downlink"),
                    or_not(before.downlink_avg_bps.map(throughput)),
                ),
                (
                    translator.text("eventDetail.uplink"),
                    or_not(before.uplink_avg_bps.map(throughput)),
                ),
                (
                    translator.text("eventDetail.packetLoss"),
                    or_not(
                        before
                            .drop_rate_avg
                            .map(|rate| format!("{}%", crate::format::to_fixed(rate * 100.0, 1))),
                    ),
                ),
                (
                    translator.text("eventDetail.heater"),
                    translator.text(if before.heater_active {
                        "eventDetail.heaterOn"
                    } else {
                        "eventDetail.heaterOff"
                    }),
                ),
            ],
            note: Some(translator.text_with(
                "eventDetail.coverage",
                &[("seconds", &before.coverage_seconds.to_string())],
            )),
        },
        DetailSection {
            title: translator.text("eventDetail.thermal"),
            rows: report
                .thermal
                .iter()
                .map(|episode| {
                    let end = episode
                        .end_ms
                        .map_or_else(|| translator.text("eventDetail.stillRunning"), clock);
                    (
                        thermal_label(&episode.alert_key, translator),
                        translator.text_with(
                            "eventDetail.span",
                            &[("start", &clock(episode.start_ms)), ("end", &end)],
                        ),
                    )
                })
                .collect(),
            note: report
                .thermal
                .is_empty()
                .then(|| translator.text("eventDetail.thermalNone")),
        },
    ]
}

/// The detail as plain text, for pasting into a chat or a ticket.
pub fn report_text(event: &OutageEvent, report: &OutageReport, translator: &Translator) -> String {
    let mut lines = vec![
        translator.text_with(
            "eventDetail.textHeader",
            &[("label", &event_title(event, translator))],
        ),
        format!(
            "{} · {}",
            event_span(event, translator),
            event_duration(event.duration_ms, translator)
        ),
    ];
    for section in report_sections(report, translator) {
        lines.push(String::new());
        lines.push(section.title);
        lines.extend(
            section
                .rows
                .into_iter()
                .map(|(label, value)| format!("{label}: {value}")),
        );
        lines.extend(section.note);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use dishy_core::postmortem::{BeforeDropSource, BeforeDropStats, ReportAlertEpisode};

    use super::*;
    use crate::i18n::Locale;

    const T: i64 = 1_800_000_000_000;

    fn english() -> Translator {
        Translator::new(Locale::En)
    }

    fn event(start_ms: i64, duration_ms: i64, cause: &str) -> OutageEvent {
        OutageEvent {
            start_ms,
            duration_ms,
            cause: cause.into(),
            severity: Severity::Critical,
            band_switch: None,
        }
    }

    fn report(start_ms: i64, source: &str) -> OutageReport {
        OutageReport {
            id: format!("system:{source}:{start_ms}"),
            source: source.into(),
            start_ms,
            end_ms: start_ms + 20_000,
            duration_ms: 20_000,
            cause: Some("NO_PINGS".into()),
            before_drop: BeforeDropStats {
                window_start_ms: start_ms - 300_000,
                window_end_ms: start_ms,
                coverage_seconds: 298,
                latency_avg_ms: Some(31.6),
                downlink_avg_bps: None,
                uplink_avg_bps: Some(4_000_000.0),
                drop_rate_avg: Some(0.0125),
                heater_active: true,
                source: BeforeDropSource::Samples,
            },
            thermal: vec![ReportAlertEpisode {
                alert_key: "thermalThrottle".into(),
                start_ms,
                end_ms: None,
            }],
            generated_at_ms: start_ms + 21_000,
        }
    }

    // SPEC-OUTAGE-004, SPEC-OUTAGE-005: events and unmatched reports, newest
    // first; a matched report rides on its event; titles use the catalog.
    #[test]
    fn rows_match_reports_and_read_newest_first() {
        let translator = english();
        let events = [
            event(T, 15_000, "EVENT_REASON_NO_PINGS"),
            event(T - 600_000, 1_000, "EVENT_REASON_OBSTRUCTED"),
        ];
        let reports = [
            report(T + 1_000, "starlinkOutage"),
            report(T + 900_000, "dishUnreachable"),
        ];
        let rows = event_rows(&events, &reports, &translator);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].event.cause, "Dish unreachable",
            "unmatched report is its own row"
        );
        assert!(rows[0].report.is_some());
        assert_eq!(rows[1].event.start_ms, T);
        assert_eq!(
            rows[1].report.as_ref().map(|r| r.source.as_str()),
            Some("starlinkOutage")
        );
        assert!(rows[2].report.is_none());

        assert_eq!(
            event_title(&events[0], &translator),
            "Ping Network Interruption"
        );
        assert_eq!(
            event_title(&event(T, 0, "EVENT_REASON_SOMETHING_NEW"), &translator),
            "Something new"
        );
        assert_eq!(
            event_kind(&events[0], rows[1].report.as_ref()),
            EventKind::Outage
        );
        assert_eq!(
            event_kind(&event(T, 2_000, "EVENT_REASON_NO_PINGS"), None),
            EventKind::Degraded,
            "a blip"
        );
        assert_eq!(event_duration(15_000, &translator), "15s");
        assert_eq!(event_duration(125_000, &translator), "2m 5s");
        assert_eq!(band_label("RF_2GHZ", &translator), "2.4 GHz");
        assert!(
            event_span(&event(T, 125_000, "EVENT_REASON_NO_PINGS"), &translator).contains(" to ")
        );
        let minute_start = (T / 60_000) * 60_000;
        assert!(
            !event_span(
                &event(minute_start, 5_000, "EVENT_REASON_NO_PINGS"),
                &translator
            )
            .contains(" to ")
        );
    }

    // SPEC-OUTAGE-005: report sections and the copyable text, with "not
    // recorded" where a figure was not kept.
    #[test]
    fn report_sections_and_text_read_as_electron_does() {
        let translator = english();
        let report = report(T, "starlinkOutage");
        let sections = report_sections(&report, &translator);
        assert_eq!(sections[0].title, "Five minutes before the drop");
        assert_eq!(
            sections[0].rows[0],
            ("Latency".to_string(), "32 ms".to_string())
        );
        assert_eq!(sections[0].rows[1].1, "not recorded");
        assert_eq!(sections[0].rows[2].1, "4.0 Mbps");
        assert_eq!(sections[0].rows[3].1, "1.3%");
        assert_eq!(sections[0].rows[4].1, "Running");
        assert_eq!(
            sections[0].note.as_deref(),
            Some("298 s of the window recorded")
        );
        assert_eq!(sections[1].title, "Thermal");
        assert_eq!(sections[1].rows[0].0, "thermal throttle");
        assert!(sections[1].rows[0].1.ends_with("still running"));

        let text = report_text(
            &event(T, 20_000, "EVENT_REASON_NO_PINGS"),
            &report,
            &translator,
        );
        assert!(text.starts_with("Dishylink outage report: Ping Network Interruption\n"));
        assert!(text.contains("\nLatency: 32 ms\n"));
        assert!(text.contains("\nThermal\n"));
        let quiet = OutageReport {
            thermal: Vec::new(),
            ..report
        };
        assert_eq!(
            report_sections(&quiet, &translator)[1].note.as_deref(),
            Some("None")
        );
    }
}
