//! The dish terminal facts: the dish's own identity and health, read from the
//! status reply the recorder already polls (spec 0173).
//!
//! Port of upstream `components/dashboard/DishTerminalCard.tsx`. Pure, so every line is
//! testable from a recorded reply.

use dishy_core::telemetry::DishStatus;

use crate::i18n::Translator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub label: String,
    pub value: String,
    pub tone: Option<Tone>,
}

fn fact(label: String, value: impl Into<String>) -> Fact {
    Fact {
        label,
        value: value.into(),
        tone: None,
    }
}

fn dash(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or("—")
        .to_owned()
}

/// "FILTER_UNCONVERGED" → "Unconverged", as `formatDeviceEnum` does.
fn device_enum(value: &str, prefix: &str) -> String {
    let words = value
        .strip_prefix(prefix)
        .unwrap_or(value)
        .replace('_', " ")
        .to_lowercase();
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// "NAT_SYMMETRIC" → "symmetric"; enum words lower-cased for a fact value.
fn enum_words(value: &str, prefix: &str) -> String {
    value
        .strip_prefix(prefix)
        .unwrap_or(value)
        .replace('_', " ")
        .to_lowercase()
}

fn attitude_label(state: &str, translator: &Translator) -> String {
    match state {
        "FILTER_CONVERGED" => translator.text("alignment.state.converged"),
        "FILTER_UNCONVERGED" => translator.text("alignment.state.unconverged"),
        "FILTER_RESET" => translator.text("alignment.state.reset"),
        "FILTER_FAULTED" => translator.text("alignment.state.faulted"),
        "FILTER_INVALID" => translator.text("alignment.state.invalid"),
        other => device_enum(other, "FILTER_"),
    }
}

pub fn format_uptime(seconds: u64, translator: &Translator) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
    );
    let text = |n: u64| n.to_string();
    if days > 0 {
        translator.text_with(
            "time.uptimeDays",
            &[
                ("days", &text(days)),
                ("hours", &text(hours)),
                ("minutes", &text(minutes)),
            ],
        )
    } else if hours > 0 {
        translator.text_with(
            "time.uptimeHours",
            &[("hours", &text(hours)), ("minutes", &text(minutes))],
        )
    } else {
        translator.text_with(
            "time.uptimeMinutes",
            &[
                ("minutes", &text(minutes)),
                ("seconds", &text(seconds % 60)),
            ],
        )
    }
}

fn service_class(status: &DishStatus, translator: &Translator) -> String {
    match status.class_of_service.as_deref() {
        Some("CONSUMER") => match status.mobility_class.as_deref() {
            Some("NOMADIC" | "MOBILE") => translator.text("terminal.serviceRoam"),
            _ => translator.text("terminal.serviceResidential"),
        },
        Some("BUSINESS") => translator.text("terminal.serviceBusiness"),
        Some("BUSINESS_PLUS") => translator.text("terminal.serviceBusinessPlus"),
        other => enum_words(other.unwrap_or("—"), ""),
    }
}

fn signal(status: &DishStatus, translator: &Translator) -> Fact {
    let label = translator.text("terminal.signal");
    let (key, tone) = if status.is_snr_persistently_low {
        ("terminal.signalWeather", Tone::Warn)
    } else {
        match status.is_snr_above_noise_floor {
            Some(false) => ("terminal.signalWeak", Tone::Bad),
            Some(true) => ("terminal.signalNormal", Tone::Good),
            None => return fact(label, "—"),
        }
    };
    Fact {
        label,
        value: translator.text(key),
        tone: Some(tone),
    }
}

/// "all ready" only when every subsystem is explicitly ready: proto3 drops
/// false, so absent means not ready.
fn subsystems(status: &DishStatus, translator: &Translator) -> Fact {
    let label = translator.text("terminal.subsystems");
    let Some(states) = &status.ready_states else {
        return fact(label, "—");
    };
    let down: Vec<&str> = [
        ("SCP", states.scp),
        ("L1L2", states.l1l2),
        ("XPHY", states.xphy),
        ("AAP", states.aap),
        ("RF", states.rf),
    ]
    .into_iter()
    .filter(|(_, ready)| !ready)
    .map(|(name, _)| name)
    .collect();
    if down.is_empty() {
        Fact {
            label,
            value: translator.text("terminal.allReady"),
            tone: Some(Tone::Good),
        }
    } else {
        Fact {
            label,
            value: translator.text_with("terminal.comingUp", &[("names", &down.join(", "))]),
            tone: Some(Tone::Warn),
        }
    }
}

