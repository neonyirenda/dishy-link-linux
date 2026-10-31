//! Where the app and the headless recorder keep their state (SPEC-RECORDER-008).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The folder under `$XDG_CONFIG_HOME` holding preferences and history.
pub const APP_DIR: &str = "Dishylink";
/// Points both the app and `dishy record` at another history directory.
pub const DATA_DIR_ENV: &str = "HISTORIAN_DATA_DIR";

/// `$XDG_CONFIG_HOME`, or `$HOME/.config` when it is unset or empty.
pub fn config_home() -> PathBuf {
    config_home_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// `$HOME`, or the working directory when it is unset or empty.
pub fn home_dir() -> PathBuf {
    non_empty(std::env::var_os("HOME")).unwrap_or_else(|| PathBuf::from("."))
}

/// The history directory: the override when one is set, otherwise
/// `<config_home>/Dishylink/data`.
pub fn data_dir(config_home: &Path, override_dir: Option<PathBuf>) -> PathBuf {
    override_dir.unwrap_or_else(|| config_home.join(APP_DIR).join("data"))
}

/// `$HISTORIAN_DATA_DIR`, when set and not empty.
pub fn data_dir_override() -> Option<PathBuf> {
    non_empty(std::env::var_os(DATA_DIR_ENV))
}

fn config_home_from(xdg: Option<OsString>, home: Option<OsString>) -> PathBuf {
    non_empty(xdg).unwrap_or_else(|| {
        non_empty(home)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
    })
}

fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-RECORDER-008
    #[test]
    fn history_defaults_under_the_config_home_unless_overridden() {
        let home = config_home_from(None, Some("/home/me".into()));
        assert_eq!(home, Path::new("/home/me/.config"));
        assert_eq!(
            config_home_from(Some("".into()), Some("/home/me".into())),
            Path::new("/home/me/.config")
        );
        assert_eq!(
            config_home_from(Some("/xdg".into()), Some("/home/me".into())),
            Path::new("/xdg")
        );
        assert_eq!(
            data_dir(&home, None),
            Path::new("/home/me/.config/Dishylink/data")
        );
        assert_eq!(
            data_dir(&home, Some(PathBuf::from("/srv/history"))),
            Path::new("/srv/history")
        );
    }
}
