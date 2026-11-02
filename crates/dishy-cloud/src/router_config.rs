//! Typed, validated router configuration writes for the authenticated gateway.
//!
//! This module deliberately stops at request construction. Callers still send
//! the resulting JSON through [`CloudClient::router_call`], which selects a
//! fresh controller and owns the credential/protobuf boundary.

use std::net::IpAddr;

use serde_json::{Value, json};

use crate::CloudError;

const MAX_NAMESERVERS: usize = 4;
const MIN_PASSPHRASE: usize = 8;
const MAX_PASSPHRASE: usize = 63;
const SUBNET_PRESETS: &[&str] = &[
    "192.168.1.1/24",
    "192.168.2.1/24",
    "192.168.3.1/24",
    "192.168.4.1/24",
    "10.0.0.1/16",
    "10.1.0.1/16",
    "10.2.0.1/16",
    "10.3.0.1/16",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterConfigUpdate {
    /// An empty list restores the router's default DNS resolvers.
    CustomDns {
        nameservers: Vec<String>,
    },
    Bypass {
        enabled: bool,
    },
    FactoryReset,
    Subnet {
        subnet: String,
        password: String,
    },
    /// The Wi-Fi passphrase alone, subnet untouched (spec 0185).
    WifiPassword {
        password: String,
    },
    MeshName {
        device_id: String,
        display_name: String,
    },
    /// Add or remove the client's whole-week `_permanent` block (spec 0170).
    /// `mac_address` lets a client the saved list lacks be appended from the
    /// live roster.
    ClientPause {
        client_id: u32,
        paused: bool,
        mac_address: Option<String>,
    },
    ClientRename {
        client_id: u32,
        given_name: String,
    },
}

/// The schedule group a pause is filed under, as the Starlink app writes it.
const PERMANENT_GROUP: &str = "_permanent";
const WEEK_MINUTES: u32 = 7 * 24 * 60;

/// The saved client list, with a client the router reported live but never
/// saved appended. Both client writes rewrite the whole list.
fn client_configs_with(
    wifi_config: &Value,
    client_id: u32,
    mac_address: Option<&str>,
) -> Result<Vec<Value>, CloudError> {
    let mut clients: Vec<Value> = wifi_config
        .get("clientConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let known = clients
        .iter()
        .any(|client| client.get("clientId").and_then(Value::as_u64) == Some(u64::from(client_id)));
    if !known {
        let mac_address = mac_address.ok_or_else(|| {
            CloudError::GrpcProtocol("the router does not know this device".into())
        })?;
        clients.push(json!({ "clientId": client_id, "macAddress": mac_address }));
    }
    Ok(clients)
}

fn is_client(client: &Value, client_id: u32) -> bool {
    client.get("clientId").and_then(Value::as_u64) == Some(u64::from(client_id))
}

/// The auth blocks whose `password` is the Wi-Fi passphrase. `authRadius` also
/// has one, but it is an external server's credential and must not change.
const PASSPHRASE_AUTH_KEYS: [&str; 3] = ["authWpa2", "authWpa3", "authWpa2Wpa3"];

/// The router's networks to send back with a new passphrase, and a new LAN
/// address on the first network when moving the subnet. The router assigns
/// `bssid` and `ifaceName` and discards the whole message if they come back;
/// the stored passphrase reads back masked, so every block carrying one is
/// overwritten with the real value.
fn networks_with_passphrase(
    wifi_config: &Value,
    password: &str,
    subnet: Option<&str>,
) -> Result<Vec<Value>, CloudError> {
    let networks = wifi_config
        .get("networks")
        .and_then(Value::as_array)
        .filter(|networks| !networks.is_empty())
        .ok_or_else(|| CloudError::GrpcProtocol("router reported no networks".into()))?;
    Ok(networks
        .iter()
        .enumerate()
        .map(|(index, network)| {
            let mut next = network.as_object().cloned().unwrap_or_default();
            if index == 0
                && let Some(subnet) = subnet
            {
                next.insert("ipv4".into(), Value::String(subnet.into()));
            }
            if let Some(sets) = next
                .get_mut("basicServiceSets")
                .and_then(Value::as_array_mut)
            {
                for set in sets.iter_mut().filter_map(Value::as_object_mut) {
                    set.remove("bssid");
                    set.remove("ifaceName");
                    for key in PASSPHRASE_AUTH_KEYS {
                        if let Some(auth) = set.get_mut(key).and_then(Value::as_object_mut)
                            && auth.contains_key("password")
                        {
                            auth.insert("password".into(), Value::String(password.into()));
                        }
                    }
                }
            }
            Value::Object(next)
        })
        .collect())
}

/// Build the precise proto3-JSON request for supported low-risk configuration
/// mutations. DNS is normalized before it can be encoded or sent.
pub fn build_router_config_request(
    target_id: &str,
    update: RouterConfigUpdate,
) -> Result<Value, CloudError> {
    build_router_config_request_with_context(target_id, update, &Value::Null)
}

/// Build a router configuration request using the current `wifiConfig` as
/// read-modify-write context for operations whose protobuf arm replaces a
/// whole collection.
pub fn build_router_config_request_with_context(
    target_id: &str,
    update: RouterConfigUpdate,
    wifi_config: &Value,
) -> Result<Value, CloudError> {
    validate_router_config_update(&update)?;
    if !target_id.starts_with("Router-") || target_id.len() == "Router-".len() {
        return Err(CloudError::GrpcProtocol("invalid router target id".into()));
    }

    let mut request = json!({ "targetId": target_id });
    match update {
        RouterConfigUpdate::CustomDns { nameservers } => {
            let mut normalized = Vec::with_capacity(nameservers.len());
            for address in nameservers {
                let parsed = address
                    .trim()
                    .parse::<IpAddr>()
                    .expect("validated DNS address");
                let canonical = parsed.to_string();
                if !normalized.contains(&canonical) {
                    normalized.push(canonical);
                }
            }
            let custom_dns_disabled = normalized.is_empty();
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "nameservers": normalized,
                "applyNameservers": true,
                "customDnsDisabled": custom_dns_disabled,
                "applyCustomDnsDisabled": true,
            }});
        }
        RouterConfigUpdate::Bypass { enabled } => {
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "bypassMode": enabled,
                "applyBypassMode": true
            }});
        }
        RouterConfigUpdate::FactoryReset => {
            request["factoryReset"] = json!({});
        }
        RouterConfigUpdate::Subnet { subnet, password } => {
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "networks": networks_with_passphrase(wifi_config, &password, Some(&subnet))?,
                "applyNetworks": true
            }});
        }
        RouterConfigUpdate::WifiPassword { password } => {
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "networks": networks_with_passphrase(wifi_config, &password, None)?,
                "applyNetworks": true
            }});
        }
        RouterConfigUpdate::MeshName {
            device_id,
            display_name,
        } => {
            let display_name = display_name.trim();
            let mesh_configs = wifi_config
                .get("meshConfigs")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    CloudError::GrpcProtocol("router does not know this mesh node".into())
                })?;
            if !mesh_configs.contains_key(&device_id) {
                return Err(CloudError::GrpcProtocol(
                    "router does not know this mesh node".into(),
                ));
            }
            let mut rebuilt = serde_json::Map::new();
            for (id, value) in mesh_configs {
                let Some(mut node) = value.as_object().cloned() else {
                    continue;
                };
                node.remove("incarnation");
                if id == &device_id {
                    node.insert("displayName".into(), Value::String(display_name.to_owned()));
                }
                rebuilt.insert(id.clone(), Value::Object(node));
            }
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "meshConfigs": rebuilt,
                "applyMeshConfigs": true
            }});
        }
        RouterConfigUpdate::ClientPause {
            client_id,
            paused,
            mac_address,
        } => {
            let clients = client_configs_with(wifi_config, client_id, mac_address.as_deref())?
                .into_iter()
                .map(|mut client| {
                    if is_client(&client, client_id) {
                        let mut schedules: Vec<Value> = client
                            .get("weeklyBlockSchedules")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        schedules.retain(|schedule| {
                            schedule.get("groupId").and_then(Value::as_str) != Some(PERMANENT_GROUP)
                        });
                        if paused {
                            schedules.push(json!({
                                "blockRanges": [{ "startMinutes": 0, "endMinutes": WEEK_MINUTES }],
                                "groupId": PERMANENT_GROUP
                            }));
                        }
                        client["weeklyBlockSchedules"] = Value::Array(schedules);
                    }
                    client
                })
                .collect::<Vec<_>>();
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "clientConfigs": clients,
                "applyClientConfigs": true
            }});
        }
        RouterConfigUpdate::ClientRename {
            client_id,
            given_name,
        } => {
            // Keyed by client id only: the firmware masks MACs, so several
            // devices can share one, and an unknown id is refused.
            let clients = client_configs_with(wifi_config, client_id, None)?
                .into_iter()
                .map(|mut client| {
                    if is_client(&client, client_id) {
                        client["givenName"] = Value::String(given_name.trim().to_owned());
                    }
                    client
                })
                .collect::<Vec<_>>();
            request["wifiSetConfig"] = json!({ "wifiConfig": {
                "clientConfigs": clients,
                "applyClientConfigs": true
            }});
        }
    }
    Ok(request)
}

