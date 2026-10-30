//! The host's settings, stored beside the recorder's history.
//!
//! Electron owns this file in `electron/preferences.ts`; the Rust shell reads
//! and writes the same JSON object in the same place. Every field is validated
//! independently, as the TypeScript implementation does: a hand-edited value
//! falls back to its default instead of taking the rest of the file with it.

use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use dishy_sky::ObserverLocation;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::i18n::Locale;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    // The desktop app's long-standing default (`src/lib/theme.ts`).
    #[default]
    Dark,
    System,
}

impl Theme {
    pub const fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
            Self::System => Self::Light,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrayIconStyle {
    Template,
    Outline,
    #[default]
    Original,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimeFormat {
    #[default]
    Auto,
    Hour12,
    Hour24,
}

impl TimeFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hour12 => "12h",
            Self::Hour24 => "24h",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "12h" => Some(Self::Hour12),
            "24h" => Some(Self::Hour24),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolbarStyle {
    #[default]
    Dock,
    Rail,
}

impl ToolbarStyle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dock => "dock",
            Self::Rail => "rail",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "dock" => Some(Self::Dock),
            "rail" => Some(Self::Rail),
            _ => None,
        }
    }
}

impl Serialize for ToolbarStyle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolbarStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_value(&value).ok_or_else(|| serde::de::Error::custom("invalid toolbar style"))
    }
}

impl Serialize for TimeFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TimeFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_value(&value).ok_or_else(|| serde::de::Error::custom("invalid time format"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl WindowBounds {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// `None` means never asked, not declined.
    pub notifications: Option<bool>,
    /// One display preference, whichever surface Linux offers. The stored key
    /// keeps Electron's name so an existing file keeps working.
    pub menu_bar_throughput: bool,
    /// macOS-only in Electron; retained so the same file round-trips.
    pub hide_tray_icon: bool,
    pub tray_icon_style: TrayIconStyle,
    pub window_bounds: Option<WindowBounds>,
    pub router_address: Option<String>,
    pub self_client_id: Option<u32>,
    pub locale: Option<Locale>,
    pub time_format: TimeFormat,
    pub toolbar_style: ToolbarStyle,
    /// Added by the Rust shell; Electron ignores the unknown field until the
    /// renderer's theme preference is moved into this shared file.
    pub theme: Theme,
    /// Where the dish stands, for satellite look angles. Electron keeps the
    /// same shape in localStorage, so the two hosts do not share it yet.
    pub observer_location: Option<ObserverLocation>,
    /// The alert chime (spec 0178), on unless muted. Separate from the
    /// notification opt-in so sound works with notifications off.
    pub alert_sounds: bool,
    /// Thin the dark glass so a compositor blur shows through (spec 0190).
    pub see_through_glass: bool,
    /// The StatusNotifier tray; the Shell extension's pill can replace it.
    pub show_tray: bool,
    /// The user installed the Shell extension; enable it once the Shell knows it.
    pub shell_extension_requested: bool,
    /// A deliberate clear, kept apart from "never set" (see
    /// `observer_location.rs`).
    pub location_cleared: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            notifications: None,
            // The Linux tray is this port's throughput surface; spec 0040 asks it
            // to report live rates. Electron's Linux default is off only because
            // it has no tray-title equivalent.
            menu_bar_throughput: true,
            hide_tray_icon: false,
            tray_icon_style: TrayIconStyle::Original,
            window_bounds: None,
            router_address: None,
            self_client_id: None,
            locale: None,
            time_format: TimeFormat::Auto,
            toolbar_style: ToolbarStyle::Dock,
            theme: Theme::Dark,
            observer_location: None,
            location_cleared: false,
            alert_sounds: true,
            see_through_glass: false,
            show_tray: true,
            shell_extension_requested: false,
        }
    }
}