fn position(status: &DishStatus, translator: &Translator) -> Fact {
    let label = translator.text("terminal.position");
    let gps = status.gps_stats.as_ref().filter(|gps| gps.gps_valid);
    let state = status
        .gps_stats
        .as_ref()
        .and_then(|gps| gps.pnt_filter_convergence_state.as_deref());
    let shown_state = state.map(|state| device_enum(state, "FILTER_"));
    let value = match (gps, state) {
        (None, _) => translator.text("terminal.noFix"),
        (Some(_), Some(raw)) if shown_state.as_deref() != Some("Converged") => translator
            .text_with(
                "terminal.lockedWithState",
                &[("state", &attitude_label(raw, translator))],
            ),
        (Some(_), _) => translator.text("terminal.locked"),
    };
    let good = gps.is_some() && shown_state.as_deref() != Some("Unconverged");
    Fact {
        label,
        value,
        tone: Some(if good { Tone::Good } else { Tone::Warn }),
    }
}

/// Every fact, in the Electron order.
pub fn terminal_facts(status: &DishStatus, at_ms: i64, translator: &Translator) -> Vec<Fact> {
    let text = |key: &str| translator.text(key);
    let info = status.device_info.as_ref();
    let model = dishy_core::alignment::resolve_dish_model(
        info.and_then(|info| info.hardware_version.as_deref()),
        status.has_actuators.as_deref() == Some("HAS_ACTUATORS_YES"),
    );
    let model_name = if model == dishy_core::alignment::DishModel::Unknown {
        text("terminal.unknownModel")
    } else {
        dishy_core::alignment::spec_for_model(model).0.to_owned()
    };
    let gps_valid = status.gps_stats.as_ref().filter(|gps| gps.gps_valid);
    let alignment = status.alignment_stats.as_ref();
    let fixed = crate::format::to_fixed;
    let routers = if status.connected_routers.is_empty() {
        "0".to_owned()
    } else {
        let mut value = format!(
            "{} · {}",
            status.connected_routers.len(),
            status.connected_routers.join(", ")
        );
        if dishy_core::runner::router_presence(status, at_ms)
            == dishy_core::alert::RouterPresence::Bypassed
        {
            value.push_str(&format!(" · {}", text("terminal.bypassed")));
        }
        value
    };
    let bandwidth = match status.dl_bandwidth_restricted_reason.as_deref() {
        None | Some("NO_LIMIT" | "NO_RESTRICTION") => text("terminal.limitNone"),
        Some(reason) => enum_words(reason, ""),
    };
    let eth = status.eth_speed_mbps.0;
    vec![
        fact(text("terminal.model"), model_name),
        signal(status, translator),
        fact(
            text("terminal.hardware"),
            dash(info.and_then(|info| info.hardware_version.as_deref())),
        ),
        fact(
            text("terminal.firmware"),
            dash(info.and_then(|info| info.software_version.as_deref())),
        ),
        fact(
            text("terminal.country"),
            dash(info.and_then(|info| info.country_code.as_deref())),
        ),
        fact(
            text("terminal.uptime"),
            format_uptime(
                status
                    .device_state
                    .as_ref()
                    .and_then(|state| state.uptime_s.as_ref())
                    .map_or(0, |uptime| uptime.0),
                translator,
            ),
        ),
        fact(
            text("terminal.bootCount"),
            info.and_then(|info| info.bootcount.as_ref())
                .map_or_else(|| "—".to_owned(), |count| count.0.to_string()),
        ),
        fact(
            text("terminal.serviceClass"),
            service_class(status, translator),
        ),
        fact(
            text("terminal.gpsSatellites"),
            match gps_valid {
                Some(gps) => translator.text_with(
                    "terminal.satellitesCount",
                    &[("count", &gps.gps_sats.to_string())],
                ),
                None => text("terminal.noFix"),
            },
        ),
        position(status, translator),
        fact(
            text("terminal.ethernetLink"),
            if eth.is_finite() && eth > 0.0 {
                format!("{eth} Mbps")
            } else {
                "—".to_owned()
            },
        ),
        fact(
            text("terminal.nat"),
            enum_words(status.nat_flag.as_deref().unwrap_or("—"), "NAT_"),
        ),
        fact(text("terminal.downstreamRouters"), routers),
        fact(text("terminal.bandwidthLimit"), bandwidth),
        subsystems(status, translator),
        fact(
            text("terminal.boresight"),
            alignment.map_or_else(
                || "—".to_owned(),
                |stats| {
                    translator.text_with(
                        "terminal.boresightValue",
                        &[
                            ("az", &fixed(stats.boresight_azimuth_deg.0, 1)),
                            ("el", &fixed(stats.boresight_elevation_deg.0, 1)),
                        ],
                    )
                },
            ),
        ),
        fact(
            text("terminal.tilt"),
            alignment.map_or_else(
                || "—".to_owned(),
                |stats| format!("{}°", fixed(stats.tilt_angle_deg.0, 1)),
            ),
        ),
        fact(
            text("terminal.softwareUpdate"),
            status
                .software_update_state
                .as_deref()
                .unwrap_or("—")
                .to_lowercase(),
        ),
    ]
}