/// Validate user-controlled fields before account lookup or gateway traffic.
/// This matters for subnet/password failures: asking Starlink for a session or
/// router config is unnecessary when the submitted value cannot be accepted.
pub fn validate_router_config_update(update: &RouterConfigUpdate) -> Result<(), CloudError> {
    match update {
        RouterConfigUpdate::CustomDns { nameservers } => {
            if nameservers.len() > MAX_NAMESERVERS {
                return Err(CloudError::GrpcProtocol(
                    "at most four DNS server addresses are allowed".into(),
                ));
            }
            for address in nameservers {
                address
                    .trim()
                    .parse::<IpAddr>()
                    .map_err(|_| CloudError::GrpcProtocol("invalid DNS server address".into()))?;
            }
        }
        RouterConfigUpdate::Subnet { subnet, password } => {
            if !SUBNET_PRESETS.contains(&subnet.as_str()) {
                return Err(CloudError::GrpcProtocol("unsupported subnet".into()));
            }
            validate_passphrase(password)?;
        }
        RouterConfigUpdate::WifiPassword { password } => validate_passphrase(password)?,
        RouterConfigUpdate::MeshName { display_name, .. } if display_name.trim().is_empty() => {
            return Err(CloudError::GrpcProtocol("mesh name cannot be empty".into()));
        }
        RouterConfigUpdate::ClientRename { given_name, .. } if given_name.trim().is_empty() => {
            return Err(CloudError::GrpcProtocol(
                "device name cannot be empty".into(),
            ));
        }
        RouterConfigUpdate::Bypass { .. }
        | RouterConfigUpdate::FactoryReset
        | RouterConfigUpdate::MeshName { .. }
        | RouterConfigUpdate::ClientPause { .. }
        | RouterConfigUpdate::ClientRename { .. } => {}
    }
    Ok(())
}

