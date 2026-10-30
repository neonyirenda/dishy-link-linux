//! What the device and node details show (spec 0175).
//!
//! Port of `components/network/deviceFacts.ts`, `DeviceThroughput.tsx`, and
//! `NodeDetail.tsx`. Pure, so every fact is testable. Reads only the roster
//! snapshot and the recorder's throughput window.

use dishy_core::client_window::ClientSample;
use dishy_core::network::{NetworkDevice, NetworkNode};

use crate::dashboard::{ChartPoint, ChartSeries, DOWN_COLOR, UP_COLOR};
use crate::i18n::Translator;

/// Per-device readings arrive at the roster cadence; only a stretch past a
/// couple of minutes is a real hole.
pub const PER_DEVICE_GAP_MS: i64 = 150_000;
pub const DEVICE_WINDOWS: [u32; 3] = [15, 60, 360];

/// The device's facts, in the Electron order; a fact the router did not send
/// is left out. `month` is this month's (rx, tx) from the odometer.
pub fn device_facts(
    device: &NetworkDevice,
    upstream_name: Option<&str>,
    vendor: Option<&str>,
    month: Option<(u64, u64)>,
    translator: &Translator,
) -> Vec<(String, String)> {
    use crate::format::format_bytes;
    use crate::network_view::{idle_text, signal_text};
    let text = |key: &str| translator.text(key);
    let whole = |value: f64| format!("{}", value.round());
    let mut facts = vec![(
        text("network.factStatus"),
        idle_text(device, translator).unwrap_or_else(|| text("network.factActive")),
    )];
    if let Some(role) = &device.role {
        facts.push((text("network.role"), role.clone()));
    }
    if let Some(upstream) = upstream_name {
        facts.push((text("network.connectedTo"), upstream.to_owned()));
    }
    facts.push((
        text("network.manufacturer"),
        vendor.map_or_else(|| text("common.unknownCapitalised"), str::to_owned),
    ));
    facts.push((text("network.connection"), device.band.clone()));
    if let Some(signal) = signal_text(device, translator) {
        facts.push((text("terminal.signal"), signal));
    }
    if let Some(snr) = device.snr.filter(|snr| *snr > 0.0) {
        facts.push((text("network.snr"), format!("{} dB", whole(snr))));
    }
    let link = &device.link;
    if let Some(width) = link.channel_width_mhz {
        facts.push((text("network.bandwidth"), format!("{} MHz", whole(width))));
    }
    if let Some(mcs) = link.mcs {
        facts.push((text("network.mcs"), whole(mcs)));
    }
    if let Some(nss) = link.nss {
        facts.push((text("network.nss"), whole(nss)));
    }
    if let Some(rate) = link.rx_rate_mbps {
        facts.push((text("network.rxRate"), format!("{} Mbps", whole(rate))));
    }
    if let Some(rate) = link.tx_rate_mbps {
        facts.push((text("network.txRate"), format!("{} Mbps", whole(rate))));
    }
    if let Some(ip) = &device.ip_address {
        facts.push(("IPv4".into(), ip.clone()));
    }
    if let Some(ip) = device.ipv6_addresses.first() {
        facts.push(("IPv6".into(), ip.clone()));
    }
    if !device.mac_address.is_empty() {
        facts.push((text("network.macAddress"), device.mac_address.clone()));
    }
    let usage = |rx: u64, tx: u64| format!("{} ↓ / {} ↑", format_bytes(rx), format_bytes(tx));
    match month.filter(|(rx, tx)| *rx > 0 || *tx > 0) {
        Some((rx, tx)) => facts.push((text("network.dataUsedMonth"), usage(rx, tx))),
        None => {
            let (rx, tx) = (device.rx_bytes.unwrap_or(0), device.tx_bytes.unwrap_or(0));
            if rx > 0 || tx > 0 {
                facts.push((text("network.dataUsedConnection"), usage(rx, tx)));
            }
        }
    }
    facts
}