/// The software-update banner, if one is due.
pub fn update_banner(status: &DishStatus, translator: &Translator) -> Option<String> {
    if let Some(seconds) = status
        .seconds_until_swupdate_reboot_possible
        .filter(|seconds| *seconds >= 0)
    {
        return Some(translator.text_with(
            "terminal.updateReady",
            &[("duration", &format_uptime(seconds as u64, translator))],
        ));
    }
    status
        .software_update_state
        .as_deref()
        .filter(|state| !state.is_empty() && *state != "IDLE")
        .map(|state| {
            translator.text_with("terminal.updateState", &[("state", &enum_words(state, ""))])
        })
}

/// "not answering · from 5 min ago" while the dish is silent.
pub fn stale_header(last_status_age: Option<String>, translator: &Translator) -> String {
    let when = match last_status_age {
        Some(age) => translator.text_with("terminal.fromAgo", &[("when", &age)]),
        None => translator.text("terminal.lastKnown"),
    };
    format!("{} · {when}", translator.text("terminal.notAnswering"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    fn status(value: serde_json::Value) -> DishStatus {
        serde_json::from_value(value).expect("status")
    }

    fn fact<'a>(facts: &'a [Fact], label: &str) -> &'a Fact {
        facts
            .iter()
            .find(|fact| fact.label == label)
            .unwrap_or_else(|| panic!("{label}"))
    }

    fn full() -> DishStatus {
        status(serde_json::json!({
            "deviceInfo": {
                "id": "ut01000000-00000000-00abcdef",
                "hardwareVersion": "rev4_prod1",
                "softwareVersion": "2026.10.1.mr1",
                "countryCode": "NG",
                "bootcount": 42
            },
            "deviceState": { "uptimeS": 93784 },
            "classOfService": "CONSUMER",
            "mobilityClass": "NOMADIC",
            "gpsStats": { "gpsValid": true, "gpsSats": 14, "pntFilterConvergenceState": "FILTER_UNCONVERGED" },
            "ethSpeedMbps": 1000,
            "natFlag": "NAT_SYMMETRIC",
            "connectedRouters": ["Router-0100"],
            "dlBandwidthRestrictedReason": "NO_LIMIT",
            "isSnrAboveNoiseFloor": true,
            "readyStates": { "scp": true, "l1l2": true, "xphy": true, "aap": true, "rf": false },
            "alignmentStats": { "boresightAzimuthDeg": 182.44, "boresightElevationDeg": 63.06, "tiltAngleDeg": 21.25 },
            "softwareUpdateState": "IDLE"
        }))
    }

    // SPEC-TERM-001, SPEC-TERM-003: every fact, in order and in the catalog's
    // words; absent fields read as a dash.
    #[test]
    fn facts_read_in_the_electron_order_and_words() {
        let english = Translator::new(Locale::En);
        let facts = terminal_facts(&full(), 0, &english);
        let labels: Vec<&str> = facts.iter().map(|fact| fact.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Model",
                "Signal",
                "Hardware",
                "Firmware",
                "Country",
                "Uptime",
                "Boot count",
                "Service class",
                "Satellites in View (GPS)",
                "Position",
                "Ethernet link",
                "NAT",
                "Downstream routers",
                "Bandwidth limit",
                "Subsystems",
                "Boresight",
                "Tilt",
                "Software update",
            ]
        );
        assert_eq!(fact(&facts, "Model").value, "Standard 4");
        assert_eq!(fact(&facts, "Uptime").value, "1d 2h 3m");
        assert_eq!(fact(&facts, "Boot count").value, "42");
        assert_eq!(fact(&facts, "Service class").value, "roam");
        assert_eq!(
            fact(&facts, "Satellites in View (GPS)").value,
            "14 satellites"
        );
        assert_eq!(fact(&facts, "Ethernet link").value, "1000 Mbps");
        assert_eq!(fact(&facts, "NAT").value, "symmetric");
        assert_eq!(fact(&facts, "Downstream routers").value, "1 · Router-0100");
        assert_eq!(fact(&facts, "Bandwidth limit").value, "none");
        assert_eq!(fact(&facts, "Boresight").value, "az 182.4° · el 63.1°");
        assert_eq!(fact(&facts, "Tilt").value, "21.3°");
        assert_eq!(fact(&facts, "Software update").value, "idle");

        let empty = terminal_facts(&status(serde_json::json!({})), 0, &english);
        assert_eq!(fact(&empty, "Model").value, "Unknown Model");
        assert_eq!(fact(&empty, "Hardware").value, "—");
        assert_eq!(fact(&empty, "Satellites in View (GPS)").value, "no fix");
        assert_eq!(fact(&empty, "Downstream routers").value, "0");
        assert_eq!(fact(&empty, "Subsystems").value, "—");
        assert_eq!(fact(&empty, "Uptime").value, "0m 0s");
        assert_eq!(format_uptime(3_725, &english), "1h 2m");
    }

    // SPEC-TERM-002: health facts carry their tone.
    #[test]
    fn health_facts_carry_a_tone() {
        let english = Translator::new(Locale::En);
        let facts = terminal_facts(&full(), 0, &english);
        assert_eq!(fact(&facts, "Signal").value, "normal");
        assert_eq!(fact(&facts, "Signal").tone, Some(Tone::Good));
        assert_eq!(fact(&facts, "Subsystems").value, "RF coming up");
        assert_eq!(fact(&facts, "Subsystems").tone, Some(Tone::Warn));
        assert_eq!(fact(&facts, "Position").value, "locked · Unconverged");
        assert_eq!(fact(&facts, "Position").tone, Some(Tone::Warn));

        let weather = status(
            serde_json::json!({ "isSnrPersistentlyLow": true, "isSnrAboveNoiseFloor": true }),
        );
        let facts = terminal_facts(&weather, 0, &english);
        assert_eq!(fact(&facts, "Signal").tone, Some(Tone::Warn));
        assert_eq!(fact(&facts, "Signal").value, "weather affecting signal");
        let weak = status(serde_json::json!({ "isSnrAboveNoiseFloor": false }));
        assert_eq!(
            fact(&terminal_facts(&weak, 0, &english), "Signal").tone,
            Some(Tone::Bad)
        );
        let ready = status(serde_json::json!({
            "readyStates": { "scp": true, "l1l2": true, "xphy": true, "aap": true, "rf": true },
            "gpsStats": { "gpsValid": true, "gpsSats": 9, "pntFilterConvergenceState": "FILTER_CONVERGED" }
        }));
        let facts = terminal_facts(&ready, 0, &english);
        assert_eq!(fact(&facts, "Subsystems").value, "all ready");
        assert_eq!(fact(&facts, "Position").value, "locked");
        assert_eq!(fact(&facts, "Position").tone, Some(Tone::Good));
    }

    // SPEC-TERM-004, SPEC-TERM-005: the update banner and the stale header.
    #[test]
    fn update_banner_and_stale_header() {
        let english = Translator::new(Locale::En);
        assert_eq!(update_banner(&full(), &english), None);
        let reboot = status(
            serde_json::json!({ "secondsUntilSwupdateRebootPossible": 3_660, "softwareUpdateState": "REBOOT_REQUIRED" }),
        );
        assert_eq!(
            update_banner(&reboot, &english).as_deref(),
            Some("Update ready — reboot possible in 1h 1m")
        );
        let downloading =
            status(serde_json::json!({ "softwareUpdateState": "DOWNLOADING_UPDATE" }));
        assert_eq!(
            update_banner(&downloading, &english).as_deref(),
            Some("Software update: downloading update")
        );
        assert_eq!(
            stale_header(Some("5 min ago".into()), &english),
            "not answering · from 5 min ago"
        );
        assert_eq!(stale_header(None, &english), "not answering · last known");
    }
}
