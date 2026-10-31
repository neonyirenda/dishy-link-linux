//! Installing the Dishylink GNOME Shell extension from Settings (spec 0192).
//! The files ship inside the app; the Shell is asked over D-Bus what it
//! thinks of them.

use std::path::{Path, PathBuf};

pub const UUID: &str = "dishylink@dishylink.app";

/// The extension as packaged in `packaging/gnome-shell-extension`.
const FILES: [(&str, &str); 4] = [
    (
        "metadata.json",
        include_str!(
            "../../../packaging/gnome-shell-extension/dishylink@dishylink.app/metadata.json"
        ),
    ),
    (
        "extension.js",
        include_str!(
            "../../../packaging/gnome-shell-extension/dishylink@dishylink.app/extension.js"
        ),
    ),
    (
        "stylesheet.css",
        include_str!(
            "../../../packaging/gnome-shell-extension/dishylink@dishylink.app/stylesheet.css"
        ),
    ),
    (
        "icons/dishylink-symbolic.svg",
        include_str!(
            "../../../packaging/gnome-shell-extension/dishylink@dishylink.app/icons/dishylink-symbolic.svg"
        ),
    ),
];

/// Where GNOME Shell looks for per-user extensions.
pub fn default_extensions_dir() -> PathBuf {
    gtk::glib::home_dir().join(".local/share/gnome-shell/extensions")
}

/// Write the extension under `extensions`, replacing an older copy.
pub fn install(extensions: &Path) -> std::io::Result<PathBuf> {
    let root = extensions.join(UUID);
    for (name, contents) in FILES {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }
    Ok(root)
}

pub fn is_installed(extensions: &Path) -> bool {
    extensions.join(UUID).join("metadata.json").exists()
}

/// The extension as the Settings row words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionStatus {
    /// Not a GNOME session: nothing to offer.
    Unsupported,
    NotInstalled,
    /// Files present, but the Shell has not loaded them (Wayland needs a login).
    NeedsLogin,
    Enabled,
    Disabled,
    OutOfDate,
    Failed,
}

/// From the Shell's `state` (its `ExtensionState`, `None` when it does not
/// know the extension) and whether the files are on disk.
pub fn status_for(gnome: bool, installed: bool, shell_state: Option<f64>) -> ExtensionStatus {
    if !gnome {
        return ExtensionStatus::Unsupported;
    }
    if !installed {
        return ExtensionStatus::NotInstalled;
    }
    // GNOME Shell's ExtensionState: 1 active, 2 inactive, 3 error,
    // 4 out of date, 6 initialised (loaded, never enabled), 7/8 changing.
    match shell_state.map(|state| state.round() as i64) {
        None => ExtensionStatus::NeedsLogin,
        Some(1 | 8) => ExtensionStatus::Enabled,
        Some(3) => ExtensionStatus::Failed,
        Some(4) => ExtensionStatus::OutOfDate,
        Some(_) => ExtensionStatus::Disabled,
    }
}

/// Whether the app should ask the Shell to enable it now.
pub fn should_enable(requested: bool, status: ExtensionStatus) -> bool {
    requested && status == ExtensionStatus::Disabled
}

pub fn is_gnome_session() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| {
        desktops
            .split(':')
            .any(|desktop| desktop.eq_ignore_ascii_case("GNOME"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SHEXT-001: install writes the packaged files, and a second
    // install replaces a stale copy.
    #[test]
    fn install_writes_the_packaged_extension() {
        let directory = tempfile::tempdir().expect("temp dir");
        assert!(!is_installed(directory.path()));
        let root = install(directory.path()).expect("installed");
        assert_eq!(root, directory.path().join(UUID));
        for (name, contents) in FILES {
            assert_eq!(
                std::fs::read_to_string(root.join(name)).unwrap(),
                contents,
                "{name}"
            );
        }
        assert!(is_installed(directory.path()));
        std::fs::write(root.join("extension.js"), "stale").unwrap();
        install(directory.path()).expect("updated");
        assert_eq!(
            std::fs::read_to_string(root.join("extension.js")).unwrap(),
            FILES[1].1
        );
    }

    // SPEC-SHEXT-002: the Shell's state and the files on disk decide the row.
    #[test]
    fn status_follows_the_shell() {
        assert_eq!(
            status_for(false, true, Some(1.0)),
            ExtensionStatus::Unsupported
        );
        assert_eq!(status_for(true, false, None), ExtensionStatus::NotInstalled);
        assert_eq!(status_for(true, true, None), ExtensionStatus::NeedsLogin);
        assert_eq!(status_for(true, true, Some(1.0)), ExtensionStatus::Enabled);
        assert_eq!(status_for(true, true, Some(2.0)), ExtensionStatus::Disabled);
        assert_eq!(status_for(true, true, Some(6.0)), ExtensionStatus::Disabled);
        assert_eq!(status_for(true, true, Some(3.0)), ExtensionStatus::Failed);
        assert_eq!(
            status_for(true, true, Some(4.0)),
            ExtensionStatus::OutOfDate
        );
        // Removed from disk while the Shell still remembers it.
        assert_eq!(
            status_for(true, false, Some(1.0)),
            ExtensionStatus::NotInstalled
        );
    }

    // SPEC-SHEXT-003: only a requested, known-but-off extension is enabled.
    #[test]
    fn enabling_needs_a_request_and_a_known_extension() {
        assert!(should_enable(true, ExtensionStatus::Disabled));
        assert!(!should_enable(false, ExtensionStatus::Disabled));
        assert!(!should_enable(true, ExtensionStatus::NeedsLogin));
        assert!(!should_enable(true, ExtensionStatus::Enabled));
    }
}