/// Down and up lines for one device over the window: one point per reading,
/// with a hole wherever readings are more than 150 s apart.
pub fn throughput_series(
    samples: &[ClientSample],
    window_minutes: u32,
    now_ms: i64,
    translator: &Translator,
) -> Vec<ChartSeries> {
    let start_ms = now_ms - i64::from(window_minutes) * 60_000;
    let in_window: Vec<&ClientSample> = samples
        .iter()
        .filter(|sample| (start_ms..=now_ms).contains(&sample.at_ms))
        .collect();
    let line = |id, key: &str, color, value: fn(&ClientSample) -> f64| ChartSeries {
        id,
        label: translator.text(key),
        color,
        points: in_window
            .iter()
            .map(|sample| ChartPoint {
                at_ms: sample.at_ms,
                value: value(sample) * 1e6,
            })
            .collect(),
        // `gap_after` breaks at twice the bucket width.
        bucket_ms: PER_DEVICE_GAP_MS / 2,
    };
    vec![
        line("down", "metrics.download", DOWN_COLOR, |sample| {
            sample.down_mbps
        }),
        line("up", "metrics.upload", UP_COLOR, |sample| sample.up_mbps),
    ]
}

/// The note an empty window shows. Shorter windows add that earlier readings
/// may exist in a longer one.
pub fn empty_note(window_minutes: u32, translator: &Translator) -> String {
    let (key, count) = if window_minutes < 60 {
        ("time.minutesSpan", window_minutes)
    } else {
        ("time.hoursSpan", window_minutes / 60)
    };
    let plural = if count == 1 { "one" } else { "other" };
    let span = translator.text_with(&format!("{key}_{plural}"), &[("count", &count.to_string())]);
    let longest = DEVICE_WINDOWS[DEVICE_WINDOWS.len() - 1];
    let note_key = if window_minutes >= longest {
        "network.nothingRecorded"
    } else {
        "network.nothingRecordedLonger"
    };
    translator.text_with(note_key, &[("span", &span)])
}

/// A node's facts; `node_device` is the node's own roster entry when it is up.
pub fn node_facts(
    node: &NetworkNode,
    node_device: Option<&NetworkDevice>,
    translator: &Translator,
) -> Vec<(String, String)> {
    let text = |key: &str| translator.text(key);
    let mut facts = Vec::new();
    if let Some(role) = node
        .role
        .clone()
        .or_else(|| node_device.and_then(|device| device.role.clone()))
    {
        facts.push((text("network.role"), role));
    }
    if let Some(device) = node_device {
        if let Some(dbm) = device.signal_dbm {
            facts.push((
                text("network.signalStrength"),
                format!("{} dBm", dbm.round()),
            ));
        }
        if let Some(rate) = device.link.rx_rate_mbps {
            facts.push((text("network.rxRate"), format!("{} Mbps", rate.round())));
        }
        facts.push((text("network.connection"), device.band.clone()));
        if !device.mac_address.is_empty() {
            facts.push((text("network.macAddress"), device.mac_address.clone()));
        }
    }
    if let Some(id) = &node.device_id {
        facts.push((text("network.deviceId"), id.clone()));
    }
    if let Some(ip) = node_device.and_then(|device| device.ip_address.clone()) {
        facts.push((text("network.ipAddress"), ip));
    }
    if let Some(hardware) = &node.hardware_version {
        facts.push((text("terminal.hardware"), hardware.clone()));
    }
    facts
}

#[cfg(test)]
mod tests {
    use dishy_core::network::{ConnectionKind, LinkFacts};

    use super::*;
    use crate::i18n::Locale;

    const NOW: i64 = 1_800_000_000_000;

    fn device() -> NetworkDevice {
        NetworkDevice {
            key: "aa|7".into(),
            usage_key: "7".into(),
            name: Some("Laptop".into()),
            hostname: None,
            mac_address: "aa:bb:cc:00:00:01".into(),
            ip_address: Some("192.168.1.20".into()),
            ipv6_addresses: vec!["fd00::20".into(), "fd00::21".into()],
            connection: ConnectionKind::Wifi,
            band: "5 GHz".into(),
            signal_dbm: Some(-52.0),
            snr: Some(38.0),
            down_mbps: 0.0,
            up_mbps: 0.0,
            rx_bytes: Some(5_000_000),
            tx_bytes: Some(1_000_000),
            has_counters: true,
            blocked: false,
            idle_s: Some(120.0),
            client_id: Some(7),
            role: Some("CLIENT".into()),
            device_id: None,
            upstream_mac: None,
            is_client: true,
            mesh_hardware: None,
            link: LinkFacts {
                rx_rate_mbps: Some(866.0),
                tx_rate_mbps: None,
                mcs: Some(9.0),
                nss: Some(2.0),
                channel_width_mhz: Some(80.0),
            },
        }
    }

