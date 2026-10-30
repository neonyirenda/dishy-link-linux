//! The five existing JSON catalogs, shared byte-for-byte with the TypeScript app.
//!
//! The files are included at compile time rather than copied into `rust/`. A
//! translation changed for the web UI is therefore the text this shell paints on
//! its next build, and there is no second catalog to leave stale.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    En,
    Es,
    Fr,
    De,
    Pt,
}

pub const SUPPORTED_LOCALES: [Locale; 5] =
    [Locale::En, Locale::Es, Locale::Fr, Locale::De, Locale::Pt];

impl Locale {
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::De => "de",
            Self::Pt => "pt",
        }
    }

    /// The language's own name, as the Electron language row shows it.
    pub fn native_label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::De => "Deutsch",
            Self::Pt => "Português",
        }
    }
}

/// The first supported language in a preference-ordered list of BCP 47 or POSIX
/// tags, exactly as `core/i18n/locale.ts` does it.
pub fn locale_from_tags<I, S>(tags: I) -> Locale
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for tag in tags {
        let language = tag
            .as_ref()
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match language.as_str() {
            "en" => return Locale::En,
            "es" => return Locale::Es,
            "fr" => return Locale::Fr,
            "de" => return Locale::De,
            "pt" => return Locale::Pt,
            _ => {}
        }
    }
    Locale::En
}

/// The desktop's language: `LANGUAGE` first, then the POSIX category variables.
pub fn detect_system_locale() -> Locale {
    let mut tags: Vec<String> = Vec::new();
    if let Ok(language) = std::env::var("LANGUAGE") {
        tags.extend(
            language
                .split(':')
                .filter(|tag| !tag.is_empty())
                .map(str::to_string),
        );
    }
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(tag) = std::env::var(key)
            && !tag.is_empty()
        {
            tags.push(tag);
        }
    }
    locale_from_tags(tags)
}

static CATALOGS: LazyLock<HashMap<Locale, HashMap<String, String>>> = LazyLock::new(|| {
    let mut catalogs = HashMap::new();
    for locale in SUPPORTED_LOCALES {
        let source = match locale {
            Locale::En => include_str!("../locales/en.json"),
            Locale::Es => include_str!("../locales/es.json"),
            Locale::Fr => include_str!("../locales/fr.json"),
            Locale::De => include_str!("../locales/de.json"),
            Locale::Pt => include_str!("../locales/pt.json"),
        };
        let root: Value = serde_json::from_str(source).expect("bundled translation catalog");
        let mut flat = HashMap::new();
        flatten(&root, "", &mut flat);
        catalogs.insert(locale, flat);
    }
    catalogs
});

fn flatten(value: &Value, prefix: &str, out: &mut HashMap<String, String>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, &path, out);
            }
        }
        Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        // i18next arrays are plural forms the shell does not use yet.
        _ => {}
    }
}

#[derive(Debug, Clone)]
pub struct Translator {
    locale: Locale,
}