impl Preferences {
    /// Parse a preferences file, merging each valid field over the defaults. A
    /// missing or unreadable file is the defaults; a malformed field is only
    /// that field's default.
    pub fn parse(text: &str) -> Self {
        let Ok(root) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let Some(object) = root.as_object() else {
            return Self::default();
        };

        let mut preferences = Self::default();
        if let Some(Value::Bool(value)) = object.get("notifications") {
            preferences.notifications = Some(*value);
        }
        if let Some(Value::Bool(value)) = object.get("menuBarThroughput") {
            preferences.menu_bar_throughput = *value;
        }
        if let Some(Value::Bool(value)) = object.get("hideTrayIcon") {
            preferences.hide_tray_icon = *value;
        }
        if let Some(value) = object.get("trayIconStyle").and_then(Value::as_str) {
            preferences.tray_icon_style = match value {
                "template" => TrayIconStyle::Template,
                "outline" => TrayIconStyle::Outline,
                _ => TrayIconStyle::Original,
            };
        }
        if let Some(value) = object.get("windowBounds") {
            preferences.window_bounds = window_bounds(value);
        }
        if let Some(Value::String(value)) = object.get("routerAddress") {
            preferences.router_address = normalize_ip_address(value);
        }
        if let Some(value) = object.get("selfClientId").and_then(Value::as_u64) {
            preferences.self_client_id = u32::try_from(value).ok();
        }
        if let Some(value) = object.get("locale").cloned() {
            preferences.locale = serde_json::from_value::<Locale>(value).ok();
        }
        if let Some(value) = object.get("timeFormat").and_then(Value::as_str)
            && let Some(time_format) = TimeFormat::from_value(value)
        {
            preferences.time_format = time_format;
        }
        if let Some(value) = object.get("toolbarStyle").and_then(Value::as_str)
            && let Some(toolbar_style) = ToolbarStyle::from_value(value)
        {
            preferences.toolbar_style = toolbar_style;
        }
        if let Some(value) = object.get("theme").cloned()
            && let Ok(theme) = serde_json::from_value::<Theme>(value)
        {
            preferences.theme = theme;
        }
        if let Some(value) = object.get("observerLocation").cloned() {
            preferences.observer_location = serde_json::from_value::<ObserverLocation>(value)
                .ok()
                .filter(|location| location.is_valid());
        }
        if let Some(Value::Bool(value)) = object.get("locationCleared") {
            preferences.location_cleared = *value;
        }
        if let Some(Value::Bool(value)) = object.get("alertSounds") {
            preferences.alert_sounds = *value;
        }
        if let Some(Value::Bool(value)) = object.get("seeThroughGlass") {
            preferences.see_through_glass = *value;
        }
        if let Some(Value::Bool(value)) = object.get("showTray") {
            preferences.show_tray = *value;
        }
        if let Some(Value::Bool(value)) = object.get("shellExtensionRequested") {
            preferences.shell_extension_requested = *value;
        }
        preferences
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Preferences is serializable")
    }
}

fn window_bounds(value: &Value) -> Option<WindowBounds> {
    let object = value.as_object()?;
    let number = |key: &str| -> Option<i32> {
        object
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
    };
    Some(WindowBounds::new(
        number("x")?,
        number("y")?,
        number("width")?,
        number("height")?,
    ))
}

/// Literal IP addresses only, matching `core/ipAddress.ts`: a hostname would
/// resolve differently in each host, so it is not a routable setting.
pub fn normalize_ip_address(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unbracketed = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    if unbracketed.contains(':') && unbracketed.contains('.') {
        return None;
    }
    let without_zone = unbracketed.split('%').next().unwrap_or(unbracketed);
    without_zone
        .parse::<IpAddr>()
        .ok()
        .map(|address| address.to_string())
}

#[derive(Debug)]
pub struct PreferencesStore {
    path: PathBuf,
    preferences: Preferences,
}

impl PreferencesStore {
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let preferences = fs::read_to_string(&path)
            .map(|text| Preferences::parse(&text))
            .unwrap_or_default();
        Self { path, preferences }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> &Preferences {
        &self.preferences
    }

