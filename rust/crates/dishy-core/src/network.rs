//! The router's local network, typed and normalised for the GTK page.
//!
//! The wire shapes are the proto3 JSON the router already returns; the rest of
//! this module is pure. Rates come from byte-counter deltas over the actual
//! elapsed time, with the router's own 15-second average as the honest fallback
//! before a second reading exists.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::telemetry::ProtoF64;

/// One connected client, as `wifiGetClients` reports it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WifiClient {
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub mac_address: Option<String>,
    pub ip_address: Option<String>,
    pub ipv6_addresses: Vec<String>,
    pub signal_strength: Option<ProtoF64>,
    pub snr: Option<ProtoF64>,
    pub iface: Option<String>,
    pub iface_name: Option<String>,
    pub channel_width: Option<ProtoF64>,
    pub role: Option<String>,
    pub device_id: Option<String>,
    pub upstream_mac_address: Option<String>,
    pub client_id: Option<i64>,
    pub blocked: bool,
    pub no_data_idle_s: Option<ProtoF64>,
    pub rx_stats: Option<WifiClientStats>,
    pub tx_stats: Option<WifiClientStats>,
    pub rx_stats_valid: bool,
    pub tx_stats_valid: bool,
    pub upload_mb: Option<ProtoF64>,
    pub download_mb: Option<ProtoF64>,
    pub est_rx_rate_mbps_from_controller: Option<ProtoF64>,
    pub est_tx_rate_mbps_from_controller: Option<ProtoF64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WifiClientStats {
    pub bytes: Option<String>,
    pub rate_mbps: Option<ProtoF64>,
    pub bandwidth: Option<ProtoF64>,
    pub nss: Option<ProtoF64>,
    pub mcs: Option<ProtoF64>,
    pub throughput_mbps_last_1m_avg: Option<ProtoF64>,
    pub throughput_mbps_last_15s_avg: Option<ProtoF64>,
}

impl WifiClientStats {
    /// The byte odometer. Proto3 JSON encodes a uint64 as a string.
    pub fn byte_count(&self) -> Option<u64> {
        self.bytes.as_deref()?.trim().parse().ok()
    }

    /// The router's point-in-time rate, preferring the 15-second average over
    /// the one-minute average. A `NaN` string is rejected by `ProtoF64`.
    pub fn throughput_mbps(&self) -> f64 {
        let value = self
            .throughput_mbps_last_15s_avg
            .or(self.throughput_mbps_last_1m_avg)
            .map(|value| value.0)
            .unwrap_or(0.0);
        if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        }
    }
}

impl WifiClient {
    /// Whether this roster entry represents an end-client rather than router
    /// infrastructure such as a mesh node.
    pub fn is_client(&self) -> bool {
        self.role.as_deref().is_none_or(|role| role == "CLIENT")
    }

    /// Identity of one roster entry. A bare MAC is not enough: cloned or
    /// vendor-masked addresses can appear more than once.
    pub fn entry_key(&self) -> String {
        let id = self
            .client_id
            .map(|id| id.to_string())
            .or_else(|| self.ip_address.clone())
            .or_else(|| self.mac_address.clone())
            .unwrap_or_else(|| "unknown".into());
        match self.mac_address.as_deref() {
            Some(mac) => format!("{mac}|{id}"),
            None => id,
        }
    }

    /// The key stored usage and rules join on. Unlike the entry key, this does
    /// not fall back through the IP.
    pub fn usage_key(&self) -> String {
        self.client_id
            .map(|id| id.to_string())
            .or_else(|| self.mac_address.clone())
            .unwrap_or_default()
    }

    pub fn has_counters(&self) -> bool {
        self.rx_stats_valid
            || self.tx_stats_valid
            || self
                .rx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count)
                .is_some()
            || self
                .tx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count)
                .is_some()
    }
}

