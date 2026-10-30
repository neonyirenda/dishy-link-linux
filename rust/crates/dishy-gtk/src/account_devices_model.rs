//! The account page's device list: every dish and router on the service line
//! with its fields (spec 0177). Port of `components/account/deviceList.ts`
//! and the device helpers in `lib/starlinkCloud.ts`.

use std::collections::HashSet;

use serde_json::Value;

use crate::i18n::Translator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Online,
    Offline,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKindTag {
    Dish,
    Router,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountDevice {
    pub key: String,
    pub kind: DeviceKindTag,
    pub name: String,
    pub status: DeviceStatus,
    pub fields: Vec<(String, String)>,
}

/// Telemetry older than this is not live.
const FRESH_MS: i64 = 5 * 60_000;
/// A dish not connected in a month is decommissioned.
const INACTIVE_MS: i64 = 30 * 24 * 3_600_000;

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_owned())
}

fn local_time(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map_or_else(
        || "—".to_owned(),
        |at| at.format("%-d %b %Y %H:%M").to_string(),
    )
}

/// "11h 27m" / "4m 31s" / "88s", as the portal shows uptime.
fn uptime(seconds: Option<f64>, translator: &Translator) -> String {
    let Some(seconds) = seconds.filter(|seconds| *seconds > 0.0) else {
        return "—".to_owned();
    };
    let seconds = seconds as u64;
    let (days, hours, minutes, rest) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );
    let n = |value: u64| value.to_string();
    if days > 0 {
        translator.text_with(
            "time.uptimeDaysHours",
            &[("days", &n(days)), ("hours", &n(hours))],
        )
    } else if hours > 0 {
        translator.text_with(
            "time.uptimeHours",
            &[("hours", &n(hours)), ("minutes", &n(minutes))],
        )
    } else if minutes > 0 {
        translator.text_with(
            "time.uptimeMinutes",
            &[("minutes", &n(minutes)), ("seconds", &n(rest))],
        )
    } else {
        format!("{rest}s")
    }
}

fn fresh(telemetry: Option<&Value>, now_ms: i64) -> bool {
    telemetry
        .and_then(|tel| tel.get("timestampMs"))
        .and_then(Value::as_f64)
        .is_some_and(|at| (now_ms as f64) - at < FRESH_MS as f64)
}

fn tail(id: &str, length: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    chars[chars.len().saturating_sub(length)..]
        .iter()
        .collect::<String>()
        .to_uppercase()
}

pub fn router_hardware_name(hardware: Option<&str>) -> String {
    match hardware {
        Some("v3") => "Starlink Router 3".into(),
        Some("v2") => "Starlink Router (Gen 2)".into(),
        Some("v1") => "Starlink Router (Gen 1)".into(),
        Some(other) => format!("Starlink Router ({other})"),
        None => "—".into(),
    }
}