    /// Apply a change in memory and persist it. A failed write does not roll
    /// back the in-memory value: the current run keeps the choice, just as the
    /// Electron host does, and the next launch reverts.
    pub fn set(&mut self, change: impl FnOnce(&mut Preferences)) -> io::Result<()> {
        change(&mut self.preferences);
        self.save()
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, self.preferences.to_json())?;
        fs::rename(temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    // SPEC-SHELL-003: the theme is one of three choices, cycles in the renderer's
    // order, and is written to the shared preferences file.
    #[test]
    fn theme_cycle_is_the_typescript_order_and_persists() {
        assert_eq!(Theme::Light.next(), Theme::Dark);
        assert_eq!(Theme::Dark.next(), Theme::System);
        assert_eq!(Theme::System.next(), Theme::Light);

        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        assert_eq!(store.get().theme, Theme::Dark);

        store
            .set(|preferences| preferences.theme = preferences.theme.next())
            .expect("save theme");
        let reloaded = PreferencesStore::load(&path);
        assert_eq!(reloaded.get().theme, Theme::System);
    }

    // SPEC-SHEXT-003: the install request defaults off and persists.
    #[test]
    fn shell_extension_request_persists() {
        assert!(!Preferences::default().shell_extension_requested);
        assert!(
            Preferences::parse(r#"{"shellExtensionRequested": true}"#).shell_extension_requested
        );
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        store
            .set(|preferences| preferences.shell_extension_requested = true)
            .expect("save");
        assert!(
            PreferencesStore::load(&path)
                .get()
                .shell_extension_requested
        );
    }

    // SPEC-PILL-004: the tray shows by default and the choice persists.
    #[test]
    fn show_tray_defaults_on_and_persists() {
        assert!(Preferences::default().show_tray);
        assert!(!Preferences::parse(r#"{"showTray": false}"#).show_tray);
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        store
            .set(|preferences| preferences.show_tray = false)
            .expect("save");
        assert!(!PreferencesStore::load(&path).get().show_tray);
    }

    // SPEC-GLASS-004: see-through glass defaults off and persists.
    #[test]
    fn see_through_glass_defaults_off_and_persists() {
        assert!(!Preferences::default().see_through_glass);
        assert!(Preferences::parse(r#"{"seeThroughGlass": true}"#).see_through_glass);
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        store
            .set(|preferences| preferences.see_through_glass = true)
            .expect("save");
        assert!(PreferencesStore::load(&path).get().see_through_glass);
    }

    // SPEC-SOUND-003: alert sounds default on and persist as `alertSounds`.
    #[test]
    fn alert_sounds_default_on_and_persist() {
        assert!(Preferences::default().alert_sounds);
        assert!(!Preferences::parse(r#"{"alertSounds": false}"#).alert_sounds);
        assert!(Preferences::parse(r#"{"alertSounds": "no"}"#).alert_sounds);
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        store
            .set(|preferences| preferences.alert_sounds = false)
            .expect("save");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"alertSounds\": false")
        );
        assert!(!PreferencesStore::load(&path).get().alert_sounds);
    }

    #[test]
    fn invalid_fields_fall_back_individually() {
        let preferences = Preferences::parse(
            r#"{
              "notifications": "yes",
              "menuBarThroughput": false,
              "hideTrayIcon": true,
              "trayIconStyle": "template",
              "windowBounds": {"x": 1, "y": 2, "width": 0, "height": 4},
              "routerAddress": "  [2001:0DB8::1]  ",
              "selfClientId": 4294967296,
              "locale": "jp",
              "theme": "sepia",
              "timeFormat": "13h",
              "toolbarStyle": "left"
            }"#,
        );
        assert_eq!(preferences.notifications, None);
        assert!(!preferences.menu_bar_throughput);
        assert!(preferences.hide_tray_icon);
        assert_eq!(preferences.tray_icon_style, TrayIconStyle::Template);
        assert_eq!(
            preferences.window_bounds,
            Some(WindowBounds::new(1, 2, 0, 4))
        );
        assert_eq!(preferences.router_address.as_deref(), Some("2001:db8::1"));
        assert_eq!(preferences.self_client_id, None);
        assert_eq!(preferences.locale, None);
        assert_eq!(preferences.theme, Theme::Dark);
        assert_eq!(preferences.time_format, TimeFormat::Auto);
        assert_eq!(preferences.toolbar_style, ToolbarStyle::Dock);
    }

    // SPEC-TOOLBAR-001, SPEC-TOOLBAR-002: Dock is the default, valid values
    // round-trip, and unsupported values preserve the default.
    #[test]
    fn toolbar_style_defaults_validates_and_persists() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        assert_eq!(store.get().toolbar_style, ToolbarStyle::Dock);
        store
            .set(|preferences| preferences.toolbar_style = ToolbarStyle::Rail)
            .expect("persist toolbar style");
        let text = fs::read_to_string(&path).expect("read preferences");
        assert!(text.contains("\"toolbarStyle\": \"rail\""));
        assert_eq!(
            PreferencesStore::load(&path).get().toolbar_style,
            ToolbarStyle::Rail
        );
        assert_eq!(
            Preferences::parse(r#"{"toolbarStyle":"dock"}"#).toolbar_style,
            ToolbarStyle::Dock
        );
        assert_eq!(
            Preferences::parse(r#"{"toolbarStyle":"left"}"#).toolbar_style,
            ToolbarStyle::Dock
        );
    }

    // SPEC-TIME-001, SPEC-TIME-002: the shared choice defaults to automatic,
    // validates persisted values, and round-trips in the host preferences file.
    #[test]
    fn time_format_defaults_validates_and_persists() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        assert_eq!(store.get().time_format, TimeFormat::Auto);
        store
            .set(|preferences| preferences.time_format = TimeFormat::Hour24)
            .expect("persist time format");
        let text = fs::read_to_string(&path).expect("read preferences");
        assert!(text.contains("\"timeFormat\": \"24h\""));
        assert_eq!(
            PreferencesStore::load(&path).get().time_format,
            TimeFormat::Hour24
        );
        assert_eq!(
            Preferences::parse(r#"{"timeFormat":"12h"}"#).time_format,
            TimeFormat::Hour12
        );
    }

    // SPEC-SELF-002: self-client IDs persist independently and clearing writes null.
    #[test]
    fn file_round_trips_with_the_electron_field_names() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        store
            .set(|preferences| {
                preferences.notifications = Some(true);
                preferences.locale = Some(Locale::Fr);
                preferences.window_bounds = Some(WindowBounds::new(10, 20, 1500, 980));
                preferences.self_client_id = Some(0xffff_ffff);
            })
            .expect("save");

        let text = fs::read_to_string(&path).expect("read preferences");
        assert!(text.contains("\"menuBarThroughput\""));
        assert!(text.contains("\"windowBounds\""));
        assert!(text.contains("\"selfClientId\": 4294967295"));
        assert_eq!(PreferencesStore::load(&path).get(), store.get());
        store
            .set(|preferences| preferences.self_client_id = None)
            .expect("clear self device");
        let cleared = fs::read_to_string(&path).expect("read cleared preferences");
        assert!(cleared.contains("\"selfClientId\": null"));
        assert_eq!(PreferencesStore::load(&path).get().self_client_id, None);
    }

    #[test]
    fn malformed_files_are_defaults_not_a_broken_launch() {
        assert_eq!(Preferences::parse("{nope"), Preferences::default());
        assert_eq!(Preferences::parse("[]"), Preferences::default());
    }

    #[test]
    fn addresses_are_literal_and_canonical() {
        assert_eq!(
            normalize_ip_address(" 192.168.1.1 ").as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(
            normalize_ip_address("[2001:0db8::1]").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(normalize_ip_address("router.local"), None);
        assert_eq!(normalize_ip_address("::ffff:192.168.1.1"), None);
    }
}