fn validate_passphrase(password: &str) -> Result<(), CloudError> {
    let length = password.encode_utf16().count();
    if (MIN_PASSPHRASE..=MAX_PASSPHRASE).contains(&length) {
        Ok(())
    } else {
        Err(CloudError::GrpcProtocol(
            "Wi-Fi password length must be 8 to 63 characters".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn config() -> Value {
        json!({ "clientConfigs": [
            { "clientId": 7, "macAddress": "aa:bb:cc:00:00:00",
              "weeklyBlockSchedules": [{ "groupId": "bedtime", "blockRanges": [{ "startMinutes": 1, "endMinutes": 2 }] }] },
            { "clientId": 8, "macAddress": "dd:ee:ff:00:00:00", "givenName": "TV" }
        ]})
    }

    fn client_configs(request: &Value) -> &Vec<Value> {
        request["wifiSetConfig"]["wifiConfig"]["clientConfigs"]
            .as_array()
            .expect("client configs")
    }

    fn wifi_config() -> Value {
        json!({ "networks": [
            { "ipv4": "192.168.1.1/24", "basicServiceSets": [
                { "ssid": "Home", "bssid": "aa", "ifaceName": "wl0", "band": "RF_2GHZ",
                  "authWpa2": { "password": "•••••" } },
                { "ssid": "Home", "bssid": "bb", "ifaceName": "wl1", "band": "RF_5GHZ",
                  "authWpa2Wpa3": { "password": "•••••" },
                  "authRadius": { "password": "radius-secret", "server": "10.0.0.2" } }
            ]},
            { "basicServiceSets": [{ "ssid": "Guest", "authWpa3": { "password": "•••••" }, "authOpen": {} }] }
        ]})
    }

    // SPEC-WIFIPW-001: the passphrase replaces every Wi-Fi auth password,
    // RADIUS and the subnet stay, and router-assigned fields are dropped.
    #[test]
    fn wifi_password_rewrites_only_the_passphrases() {
        let request = build_router_config_request_with_context(
            "Router-0100",
            RouterConfigUpdate::WifiPassword {
                password: "correct horse".into(),
            },
            &wifi_config(),
        )
        .unwrap();
        let config = &request["wifiSetConfig"]["wifiConfig"];
        assert_eq!(config["applyNetworks"], json!(true));
        let networks = config["networks"].as_array().unwrap();
        assert_eq!(networks[0]["ipv4"], json!("192.168.1.1/24"));
        let first = &networks[0]["basicServiceSets"][0];
        assert_eq!(first["authWpa2"]["password"], json!("correct horse"));
        assert!(first.get("bssid").is_none() && first.get("ifaceName").is_none());
        assert_eq!(first["band"], json!("RF_2GHZ"));
        let second = &networks[0]["basicServiceSets"][1];
        assert_eq!(second["authWpa2Wpa3"]["password"], json!("correct horse"));
        assert_eq!(second["authRadius"]["password"], json!("radius-secret"));
        let guest = &networks[1]["basicServiceSets"][0];
        assert_eq!(guest["authWpa3"]["password"], json!("correct horse"));
        assert!(
            networks[1].get("ipv4").is_none(),
            "only the first network carries the LAN address"
        );
        assert!(
            build_router_config_request_with_context(
                "Router-0100",
                RouterConfigUpdate::WifiPassword {
                    password: "correct horse".into()
                },
                &json!({ "networks": [] }),
            )
            .is_err()
        );
    }

    // SPEC-WIFIPW-002: 8 to 63 characters, as for the subnet move, and the
    // subnet write still sets the address through the shared rewrite.
    #[test]
    fn wifi_password_length_matches_the_subnet_rule() {
        for (password, ok) in [
            ("1234567", false),
            ("12345678", true),
            (&"x".repeat(63)[..], true),
            (&"x".repeat(64)[..], false),
        ] {
            let update = RouterConfigUpdate::WifiPassword {
                password: password.into(),
            };
            assert_eq!(
                validate_router_config_update(&update).is_ok(),
                ok,
                "{}",
                password.len()
            );
        }
        let moved = build_router_config_request_with_context(
            "Router-0100",
            RouterConfigUpdate::Subnet {
                subnet: "10.0.0.1/16".into(),
                password: "correct horse".into(),
            },
            &wifi_config(),
        )
        .unwrap();
        let networks = &moved["wifiSetConfig"]["wifiConfig"]["networks"];
        assert_eq!(networks[0]["ipv4"], json!("10.0.0.1/16"));
        assert_eq!(
            networks[1]["basicServiceSets"][0]["authWpa3"]["password"],
            json!("correct horse")
        );
    }

    // SPEC-PAUSE-001: pausing adds a whole-week _permanent block to one client
    // and keeps every other entry and schedule; unpausing removes only it.
    #[test]
    fn pause_adds_and_unpause_removes_only_the_permanent_block() {
        let pause = RouterConfigUpdate::ClientPause {
            client_id: 7,
            paused: true,
            mac_address: None,
        };
        let request =
            build_router_config_request_with_context("Router-abc", pause, &config()).unwrap();
        assert_eq!(request["targetId"], "Router-abc");
        assert_eq!(
            request["wifiSetConfig"]["wifiConfig"]["applyClientConfigs"],
            true
        );
        let clients = client_configs(&request);
        assert_eq!(clients[1], config()["clientConfigs"][1]);
        let schedules = clients[0]["weeklyBlockSchedules"].as_array().unwrap();
        assert_eq!(schedules[0]["groupId"], "bedtime");
        assert_eq!(
            schedules[1],
            json!({ "blockRanges": [{ "startMinutes": 0, "endMinutes": 10080 }], "groupId": "_permanent" })
        );

        let paused_config = json!({ "clientConfigs": client_configs(&request) });
        let unpause = RouterConfigUpdate::ClientPause {
            client_id: 7,
            paused: false,
            mac_address: None,
        };
        let request =
            build_router_config_request_with_context("Router-abc", unpause, &paused_config)
                .unwrap();
        assert_eq!(
            client_configs(&request)[0]["weeklyBlockSchedules"],
            config()["clientConfigs"][0]["weeklyBlockSchedules"]
        );
    }

    // SPEC-PAUSE-001: a client absent from the saved list is appended from the
    // live roster, and refused without its MAC; the target must be a router.
    #[test]
    fn missing_clients_append_from_the_roster_or_are_refused() {
        let appended = build_router_config_request_with_context(
            "Router-abc",
            RouterConfigUpdate::ClientPause {
                client_id: 9,
                paused: true,
                mac_address: Some("11:22:33:00:00:00".into()),
            },
            &config(),
        )
        .unwrap();
        let clients = client_configs(&appended);
        assert_eq!(clients.len(), 3);
        assert_eq!(clients[2]["clientId"], 9);
        assert_eq!(clients[2]["macAddress"], "11:22:33:00:00:00");

        let unknown = RouterConfigUpdate::ClientPause {
            client_id: 9,
            paused: true,
            mac_address: None,
        };
        assert!(
            build_router_config_request_with_context("Router-abc", unknown.clone(), &config())
                .is_err()
        );
        assert!(build_router_config_request_with_context("Dish-abc", unknown, &config()).is_err());
    }

    // SPEC-PAUSE-002: a rename sets one client's givenName and refuses an id
    // the router has never reported.
    #[test]
    fn rename_sets_one_given_name() {
        let request = build_router_config_request_with_context(
            "Router-abc",
            RouterConfigUpdate::ClientRename {
                client_id: 7,
                given_name: "Laptop".into(),
            },
            &config(),
        )
        .unwrap();
        let clients = client_configs(&request);
        assert_eq!(clients[0]["givenName"], "Laptop");
        assert_eq!(clients[1]["givenName"], "TV");
        assert!(
            build_router_config_request_with_context(
                "Router-abc",
                RouterConfigUpdate::ClientRename {
                    client_id: 99,
                    given_name: "Ghost".into(),
                },
                &config(),
            )
            .is_err()
        );
        assert!(
            validate_router_config_update(&RouterConfigUpdate::ClientRename {
                client_id: 7,
                given_name: "  ".into(),
            })
            .is_err()
        );
    }
}