/// Every dish then its routers, inactive dishes (and their routers) last.
/// `lan_online` holds the telemetry ids answering on this LAN right now.
pub fn account_devices(
    service_line: Option<&Value>,
    telemetry: &Value,
    lan_online: &HashSet<String>,
    now_ms: i64,
    translator: &Translator,
) -> Vec<AccountDevice> {
    let Some(terminals) = service_line
        .and_then(|line| line.pointer("/content/userTerminals"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let t = |key: &str| translator.text(key);
    let status_of = |id: &str, tel: Option<&Value>, parent_inactive: bool| {
        if lan_online.contains(id) {
            DeviceStatus::Online
        } else if parent_inactive {
            DeviceStatus::Inactive
        } else if fresh(tel, now_ms) {
            DeviceStatus::Online
        } else {
            DeviceStatus::Offline
        }
    };
    let (mut active, mut inactive) = (Vec::new(), Vec::new());
    for (terminal_index, terminal) in terminals.iter().enumerate() {
        let terminal_id = text(terminal.get("userTerminalId")).unwrap_or_default();
        let dish_key = format!("ut{terminal_id}");
        let tel = telemetry.get(&dish_key);
        let last_connected = text(terminal.get("lastConnected"))
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(&date).ok())
            .map(|date| date.timestamp_millis());
        let gone = last_connected.is_some_and(|at| now_ms - at > INACTIVE_MS)
            && !lan_online.contains(&dish_key);
        let hex = tail(terminal_id.rsplit('-').next().unwrap_or_default(), 6);
        let name = if hex.is_empty() {
            text(terminal.get("serialNumber")).unwrap_or_else(|| t("account.dishFallbackName"))
        } else {
            format!("STARLINK {hex}")
        };
        let tel_text = |key: &str| text(tel.and_then(|tel| tel.get(key)));
        let bucket = if gone { &mut inactive } else { &mut active };
        bucket.push(AccountDevice {
            key: if terminal_id.is_empty() {
                format!("dish-{terminal_index}")
            } else {
                terminal_id.clone()
            },
            kind: DeviceKindTag::Dish,
            name,
            status: if gone {
                DeviceStatus::Inactive
            } else {
                status_of(&dish_key, tel, false)
            },
            fields: vec![
                (
                    t("account.starlinkId"),
                    dash(Some(terminal_id.clone()).filter(|id| !id.is_empty())),
                ),
                (
                    t("account.serialNumber"),
                    dash(text(terminal.get("dishSerialNumber"))),
                ),
                (
                    t("account.kitNumber"),
                    dash(text(terminal.get("serialNumber"))),
                ),
                (
                    t("account.softwareVersion"),
                    dash(tel_text("softwareVersion")),
                ),
                (
                    t("terminal.uptime"),
                    uptime(
                        tel.and_then(|tel| tel.get("uptimeS"))
                            .and_then(Value::as_f64),
                        translator,
                    ),
                ),
                (
                    t("account.lastUpdated"),
                    tel.and_then(|tel| tel.get("timestampMs"))
                        .and_then(Value::as_f64)
                        .map_or_else(|| "—".into(), |at| local_time(at as i64)),
                ),
                (
                    t("account.timeObstructed"),
                    tel.and_then(|tel| tel.get("obstructionPct"))
                        .and_then(Value::as_f64)
                        .map_or_else(
                            || "—".into(),
                            |pct| format!("{}%", crate::format::to_fixed(pct * 100.0, 2)),
                        ),
                ),
                (
                    t("account.lastConnected"),
                    last_connected.map_or_else(|| "—".into(), local_time),
                ),
            ],
        });
        for (router_index, router) in terminal
            .get("routers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let router_id = text(router.get("routerId")).unwrap_or_default();
            let router_key = format!("Router-{router_id}");
            let rtel = telemetry.get(&router_key);
            let number = |key: &str| rtel.and_then(|tel| tel.get(key)).and_then(Value::as_f64);
            let hops = number("hops").unwrap_or(0.0) as u64;
            let mesh = rtel
                .and_then(|tel| tel.get("isRepeater"))
                .and_then(Value::as_bool)
                == Some(true)
                || hops > 0;
            let prefix = match (rtel.is_some(), mesh) {
                (false, _) => t("account.routerGeneric"),
                (true, true) => t("account.routerMesh"),
                (true, false) => t("account.routerMain"),
            };
            let suffix = tail(&router_id, 12).trim_start_matches('0').to_owned();
            let connection = if hops == 0 {
                t("account.connectionDirect")
            } else {
                let plural = if hops == 1 { "one" } else { "other" };
                translator.text_with(
                    &format!("account.connectionMesh_{plural}"),
                    &[("count", &hops.to_string())],
                )
            };
            let bucket = if gone { &mut inactive } else { &mut active };
            bucket.push(AccountDevice {
                key: if router_id.is_empty() {
                    format!("router-{terminal_index}-{router_index}")
                } else {
                    router_id.clone()
                },
                kind: DeviceKindTag::Router,
                name: if suffix.is_empty() {
                    prefix
                } else {
                    format!("{prefix} {suffix}")
                },
                status: status_of(&router_key, rtel, gone),
                fields: vec![
                    (
                        t("account.routerId"),
                        dash(Some(router_id.clone()).filter(|id| !id.is_empty())),
                    ),
                    (
                        t("account.hardwareVersion"),
                        router_hardware_name(
                            text(rtel.and_then(|tel| tel.get("hardwareVersion"))).as_deref(),
                        ),
                    ),
                    (
                        t("account.softwareVersion"),
                        dash(text(rtel.and_then(|tel| tel.get("softwareVersion")))),
                    ),
                    (
                        t("account.clients"),
                        number("clients")
                            .map_or_else(|| "—".into(), |clients| format!("{}", clients as u64)),
                    ),
                    (t("terminal.uptime"), uptime(number("uptimeS"), translator)),
                    (t("account.connectionToStarlink"), connection),
                    (
                        t("account.bypassed"),
                        match rtel {
                            Some(tel) => t(
                                if tel.get("isBypassed").and_then(Value::as_bool) == Some(true) {
                                    "common.yes"
                                } else {
                                    "common.no"
                                },
                            ),
                            None => "—".into(),
                        },
                    ),
                    (
                        t("account.lastUpdated"),
                        number("timestampMs")
                            .map_or_else(|| "—".into(), |at| local_time(at as i64)),
                    ),
                ],
            });
        }
    }
    active.extend(inactive);
    active
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::i18n::Locale;

    const NOW: i64 = 1_800_000_000_000;

    fn line() -> Value {
        json!({ "content": { "userTerminals": [
            {
                "userTerminalId": "01000000-00000000-004c8bb9",
                "serialNumber": "KIT12345",
                "dishSerialNumber": "4PB00000001",
                "lastConnected": "2027-01-15T07:00:00Z",
                "routers": [{ "routerId": "010000000000000000ab12cd" }, { "routerId": "010000000000000000ffff01" }]
            },
            {
                "userTerminalId": "01000000-00000000-00aaaaaa",
                "lastConnected": "2025-01-01T00:00:00Z",
                "routers": [{ "routerId": "0100000000000000000000ee" }]
            }
        ]}})
    }

    fn telemetry() -> Value {
        json!({
            "ut01000000-00000000-004c8bb9": { "kind": "dish", "timestampMs": NOW - 60_000, "softwareVersion": "2027.01.1", "uptimeS": 41_220, "obstructionPct": 0.0123 },
            "Router-010000000000000000ab12cd": { "kind": "router", "timestampMs": NOW - 30_000, "hardwareVersion": "v3", "softwareVersion": "2027.01.2", "uptimeS": 300, "clients": 7, "hops": 0, "isBypassed": false },
            "Router-010000000000000000ffff01": { "kind": "router", "timestampMs": NOW - 600_000, "hardwareVersion": "v2", "hops": 2, "isRepeater": true }
        })
    }

    fn field<'a>(device: &'a AccountDevice, label: &str) -> &'a str {
        device
            .fields
            .iter()
            .find(|(name, _)| name == label)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("{label}"))
    }

    // SPEC-ACCTDEV-001, SPEC-ACCTDEV-002: names, statuses, and grouping.
    #[test]
    fn devices_are_named_grouped_and_rated() {
        let english = Translator::new(Locale::En);
        let devices = account_devices(Some(&line()), &telemetry(), &HashSet::new(), NOW, &english);
        let names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "STARLINK 4C8BB9",
                "Main Router AB12CD",
                "MESH FFFF01",
                "STARLINK AAAAAA",
                "Router EE"
            ]
        );
        let statuses: Vec<DeviceStatus> = devices.iter().map(|device| device.status).collect();
        assert_eq!(
            statuses,
            [
                DeviceStatus::Online,
                DeviceStatus::Online,
                DeviceStatus::Offline,
                DeviceStatus::Inactive,
                DeviceStatus::Inactive
            ]
        );

        let lan: HashSet<String> = ["Router-010000000000000000ffff01".to_string()].into();
        let devices = account_devices(Some(&line()), &telemetry(), &lan, NOW, &english);
        assert_eq!(
            devices[2].status,
            DeviceStatus::Online,
            "answering on the LAN"
        );
        assert!(account_devices(None, &json!({}), &HashSet::new(), NOW, &english).is_empty());
    }

    // SPEC-ACCTDEV-003, SPEC-ACCTDEV-004: each kind's fields, with dashes for
    // what is missing.
    #[test]
    fn each_kind_shows_its_fields() {
        let english = Translator::new(Locale::En);
        let devices = account_devices(Some(&line()), &telemetry(), &HashSet::new(), NOW, &english);
        let dish = &devices[0];
        assert_eq!(field(dish, "Starlink ID"), "01000000-00000000-004c8bb9");
        assert_eq!(field(dish, "Kit number"), "KIT12345");
        assert_eq!(field(dish, "Software version"), "2027.01.1");
        assert_eq!(field(dish, "Uptime"), "11h 27m");
        assert_eq!(field(dish, "Time obstructed"), "1.23%");
        let router = &devices[1];
        assert_eq!(field(router, "Hardware version"), "Starlink Router 3");
        assert_eq!(field(router, "Clients"), "7");
        assert_eq!(field(router, "Connection to Starlink"), "Direct");
        assert_eq!(field(router, "Bypassed"), "No");
        assert_eq!(
            field(&devices[2], "Connection to Starlink"),
            "Mesh (2 hops)"
        );
        assert_eq!(field(&devices[4], "Bypassed"), "—");
        assert_eq!(router_hardware_name(Some("v9")), "Starlink Router (v9)");
        assert_eq!(router_hardware_name(None), "—");
    }
}