    // SPEC-NETDETAIL-002: the facts in order, with absent ones left out and the
    // month's usage preferred over the connection's.
    #[test]
    fn device_facts_read_in_order() {
        let english = Translator::new(Locale::En);
        let facts = device_facts(
            &device(),
            Some("Living room"),
            Some("Apple"),
            Some((2_000_000_000, 100_000_000)),
            &english,
        );
        let labels: Vec<&str> = facts.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Status",
                "Role",
                "Connected to",
                "Manufacturer",
                "Connection",
                "Signal",
                "Signal-to-noise",
                "Bandwidth",
                "MCS index",
                "Spatial streams",
                "Rx rate",
                "IPv4",
                "IPv6",
                "MAC address",
                "Data used this month"
            ]
        );
        assert_eq!(facts[0].1, "idle · 2m 0s");
        assert_eq!(facts[6].1, "38 dB");
        assert_eq!(facts[7].1, "80 MHz");
        assert_eq!(facts[10].1, "866 Mbps");
        assert_eq!(facts[12].1, "fd00::20");

        let mut quiet = device();
        quiet.idle_s = Some(3.0);
        quiet.role = None;
        let facts = device_facts(&quiet, None, None, None, &english);
        assert_eq!(facts[0].1, "active");
        assert_eq!(
            facts[1],
            ("Manufacturer".to_string(), "Unknown".to_string())
        );
        assert_eq!(facts.last().unwrap().0, "Data used (this connection)");
    }

    // SPEC-NETDETAIL-003: one point per reading, gaps past 150 s break the
    // line, and an empty window says so for its span.
    #[test]
    fn throughput_charts_the_window_and_explains_an_empty_one() {
        let english = Translator::new(Locale::En);
        let sample = |offset_s: i64, down: f64| ClientSample {
            key: Some("7".into()),
            mac_address: "aa".into(),
            at_ms: NOW - offset_s * 1_000,
            down_mbps: down,
            up_mbps: 1.0,
        };
        let samples = [
            sample(3_000, 9.0),
            sample(600, 5.0),
            sample(595, 6.0),
            sample(300, 7.0),
        ];
        let series = throughput_series(&samples, 15, NOW, &english);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].label, "Download");
        assert_eq!(
            series[0].points.len(),
            3,
            "the 50-minute-old reading is outside 15 min"
        );
        assert!(!series[0].gap_after(0), "5 s apart is continuous");
        assert!(series[0].gap_after(1), "295 s apart is a hole");
        assert_eq!(
            empty_note(360, &english),
            "Nothing recorded for this device in the last 6 hours."
        );
        assert!(
            empty_note(15, &english)
                .starts_with("Nothing recorded for this device in the last 15 minutes.")
        );
    }

    // SPEC-NETDETAIL-004: node facts from its own roster entry.
    #[test]
    fn node_facts_come_from_its_roster_entry() {
        let english = Translator::new(Locale::En);
        let node = NetworkNode {
            key: "Router-01".into(),
            configured_name: Some("Garage".into()),
            connected: true,
            role: Some("SATELLITE".into()),
            device_id: Some("Router-01".into()),
            client_key: Some("aa|9".into()),
            device_keys: vec!["aa|7".into()],
            hardware_version: Some("v3_mesh".into()),
        };
        let mut entry = device();
        entry.role = Some("SATELLITE".into());
        let facts = node_facts(&node, Some(&entry), &english);
        let labels: Vec<&str> = facts.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Role",
                "Signal strength",
                "Rx rate",
                "Connection",
                "MAC address",
                "Device ID",
                "IP address",
                "Hardware"
            ]
        );
        assert_eq!(facts[1].1, "-52 dBm");
        let down = node_facts(
            &NetworkNode {
                connected: false,
                ..node
            },
            None,
            &english,
        );
        let down_labels: Vec<&str> = down.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(
            down_labels,
            ["Role", "Device ID", "Hardware"],
            "radio facts need the live entry"
        );
    }
}
