//! Where the Linux app keeps its state.
//!
//! These paths deliberately match the Electron app's `app.getPath("userData")`
//! on Linux (`~/.config/Dishylink`), so a Rust launch sees the same preferences
//! and history as a TypeScript one. That is the point of the shared on-disk
//! contract: neither implementation gets a private copy of the user's data.

use std::path::{Path, PathBuf};

/// The application id used by the desktop entry, D-Bus single-instance name, and
/// the autostart filename.
pub const APP_ID: &str = "com.dishylink.app";
/// Electron's `app.setName("Dishylink")`, which is what makes userData's final
/// path component capitalized on Linux.
pub const APP_DIR: &str = "Dishylink";
/// The filename under `$XDG_CONFIG_HOME/autostart`.
pub const AUTOSTART_FILE: &str = "com.dishylink.app.desktop";

/// Where start-at-login entries go. Inside the Flatpak sandbox
/// `XDG_CONFIG_HOME` is the app's private folder, which the session never
/// reads, so the entry goes to the host's `~/.config/autostart` (granted by
/// the manifest) instead.
pub fn autostart_dir(config_home: &Path, home: &Path, sandboxed: bool) -> PathBuf {
    if sandboxed {
        home.join(".config").join("autostart")
    } else {
        config_home.join("autostart")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    autostart_dir: PathBuf,
    config_home: PathBuf,
    app_dir: PathBuf,
    data_dir: PathBuf,
}

impl HostPaths {
    /// Resolve the same locations the Electron host uses, with the historian's
    /// existing `HISTORIAN_DATA_DIR` override still honoured for development and
    /// for running beside a separately installed collector.
    pub fn discover() -> Self {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"));
        let data_override = std::env::var_os("HISTORIAN_DATA_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let sandboxed = std::env::var_os("FLATPAK_ID").is_some_and(|id| !id.is_empty());
        let mut paths = Self::from_parts(config_home, data_override);
        paths.autostart_dir = autostart_dir(&paths.config_home, &home_dir(), sandboxed);
        paths
    }

    pub fn from_parts(config_home: PathBuf, data_override: Option<PathBuf>) -> Self {
        let app_dir = config_home.join(APP_DIR);
        let data_dir = data_override.unwrap_or_else(|| app_dir.join("data"));
        Self {
            autostart_dir: config_home.join("autostart"),
            config_home,
            app_dir,
            data_dir,
        }
    }

    pub fn config_home(&self) -> &Path {
        &self.config_home
    }

    /// `~/.config/Dishylink`, holding preferences and the data directory.
    pub fn app_dir(&self) -> &Path {
        &self.app_dir
    }

    pub fn preferences_file(&self) -> PathBuf {
        self.app_dir.join("preferences.json")
    }

    /// The cached Starlink element catalogue. It is about 2 MB and refetched
    /// after six hours, so keeping it saves a download on most launches.
    pub fn tle_cache_file(&self) -> PathBuf {
        self.app_dir.join("starlink-tle.json")
    }

    /// `~/.config/Dishylink/data`, or the historian's environment override.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `~/.config/autostart/com.dishylink.app.desktop`.
    pub fn autostart_file(&self) -> PathBuf {
        self.autostart_dir.join(AUTOSTART_FILE)
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn electron_compatible_locations_and_history_override() {
        let paths = HostPaths::from_parts(PathBuf::from("/tmp/config"), None);
        assert_eq!(
            paths.preferences_file(),
            PathBuf::from("/tmp/config/Dishylink/preferences.json")
        );
        assert_eq!(paths.data_dir(), Path::new("/tmp/config/Dishylink/data"));
        assert_eq!(
            paths.autostart_file(),
            PathBuf::from("/tmp/config/autostart/com.dishylink.app.desktop")
        );

        let overridden = HostPaths::from_parts(
            PathBuf::from("/tmp/config"),
            Some(PathBuf::from("/tmp/history")),
        );
        assert_eq!(overridden.data_dir(), Path::new("/tmp/history"));
    }
}
