//! Best-effort device form factor classification from device name.
//!
//! Mirrors `src/lib/deviceKind.ts`: matches whole-word tokens as well as distinctive
//! substrings against known device types.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// High-level device form factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Phone,
    Laptop,
    Desktop,
    Tablet,
    Watch,
    Tv,
    Console,
    Light,
    Unknown,
}

impl DeviceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Laptop => "laptop",
            Self::Desktop => "desktop",
            Self::Tablet => "tablet",
            Self::Watch => "watch",
            Self::Tv => "tv",
            Self::Console => "console",
            Self::Light => "light",
            Self::Unknown => "unknown",
        }
    }

    /// Symbolic icon name from the standard Freedesktop / Adwaita icon theme.
    pub const fn icon_name(self) -> &'static str {
        match self {
            Self::Phone => "phone-symbolic",
            Self::Laptop => "laptop-symbolic",
            Self::Desktop => "computer-symbolic",
            Self::Tablet => "tablet-symbolic",
            Self::Watch => "alarm-symbolic",
            Self::Tv => "video-display-symbolic",
            Self::Console => "applications-games-symbolic",
            Self::Light => "lightbulb-symbolic",
            Self::Unknown => "network-wired-symbolic",
        }
    }
}

struct KindRule {
    kind: DeviceKind,
    tokens: &'static [&'static str],
    text: &'static [&'static str],
}

// Ordered rules matching `src/lib/deviceKind.ts`.
const KIND_RULES: &[KindRule] = &[
    KindRule {
        kind: DeviceKind::Watch,
        tokens: &["watch"],
        text: &["applewatch", "apple watch", "galaxy watch"],
    },
    KindRule {
        kind: DeviceKind::Tablet,
        tokens: &["ipad", "tablet", "kindle"],
        text: &["ipad", "galaxy tab", "surface pro"],
    },
    KindRule {
        kind: DeviceKind::Tv,
        tokens: &["tv", "roku", "chromecast", "firestick", "shield", "appletv"],
        text: &[
            "apple tv",
            "apple-tv",
            "fire tv",
            "firetv",
            "chromecast",
            "roku",
            "nvidia shield",
            "bravia",
            "vizio",
            "webos",
            "smart tv",
            "smarttv",
            "google tv",
        ],
    },
    KindRule {
        kind: DeviceKind::Light,
        tokens: &["lamp", "light", "led"],
        text: &[],
    },
    KindRule {
        kind: DeviceKind::Console,
        tokens: &["console", "ps5", "ps4", "ps3", "xbox", "playstation"],
        text: &["playstation", "nintendo", "xbox", "steam deck", "steamdeck"],
    },
    KindRule {
        kind: DeviceKind::Phone,
        tokens: &[
            "phone", "iphone", "android", "pixel", "galaxy", "oneplus", "oppo", "vivo", "moto",
            "nokia",
        ],
        text: &[
            "iphone", "android", "pixel", "oneplus", "xiaomi", "redmi", "huawei", "oppo",
            "motorola", "nokia",
        ],
    },
    KindRule {
        kind: DeviceKind::Laptop,
        tokens: &[
            "laptop",
            "macbook",
            "notebook",
            "thinkpad",
            "chromebook",
            "xps",
        ],
        text: &[
            "macbook",
            "laptop",
            "notebook",
            "thinkpad",
            "chromebook",
            "zenbook",
            "ideapad",
            "spectre",
            "elitebook",
        ],
    },
    KindRule {
        kind: DeviceKind::Desktop,
        tokens: &[
            "imac",
            "desktop",
            "pc",
            "workstation",
            "macmini",
            "macstudio",
        ],
        text: &[
            "imac",
            "mac mini",
            "mac-mini",
            "macmini",
            "mac studio",
            "macstudio",
            "desktop",
            "workstation",
        ],
    },
];

/// Classify a device name into its corresponding `DeviceKind`.
// SPEC-DEV-002: device classification by token and substring rules.
pub fn classify_device(name: Option<&str>) -> DeviceKind {
    let Some(raw_str) = name else {
        return DeviceKind::Unknown;
    };
    let raw = raw_str.trim().to_lowercase();
    if raw.is_empty() {
        return DeviceKind::Unknown;
    }

    let tokens: HashSet<&str> = raw
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    for rule in KIND_RULES {
        let token_match = rule.tokens.iter().any(|token| tokens.contains(token));
        let text_match = rule.text.iter().any(|snippet| raw.contains(snippet));
        if token_match || text_match {
            return rule.kind;
        }
    }

    DeviceKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_device_types() {
        assert_eq!(classify_device(Some("Neo's iPhone 15")), DeviceKind::Phone);
        assert_eq!(classify_device(Some("MacBook Pro")), DeviceKind::Laptop);
        assert_eq!(classify_device(Some("Desk iMac")), DeviceKind::Desktop);
        assert_eq!(classify_device(Some("Living Room TV")), DeviceKind::Tv);
        assert_eq!(classify_device(Some("PS5 Pro")), DeviceKind::Console);
        assert_eq!(classify_device(Some("Apple Watch")), DeviceKind::Watch);
        assert_eq!(classify_device(Some("iPad Mini")), DeviceKind::Tablet);
        assert_eq!(classify_device(Some("Smart LED Lamp")), DeviceKind::Light);
        assert_eq!(classify_device(Some("Generic Switch")), DeviceKind::Unknown);
        assert_eq!(classify_device(None), DeviceKind::Unknown);
    }
}