impl Translator {
    pub fn new(locale: Locale) -> Self {
        Self { locale }
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn text(&self, key: &str) -> String {
        self.text_with(key, &[])
    }

    /// Look one key up, interpolating `{{name}}` placeholders. A key missing from
    /// a translation falls back to English; a key missing everywhere is returned
    /// verbatim so a layout mistake shows up instead of silently reading blank.
    pub fn text_with(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let catalog = CATALOGS
            .get(&self.locale)
            .expect("all supported locales are bundled");
        let template = catalog
            .get(key)
            .or_else(|| CATALOGS.get(&Locale::En).and_then(|en| en.get(key)))
            .map(String::as_str)
            .unwrap_or(key);
        interpolate(template, vars)
    }
}

fn interpolate(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        // The shared catalogs occasionally carry an ICU argument shape such as
        // `{{count, number}}`; the variable name is still `count`. The GTK
        // translator does not implement ICU number formatting, but substituting
        // the raw value is better than printing the placeholder.
        let name = after[..end].split(',').next().unwrap_or_default().trim();
        if let Some((_, value)) = vars.iter().find(|(key, _)| *key == name) {
            out.push_str(value);
        } else {
            out.push_str(&rest[start..start + 2 + end + 2]);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {

    // SPEC-NAME-001: the GTK app is "Dishylink Linux" in every locale, on its
    // own keys; Electron's shared strings keep their name.
    #[test]
    fn the_gtk_app_is_dishylink_linux() {
        for locale in [Locale::En, Locale::De, Locale::Es, Locale::Fr, Locale::Pt] {
            let translator = Translator::new(locale);
            assert_eq!(translator.text("desktop.appName"), "Dishylink Linux");
            for key in [
                "desktop.gtkWindowTitle",
                "desktop.gtkOpenApp",
                "desktop.gtkQuit",
            ] {
                assert!(
                    translator.text(key).contains("Dishylink Linux"),
                    "{locale:?} {key}"
                );
            }
            for key in ["desktop.windowTitle", "desktop.openApp", "desktop.quit"] {
                assert!(
                    !translator.text(key).contains("Dishylink Linux"),
                    "{locale:?} {key}"
                );
            }
        }
    }

    use super::*;

    const SHELL_KEYS: &[&str] = &[
        "navigation.dashboard",
        "navigation.dataUsage",
        "navigation.network",
        "navigation.alerts",
        "network.connectThroughCloud",
        "network.rosterViaAccount",
        "network.cloudRosterNote",
        "network.liveFromRouter",
        "network.connectingEllipsis",
        "errors.connectAccountFirst",
        "errors.accountUnreachableCheckInternet",
        "alerts.system.monthlyDataLimit.ok",
        "alerts.system.monthlyDataLimit.firing",
        "devices.setMonthlyLimit",
        "devices.editMonthlyLimit",
        "devices.monthlyLimitTitle",
        "devices.monthlyLimitPrompt",
        "devices.monthlyLimitPlaceholder",
        "devices.monthlyLimitProgress",
        "devices.monthlyLimitReached",
        "devices.monthlyLimitRefused",
        "devices.monthlyLimitUnavailable",
        "devices.removeMonthlyLimit",
        "devices.clearAllIncludingLimits",
        "devices.deleteUsageAndLimitFor",
        "network.renameNode",
        "network.meshRenameChoose",
        "network.meshRenameNeedsNetwork",
        "network.meshRenameHelp",
        "network.meshNamePlaceholder",
        "network.thisDevice",
        "network.unknownDevice",
        "navigation.settings",
        "settings.debugData",
        "settings.debugDataDesc",
        "settings.copy",
        "settings.copied",
        "settings.copyFailed",
        "settings.timeFormat",
        "settings.timeFormatDesc",
        "settings.timeFormatAuto",
        "settings.timeFormat12h",
        "settings.timeFormat24h",
        "settings.timeFormatAm",
        "settings.timeFormatPm",
        "settings.appToolbar",
        "settings.appToolbarDesc",
        "settings.dock",
        "settings.rail",
        "settings.yourDevice",
        "settings.yourDeviceDesc",
        "settings.deviceSaveFailed",
        "common.none",
        "common.notConnected",
        "desktop.gtkWindowTitle",
        "desktop.appName",
        "desktop.gtkOpenApp",
        "desktop.notifyMe",
        "desktop.startAtLogin",
        "desktop.gtkQuit",
        "status.colorTheme",
        "status.switchTheme",
        "theme.light",
        "theme.dark",
        "theme.system",
        "notifications.enabledTitle",
        "notifications.enabledBody",
        "ranges.15m",
        "ranges.1h",
        "ranges.6h",
        "metrics.searchingHint",
        "obstruction.waiting",
        "obstruction.pauseRotation",
        "obstruction.resumeRotation",
        "obstruction.showUnmapped",
        "obstruction.hideUnmapped",
        "obstruction.legendUnmapped",
        "obstruction.legendClear",
        "obstruction.legendPartial",
        "obstruction.legendObstructions",
        "obstruction.observedFor",
        "obstruction.observedHours",
        "satellite.resetView",
        "satellite.hideDome",
        "satellite.showDome",
        "satellite.controlsHintWindow",
        "satellite.noGtkGl",
        "time.justNow",
        "time.minutesAgo",
        "time.hoursAgo_one",
        "time.hoursAgo_other",
        "time.daysAgo_one",
        "time.daysAgo_other",
    ];

    // SPEC-SHELL-004: every shell string comes from the same five catalogs the
    // TypeScript app ships, and every locale can word each one.
    #[test]
    fn shell_strings_exist_in_every_shared_catalog() {
        for locale in SUPPORTED_LOCALES {
            let translator = Translator::new(locale);
            for key in SHELL_KEYS {
                assert_ne!(
                    translator.text(key),
                    *key,
                    "{} is missing from {}",
                    key,
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn shell_text_is_locale_specific_not_english_borrowed() {
        let english = Translator::new(Locale::En);
        let spanish = Translator::new(Locale::Es);
        assert_eq!(english.text("desktop.openApp"), "Open Dishylink");
        assert_eq!(spanish.text("desktop.openApp"), "Abrir Dishylink");
        assert_ne!(
            english.text("desktop.windowTitle"),
            spanish.text("desktop.windowTitle")
        );
    }

    #[test]
    fn tags_match_the_typescript_rule_and_default_to_english() {
        assert_eq!(locale_from_tags(["pt-BR"]), Locale::Pt);
        assert_eq!(locale_from_tags(["fr_CA.UTF-8"]), Locale::Fr);
        assert_eq!(locale_from_tags(["nl-NL", "de-DE"]), Locale::De);
        assert_eq!(locale_from_tags(["zz-ZZ"]), Locale::En);
        assert_eq!(locale_from_tags(std::iter::empty::<&str>()), Locale::En);
    }

    #[test]
    fn interpolation_matches_i18next_placeholders() {
        let translator = Translator::new(Locale::En);
        let text = translator.text_with(
            "status.switchTheme",
            &[("theme", "dark"), ("next", "system")],
        );
        assert_eq!(text, "Color theme: dark. Switch to system.");

        // An unknown placeholder is preserved, matching i18next.
        assert_eq!(interpolate("Hello {{name}}", &[]), "Hello {{name}}");

        // Number-shaped ICU arguments still resolve by their variable name.
        assert_eq!(
            interpolate("{{count, number}} patches", &[("count", "1234")]),
            "1234 patches"
        );
    }
}
