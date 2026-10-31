//! Where the Linux app keeps its state: `~/.config/Dishylink`, shared with
//! `dishy record` through `dishy_core::paths` (SPEC-RECORDER-008).

use std::path::{Path, PathBuf};

pub use dishy_core::paths::APP_DIR;
use dishy_core::paths::{config_home, data_dir, data_dir_override, home_dir};

/// The application id used by the desktop entry, D-Bus single-instance name, and
/// the autostart filename.
pub const APP_ID: &str = "com.dishylink.app";
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
    /// Resolve from the environment; `HISTORIAN_DATA_DIR` moves the history.
    pub fn discover() -> Self {
        let config_home = config_home();
        let data_override = data_dir_override();
        let sandboxed = std::env::var_os("FLATPAK_ID").is_some_and(|id| !id.is_empty());
        let mut paths = Self::from_parts(config_home, data_override);
        paths.autostart_dir = autostart_dir(&paths.config_home, &home_dir(), sandboxed);
        paths
    }

    pub fn from_parts(config_home: PathBuf, data_override: Option<PathBuf>) -> Self {
        let app_dir = config_home.join(APP_DIR);
        let data_dir = data_dir(&config_home, data_override);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_and_history_override() {
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
