//! IEEE OUI vendor registry lookup and private MAC address detection.
//!
//! Mirrors upstream `src/lib/macVendor.ts`: resolves the first 3 octets against the
//! IEEE registry, while detecting locally administered (randomized) MAC
//! addresses as private.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Marker returned for a randomized or locally administered MAC address.
pub const PRIVATE_VENDOR: &str = "Private";

/// Embedded default registry loaded from `data/oui.json`.
static BUNDLED_REGISTRY: OnceLock<OuiRegistry> = OnceLock::new();

/// The IEEE OUI vendor registry.
#[derive(Debug, Clone, Default)]
pub struct OuiRegistry {
    entries: HashMap<String, String>,
}

impl OuiRegistry {
    /// Load an OUI registry from a JSON object mapping 6-hex OUI keys to vendor names.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        let entries: HashMap<String, String> = serde_json::from_str(json)?;
        Ok(Self { entries })
    }

    /// Access the shared bundled registry instance.
    pub fn bundled() -> &'static Self {
        BUNDLED_REGISTRY.get_or_init(|| {
            const BUNDLED_JSON: &str = include_str!("../data/oui.json");
            Self::from_json_str(BUNDLED_JSON).unwrap_or_default()
        })
    }

    /// Resolve the vendor for a MAC address.
    ///
    /// If the address is randomized/locally administered, returns `Some("Private")`.
    /// Otherwise, looks up the 6-hex OUI prefix in the registry.
    // SPEC-DEV-001: randomized MACs report Private; real OUIs resolve from the registry.
    pub fn vendor_for_mac(&self, mac: &str) -> Option<&str> {
        if is_private_mac(mac) {
            return Some(PRIVATE_VENDOR);
        }
        let key = oui_key(mac);
        self.entries.get(&key).map(|s| s.as_str())
    }
}

/// Normalizes a MAC string to 6 lowercase hex characters without separators.
pub fn oui_key(mac: &str) -> String {
    mac.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Returns true if the MAC address has the locally administered / randomized bit set.
///
/// In standard IEEE 802 MAC format, the second hexadecimal digit of the first octet
/// has the local assignment bit (bit 1) set when it is 2, 6, A, or E.
// SPEC-DEV-001: private MAC detection.
pub fn is_private_mac(mac: &str) -> bool {
    let mut hex = mac.chars().filter(|c| c.is_ascii_hexdigit());
    let _first = hex.next();
    match hex.next() {
        Some(second) => matches!(second, '2' | '6' | 'a' | 'A' | 'e' | 'E'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_private_mac_addresses() {
        assert!(is_private_mac("02:00:00:00:00:00"));
        assert!(is_private_mac("da:a1:19:bb:cc:dd"));
        assert!(is_private_mac("f6:74:f4:00:11:22"));
        assert!(is_private_mac("FE:74:F4:00:11:22"));

        assert!(!is_private_mac("00:00:0c:44:55:66"));
        assert!(!is_private_mac("10:00:20:11:22:33"));
        assert!(!is_private_mac("60:74:f4:aa:bb:cc"));
        assert!(!is_private_mac(""));
    }

    #[test]
    fn oui_keys_extract_six_hex_digits() {
        assert_eq!(oui_key("10:00:20:11:22:33"), "100020");
        assert_eq!(oui_key("00-00-0C-44-55-66"), "00000c");
        assert_eq!(oui_key("da.a1.19"), "daa119");
    }

    #[test]
    fn resolves_vendors_from_bundled_registry() {
        let registry = OuiRegistry::bundled();
        assert_eq!(registry.vendor_for_mac("10:00:20:11:22:33"), Some("Apple"));
        assert_eq!(
            registry.vendor_for_mac("00:00:0c:44:55:66"),
            Some("Cisco Systems")
        );
        assert_eq!(
            registry.vendor_for_mac("02:11:22:33:44:55"),
            Some(PRIVATE_VENDOR)
        );
    }
}