/// The Wi-Fi config's saved names and network facts.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WifiConfig {
    pub country_code: Option<String>,
    pub nameservers: Vec<String>,
    pub custom_dns_disabled: bool,
    pub networks: Vec<WifiLanNetwork>,
    pub mesh_configs: BTreeMap<String, MeshNode>,
    pub client_configs: Vec<ClientConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WifiLanNetwork {
    pub ipv4: Option<String>,
    pub domain: Option<String>,
    pub basic_service_sets: Vec<BasicServiceSet>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BasicServiceSet {
    pub bssid: Option<String>,
    pub ssid: Option<String>,
    pub band: Option<String>,
    pub iface_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MeshNode {
    pub display_name: Option<String>,
    pub hardware_version: Option<String>,
    pub last_connected: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientConfig {
    pub client_id: Option<i64>,
    pub mac_address: Option<String>,
    pub given_name: Option<String>,
}

impl WifiConfig {
    pub fn ssid(&self) -> Option<&str> {
        self.networks
            .iter()
            .flat_map(|network| &network.basic_service_sets)
            .find_map(|bss| bss.ssid.as_deref().filter(|ssid| !ssid.is_empty()))
    }

    pub fn subnet(&self) -> Option<&str> {
        self.networks.first()?.ipv4.as_deref()
    }

    /// Saved name for a client, matching the router's per-device id first and
    /// the MAC second.
    pub fn saved_name(&self, client: &WifiClient) -> Option<&str> {
        self.saved_name_for(client.client_id, client.mac_address.as_deref())
    }

    /// Saved name by router id or MAC, for callers that no longer hold the raw
    /// client entry.
    pub fn saved_name_for(
        &self,
        client_id: Option<i64>,
        mac_address: Option<&str>,
    ) -> Option<&str> {
        self.client_configs
            .iter()
            .find(|config| {
                config.client_id == client_id
                    || (config.mac_address.is_some()
                        && config.mac_address.as_deref() == mac_address)
            })
            .and_then(|config| config.given_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// The order the Electron panel uses: the live entry, the saved config,
    /// then the hostname. The caller supplies the unnamed fallback.
    pub fn display_name(&self, client: &WifiClient) -> Option<String> {
        client
            .given_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| self.saved_name(client).map(str::to_owned))
            .or_else(|| {
                client
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
    }

    pub fn mesh_name(&self, device_id: &str) -> Option<&str> {
        self.mesh_configs
            .get(device_id)
            .and_then(|node| node.display_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Wifi,
    Ethernet,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalQuality {
    Excellent,
    Good,
    Fair,
    Weak,
}

/// The band is a unit/proper noun, so it is not translated. `RF_2GHZ` is the
/// 2.4 GHz band, not "2 GHz".
pub fn iface_label(iface: Option<&str>) -> String {
    let iface = iface.unwrap_or_default();
    if iface == "ETH" {
        return "Ethernet".into();
    }
    if iface == "RF_2GHZ" {
        return "2.4 GHz".into();
    }
    if iface.starts_with("RF_5GHZ") {
        return "5 GHz".into();
    }
    if iface.starts_with("RF_6GHZ") {
        return "6 GHz".into();
    }
    if let Some(rest) = iface.strip_prefix("RF_") {
        return rest.replace("GHZ", " GHz");
    }
    if iface.is_empty() {
        "—".into()
    } else {
        iface.into()
    }
}

pub fn connection_kind(iface: Option<&str>) -> ConnectionKind {
    match iface.unwrap_or_default() {
        "ETH" => ConnectionKind::Ethernet,
        value if value.starts_with("RF_") => ConnectionKind::Wifi,
        _ => ConnectionKind::Unknown,
    }
}

/// Signal buckets shared with `networkFormat.ts`. Zero and missing mean the
/// router has no RSSI, not a catastrophic signal.
pub fn signal_quality(dbm: Option<f64>) -> Option<SignalQuality> {
    let dbm = dbm.filter(|value| value.is_finite() && *value != 0.0)?;
    if dbm > -55.0 {
        Some(SignalQuality::Excellent)
    } else if dbm > -67.0 {
        Some(SignalQuality::Good)
    } else if dbm > -75.0 {
        Some(SignalQuality::Fair)
    } else {
        Some(SignalQuality::Weak)
    }
}

/// One live rate pair, in Mbps.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClientRates {
    pub down_mbps: f64,
    pub up_mbps: f64,
}

#[derive(Debug, Clone, Copy)]
struct Counters {
    rx: u64,
    tx: u64,
    at_ms: i64,
}

/// Turns successive byte readings into rates. The router reports cumulative
/// counters; a delta over the actual elapsed time is the only honest rate.
#[derive(Debug, Default)]
pub struct ClientRateTracker {
    previous: HashMap<String, Counters>,
}

impl ClientRateTracker {
    pub fn reset(&mut self) {
        self.previous.clear();
    }

    pub fn rates(&mut self, clients: &[WifiClient], at_ms: i64) -> HashMap<String, ClientRates> {
        let mut rates = HashMap::with_capacity(clients.len());
        let mut live = std::collections::HashSet::with_capacity(clients.len());
        for client in clients {
            let key = client.entry_key();
            live.insert(key.clone());
            let fallback = ClientRates {
                down_mbps: client
                    .rx_stats
                    .as_ref()
                    .map(WifiClientStats::throughput_mbps)
                    .unwrap_or(0.0),
                up_mbps: client
                    .tx_stats
                    .as_ref()
                    .map(WifiClientStats::throughput_mbps)
                    .unwrap_or(0.0),
            };
            let Some(rx) = client
                .rx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count)
            else {
                rates.insert(key, fallback);
                continue;
            };
            let Some(tx) = client
                .tx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count)
            else {
                rates.insert(key, fallback);
                continue;
            };
            let rate = match self.previous.get(&key) {
                Some(previous)
                    if at_ms > previous.at_ms && rx >= previous.rx && tx >= previous.tx =>
                {
                    let seconds = (at_ms - previous.at_ms) as f64 / 1_000.0;
                    let down = (rx - previous.rx) as f64 * 8.0 / seconds / 1e6;
                    let up = (tx - previous.tx) as f64 * 8.0 / seconds / 1e6;
                    ClientRates {
                        down_mbps: down.max(0.0),
                        up_mbps: up.max(0.0),
                    }
                }
                _ => fallback,
            };
            self.previous
                .insert(key.clone(), Counters { rx, tx, at_ms });
            rates.insert(key, rate);
        }
        self.previous.retain(|key, _| live.contains(key));
        rates
    }
}

/// One radio's temperature and throttle duty cycle.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadioStatsReply {
    pub radio_stats: Vec<WifiRadio>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WifiRadio {
    pub band: Option<String>,
    pub thermal_status: Option<RadioThermal>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RadioThermal {
    pub temp: Option<ProtoF64>,
    pub temp2: Option<ProtoF64>,
    pub duty_cycle: Option<ProtoF64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RadioReading {
    pub band: String,
    pub temp_c: f64,
    /// Percent of airtime the radio may transmit; 100 when the router omits it.
    pub duty_cycle: f64,
}

impl RadioStatsReply {
    pub fn readings(&self) -> Vec<RadioReading> {
        self.radio_stats
            .iter()
            .filter_map(|radio| {
                let thermal = radio.thermal_status.as_ref()?;
                // This firmware leaves `temp` absent and reports `temp2`.
                let temp = thermal
                    .temp2
                    .or(thermal.temp)
                    .map(|value| value.0)
                    .filter(|value| value.is_finite())?;
                Some(RadioReading {
                    band: radio.band.clone().unwrap_or_else(|| "unknown".into()),
                    temp_c: temp,
                    duty_cycle: thermal
                        .duty_cycle
                        .map(|value| value.0)
                        .filter(|value| value.is_finite())
                        .unwrap_or(100.0),
                })
            })
            .collect()
    }
}

/// A roster entry ready for the page.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkDevice {
    pub key: String,
    pub usage_key: String,
    pub name: Option<String>,
    pub hostname: Option<String>,
    pub mac_address: String,
    pub ip_address: Option<String>,
    pub ipv6_addresses: Vec<String>,
    pub connection: ConnectionKind,
    pub band: String,
    pub signal_dbm: Option<f64>,
    pub snr: Option<f64>,
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub has_counters: bool,
    pub blocked: bool,
    pub idle_s: Option<f64>,
    pub client_id: Option<i64>,
    pub role: Option<String>,
    pub device_id: Option<String>,
    pub upstream_mac: Option<String>,
    pub is_client: bool,
    pub mesh_hardware: Option<String>,
    /// Radio link facts for the device detail (spec 0175); absent when the
    /// router did not send them.
    pub link: LinkFacts,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkFacts {
    pub rx_rate_mbps: Option<f64>,
    pub tx_rate_mbps: Option<f64>,
    pub mcs: Option<f64>,
    pub nss: Option<f64>,
    pub channel_width_mhz: Option<f64>,
}

impl NetworkDevice {
    fn from_client(client: &WifiClient, config: Option<&WifiConfig>, rates: ClientRates) -> Self {
        let role = client.role.clone();
        let is_client = client.is_client();
        let mesh_hardware = client
            .device_id
            .as_deref()
            .and_then(|id| config.and_then(|config| config.mesh_configs.get(id)))
            .and_then(|node| node.hardware_version.clone());
        let life = client
            .no_data_idle_s
            .map(|value| value.0)
            .filter(|value| value.is_finite());
        Self {
            key: client.entry_key(),
            usage_key: client.usage_key(),
            name: config.and_then(|config| config.display_name(client)),
            hostname: client
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            mac_address: client.mac_address.clone().unwrap_or_default(),
            ip_address: client.ip_address.clone(),
            ipv6_addresses: client.ipv6_addresses.clone(),
            connection: connection_kind(client.iface.as_deref()),
            band: iface_label(client.iface.as_deref()),
            signal_dbm: client
                .signal_strength
                .map(|value| value.0)
                .filter(|value| value.is_finite() && *value != 0.0),
            snr: client
                .snr
                .map(|value| value.0)
                .filter(|value| value.is_finite()),
            down_mbps: rates.down_mbps,
            up_mbps: rates.up_mbps,
            rx_bytes: client
                .rx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count),
            tx_bytes: client
                .tx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count),
            has_counters: client.has_counters(),
            blocked: client.blocked,
            idle_s: life,
            client_id: client.client_id,
            role,
            device_id: client.device_id.clone(),
            upstream_mac: client.upstream_mac_address.clone(),
            is_client,
            mesh_hardware,
            link: LinkFacts {
                rx_rate_mbps: positive(client.rx_stats.as_ref().and_then(|stats| stats.rate_mbps)),
                tx_rate_mbps: positive(client.tx_stats.as_ref().and_then(|stats| stats.rate_mbps)),
                mcs: client
                    .rx_stats
                    .as_ref()
                    .and_then(|stats| stats.mcs)
                    .map(|value| value.0)
                    .filter(|value| value.is_finite()),
                nss: client
                    .rx_stats
                    .as_ref()
                    .and_then(|stats| stats.nss)
                    .map(|value| value.0)
                    .filter(|value| value.is_finite()),
                channel_width_mhz: positive(client.channel_width),
            },
        }
    }
}

/// A proto3 number that means something only when present and above zero.
fn positive(value: Option<ProtoF64>) -> Option<f64> {
    value
        .map(|value| value.0)
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// One row in the Nodes tab, including paired nodes that are currently down.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkNode {
    pub key: String,
    pub configured_name: Option<String>,
    pub connected: bool,
    pub role: Option<String>,
    pub device_id: Option<String>,
    pub client_key: Option<String>,
    /// Keys of the client devices attached to this node.
    pub device_keys: Vec<String>,
    pub hardware_version: Option<String>,
}

/// Build the nodes roster from live infrastructure entries plus the config's
/// paired mesh nodes.
pub fn build_nodes(devices: &[NetworkDevice], config: Option<&WifiConfig>) -> Vec<NetworkNode> {
    let infrastructure: Vec<&NetworkDevice> = devices
        .iter()
        .filter(|device| device.role.as_deref().is_some_and(|role| role != "CLIENT"))
        .collect();
    let live_ids: std::collections::HashSet<String> = infrastructure
        .iter()
        .filter_map(|device| device.device_id.clone())
        .collect();
    let mut nodes: Vec<NetworkNode> = infrastructure
        .iter()
        .map(|device| NetworkNode {
            key: device
                .device_id
                .clone()
                .or_else(|| Some(device.mac_address.clone()))
                .unwrap_or_else(|| device.key.clone()),
            configured_name: device.device_id.as_deref().and_then(|id| {
                config
                    .and_then(|config| config.mesh_name(id))
                    .map(str::to_owned)
            }),
            connected: true,
            role: device.role.clone(),
            device_id: device.device_id.clone(),
            client_key: Some(device.key.clone()),
            device_keys: devices
                .iter()
                .filter(|peer| {
                    peer.is_client
                        && (peer.upstream_mac.as_deref() == Some(device.mac_address.as_str())
                            || (device.role.as_deref() == Some("CONTROLLER")
                                && peer.upstream_mac.is_none()))
                })
                .map(|peer| peer.key.clone())
                .collect(),
            hardware_version: device.mesh_hardware.clone().or_else(|| {
                device.device_id.as_deref().and_then(|id| {
                    config
                        .and_then(|config| config.mesh_configs.get(id))
                        .and_then(|node| node.hardware_version.clone())
                })
            }),
        })
        .collect();

    if let Some(config) = config {
        for (device_id, node) in &config.mesh_configs {
            if live_ids.contains(device_id) {
                continue;
            }
            nodes.push(NetworkNode {
                key: device_id.clone(),
                configured_name: node
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
                connected: false,
                role: None,
                device_id: Some(device_id.clone()),
                client_key: None,
                device_keys: Vec::new(),
                hardware_version: node.hardware_version.clone(),
            });
        }
    }

    // The router first, then connected mesh nodes, then the ones that are down.
    nodes.sort_by_key(|node| (node.role.as_deref() != Some("CONTROLLER"), !node.connected));
    nodes
}

/// Everything the network page draws, folded into one value.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkSnapshot {
    pub reachable: bool,
    pub devices: Vec<NetworkDevice>,
    pub nodes: Vec<NetworkNode>,
    pub config: Option<WifiConfig>,
    pub radios: Vec<RadioReading>,
}

impl NetworkSnapshot {
    pub fn from_roster(
        clients: &[WifiClient],
        config: Option<&WifiConfig>,
        radios: Vec<RadioReading>,
        tracker: &mut ClientRateTracker,
        at_ms: i64,
        reachable: bool,
    ) -> Self {
        let rates = tracker.rates(clients, at_ms);
        let devices: Vec<NetworkDevice> = clients
            .iter()
            .map(|client| {
                NetworkDevice::from_client(
                    client,
                    config,
                    rates.get(&client.entry_key()).copied().unwrap_or_default(),
                )
            })
            .collect();
        let nodes = build_nodes(&devices, config);
        Self {
            reachable,
            devices,
            nodes,
            config: config.cloned(),
            radios,
        }
    }

    pub fn client_devices(&self) -> impl Iterator<Item = &NetworkDevice> {
        self.devices.iter().filter(|device| device.is_client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: i64, mac: &str) -> WifiClient {
        WifiClient {
            client_id: Some(id),
            mac_address: Some(mac.into()),
            iface: Some("RF_5GHZ".into()),
            ..WifiClient::default()
        }
    }

    fn stats(bytes: &str, average: f64) -> WifiClientStats {
        WifiClientStats {
            bytes: Some(bytes.into()),
            throughput_mbps_last_15s_avg: Some(ProtoF64(average)),
            ..WifiClientStats::default()
        }
    }

    // SPEC-NET-002: the entry key keeps two clients behind one MAC apart.
    #[test]
    fn roster_entries_are_keyed_by_mac_and_router_id() {
        let a = client(7, "aa:bb:cc:dd:ee:ff");
        let b = client(8, "aa:bb:cc:dd:ee:ff");
        assert_eq!(a.entry_key(), "aa:bb:cc:dd:ee:ff|7");
        assert_eq!(b.entry_key(), "aa:bb:cc:dd:ee:ff|8");
        assert_ne!(a.entry_key(), b.entry_key());
        assert_eq!(a.usage_key(), "7");

        let without_id = WifiClient {
            mac_address: Some("aa:bb:cc:dd:ee:ff".into()),
            ip_address: Some("192.168.1.20".into()),
            ..WifiClient::default()
        };
        assert_eq!(without_id.entry_key(), "aa:bb:cc:dd:ee:ff|192.168.1.20");
        assert_eq!(without_id.usage_key(), "aa:bb:cc:dd:ee:ff");
    }

    // SPEC-NET-002: live name, saved config name, then hostname.
    #[test]
    fn names_resolve_in_the_panel_order() {
        let config = WifiConfig {
            client_configs: vec![ClientConfig {
                client_id: Some(7),
                given_name: Some("Saved".into()),
                ..ClientConfig::default()
            }],
            ..WifiConfig::default()
        };
        let mut entry = client(7, "aa:bb:cc:dd:ee:ff");
        entry.name = Some("hostname".into());
        assert_eq!(config.display_name(&entry).as_deref(), Some("Saved"));
        entry.given_name = Some("Live".into());
        assert_eq!(config.display_name(&entry).as_deref(), Some("Live"));
        entry.given_name = None;
        entry.name = Some("hostname".into());
        let empty = WifiConfig::default();
        assert_eq!(empty.display_name(&entry).as_deref(), Some("hostname"));
    }

    // SPEC-NET-003: the first reading uses the router's average, the second a
    // real delta, and a counter reset falls back rather than inventing traffic.
    #[test]
    fn rates_come_from_counter_deltas_with_a_router_average_fallback() {
        let mut tracker = ClientRateTracker::default();
        let mut entry = client(7, "aa:bb:cc:dd:ee:ff");
        entry.rx_stats = Some(stats("1000000", 12.5));
        entry.tx_stats = Some(stats("500000", 2.5));
        let first = tracker.rates(std::slice::from_ref(&entry), 1_000_000);
        let rate = first.get(&entry.entry_key()).copied().unwrap();
        assert_eq!(
            rate,
            ClientRates {
                down_mbps: 12.5,
                up_mbps: 2.5
            }
        );

        entry.rx_stats = Some(stats("1625000", 0.0));
        entry.tx_stats = Some(stats("625000", 0.0));
        let second = tracker.rates(std::slice::from_ref(&entry), 1_005_000);
        let rate = second.get(&entry.entry_key()).copied().unwrap();
        assert!((rate.down_mbps - 1.0).abs() < 1e-9); // 625,000 bytes in 5 s
        assert!((rate.up_mbps - 0.2).abs() < 1e-9);

        entry.rx_stats = Some(stats("10", 3.0));
        entry.tx_stats = Some(stats("10", 0.5));
        let reset = tracker.rates(std::slice::from_ref(&entry), 1_010_000);
        let rate = reset.get(&entry.entry_key()).copied().unwrap();
        assert_eq!(
            rate,
            ClientRates {
                down_mbps: 3.0,
                up_mbps: 0.5
            }
        );
    }

    // SPEC-NET-002, SPEC-NET-003: the real proto3 JSON field names decode into
    // the roster, including string-encoded uint64 counters and absent fields.
    #[test]
    fn wire_roster_json_decodes() {
        let json = serde_json::json!({
            "name": "pixel-9",
            "givenName": "Phone",
            "macAddress": "aa:bb:cc:dd:ee:ff",
            "ipAddress": "192.168.1.20",
            "ipv6Addresses": ["fd00::1"],
            "signalStrength": -52,
            "snr": 41,
            "iface": "RF_5GHZ",
            "role": "CLIENT",
            "clientId": 7,
            "blocked": false,
            "noDataIdleS": 12,
            "rxStats": {
                "bytes": "1234567890123",
                "rateMbps": 866,
                "throughputMbpsLast15sAvg": 12.5
            },
            "txStats": {
                "bytes": "987654321",
                "throughputMbpsLast15sAvg": "NaN"
            },
            "rxStatsValid": true,
            "txStatsValid": true
        });
        let client: WifiClient = serde_json::from_value(json).expect("wifi client JSON");
        assert_eq!(client.client_id, Some(7));
        assert_eq!(client.signal_strength.map(|value| value.0), Some(-52.0));
        assert_eq!(
            client
                .rx_stats
                .as_ref()
                .and_then(WifiClientStats::byte_count),
            Some(1_234_567_890_123)
        );
        assert_eq!(
            client
                .rx_stats
                .as_ref()
                .map(WifiClientStats::throughput_mbps),
            Some(12.5)
        );
        assert_eq!(
            client
                .tx_stats
                .as_ref()
                .map(WifiClientStats::throughput_mbps),
            Some(0.0)
        );
        assert!(client.has_counters());
        assert_eq!(client.entry_key(), "aa:bb:cc:dd:ee:ff|7");
        assert_eq!(client.usage_key(), "7");
    }

    // SPEC-NET-005: the config's mesh names and the radio reply decode from the
    // shapes the router actually sends.
    #[test]
    fn wire_config_and_radio_json_decode() {
        let config: WifiConfig = serde_json::from_value(serde_json::json!({
            "countryCode": "US",
            "networks": [{
                "ipv4": "192.168.1.1/24",
                "basicServiceSets": [{
                    "bssid": "aa:bb:cc:dd:ee:00",
                    "ssid": "Starlink",
                    "band": "RF_5GHZ",
                    "ifaceName": "wlan0"
                }]
            }],
            "meshConfigs": {
                "Router-0002": {
                    "displayName": "Living room",
                    "hardwareVersion": "rev4",
                    "lastConnected": "2026-06-01T00:00:00Z"
                }
            },
            "clientConfigs": [{
                "clientId": 7,
                "macAddress": "aa:bb:cc:dd:ee:ff",
                "givenName": "Phone"
            }]
        }))
        .expect("wifi config JSON");
        assert_eq!(config.ssid(), Some("Starlink"));
        assert_eq!(config.subnet(), Some("192.168.1.1/24"));
        assert_eq!(config.mesh_name("Router-0002"), Some("Living room"));

        let reply: RadioStatsReply = serde_json::from_value(serde_json::json!({
            "radioStats": [{
                "band": "RF_2GHZ",
                "thermalStatus": { "temp2": 47.5, "dutyCycle": 60 }
            }]
        }))
        .expect("radio stats JSON");
        let readings = reply.readings();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].temp_c, 47.5);
        assert_eq!(readings[0].duty_cycle, 60.0);
    }

    // SPEC-NET-004: absent counters are absent, not a zero reading.
    #[test]
    fn a_device_without_counters_is_distinguishable_from_zero_use() {
        let wired = WifiClient {
            mac_address: Some("aa:bb:cc:dd:ee:ff".into()),
            iface: Some("ETH".into()),
            ..WifiClient::default()
        };
        assert!(!wired.has_counters());
        let device = NetworkDevice::from_client(&wired, None, ClientRates::default());
        assert_eq!(device.connection, ConnectionKind::Ethernet);
        assert_eq!(device.band, "Ethernet");
        assert_eq!(device.rx_bytes, None);
        assert!(!device.has_counters);
        assert_eq!(iface_label(Some("RF_2GHZ")), "2.4 GHz");
        assert_eq!(signal_quality(Some(-50.0)), Some(SignalQuality::Excellent));
        assert_eq!(signal_quality(Some(-70.0)), Some(SignalQuality::Fair));
        assert_eq!(signal_quality(Some(0.0)), None);
    }

    // SPEC-NET-005: paired mesh nodes that are down still appear, and the live
    // router joins them at the top.
    #[test]
    fn nodes_join_live_entries_with_configured_mesh_nodes() {
        let router = WifiClient {
            role: Some("CONTROLLER".into()),
            mac_address: Some("router-mac".into()),
            device_id: Some("Router-0001".into()),
            ..WifiClient::default()
        };
        let mesh = WifiClient {
            role: Some("MESH".into()),
            mac_address: Some("mesh-mac".into()),
            device_id: Some("Router-0002".into()),
            ..WifiClient::default()
        };
        let attached = WifiClient {
            role: Some("CLIENT".into()),
            mac_address: Some("client-mac".into()),
            upstream_mac_address: Some("mesh-mac".into()),
            ..WifiClient::default()
        };
        let config = WifiConfig {
            mesh_configs: BTreeMap::from([
                (
                    "Router-0002".into(),
                    MeshNode {
                        display_name: Some("Living room".into()),
                        hardware_version: Some("rev4".into()),
                        ..MeshNode::default()
                    },
                ),
                (
                    "Router-0003".into(),
                    MeshNode {
                        display_name: Some("Garage".into()),
                        ..MeshNode::default()
                    },
                ),
            ]),
            ..WifiConfig::default()
        };
        let mut tracker = ClientRateTracker::default();
        let snapshot = NetworkSnapshot::from_roster(
            &[router, mesh, attached],
            Some(&config),
            Vec::new(),
            &mut tracker,
            1_000,
            true,
        );
        assert_eq!(snapshot.nodes.len(), 3);
        assert!(snapshot.nodes[0].connected);
        assert_eq!(snapshot.nodes[0].role.as_deref(), Some("CONTROLLER"));
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .find(|node| node.key == "Router-0002")
                .map(|node| node.device_keys.len()),
            Some(1)
        );
        let garage = snapshot
            .nodes
            .iter()
            .find(|node| node.key == "Router-0003")
            .expect("configured node");
        assert!(!garage.connected);
        assert_eq!(garage.configured_name.as_deref(), Some("Garage"));
    }

    // SPEC-NET-005: the router's fallback sensor and default duty cycle.
    #[test]
    fn radio_readings_use_the_temperature_the_router_reports() {
        let reply = RadioStatsReply {
            radio_stats: vec![
                WifiRadio {
                    band: Some("RF_2GHZ".into()),
                    thermal_status: Some(RadioThermal {
                        temp: Some(ProtoF64(40.0)),
                        temp2: Some(ProtoF64(47.5)),
                        duty_cycle: Some(ProtoF64(60.0)),
                    }),
                },
                WifiRadio {
                    band: Some("RF_5GHZ".into()),
                    thermal_status: Some(RadioThermal {
                        temp: Some(ProtoF64(51.0)),
                        temp2: None,
                        duty_cycle: None,
                    }),
                },
                WifiRadio {
                    band: Some("RF_6GHZ".into()),
                    thermal_status: Some(RadioThermal::default()),
                },
            ],
        };
        let readings = reply.readings();
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].temp_c, 47.5);
        assert_eq!(readings[0].duty_cycle, 60.0);
        assert_eq!(readings[1].temp_c, 51.0);
        assert_eq!(readings[1].duty_cycle, 100.0);
    }
}
