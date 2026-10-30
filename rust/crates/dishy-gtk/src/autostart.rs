//! XDG autostart, the Linux replacement for Electron's login item.
//!
//! The entry launches the same binary with `--hidden`, so a login start becomes
//! a recorder in the tray rather than a window in the user's face.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How the autostart entry starts the app: this executable, or through
/// `flatpak run` when the app is sandboxed and its own path means nothing to
/// the host (spec 0171).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Executable(PathBuf),
    Flatpak(String),
}

impl Launch {
    /// The sandbox sets `FLATPAK_ID`; anywhere else, the running binary.
    pub fn current() -> io::Result<Self> {
        match std::env::var("FLATPAK_ID") {
            Ok(app_id) if !app_id.is_empty() => Ok(Self::Flatpak(app_id)),
            _ => std::env::current_exe().map(Self::Executable),
        }
    }

    fn exec(&self) -> String {
        match self {
            Self::Executable(path) => quote_exec(path),
            Self::Flatpak(app_id) => format!("flatpak run --command=dishy-gtk {app_id}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Autostart {
    path: PathBuf,
}

impl Autostart {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_installed(&self) -> bool {
        self.path.is_file()
    }

    /// Write the desktop entry atomically enough for a login-time read: a
    /// temp file in the same directory, then rename over the old entry.
    pub fn install(&self, launch: &Launch, app_name: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("desktop.tmp");
        fs::write(&temporary, desktop_entry(launch, app_name))?;
        fs::rename(temporary, &self.path)
    }

    /// Remove the entry. `false` means there was nothing to remove, which is a
    /// successful outcome for a toggle that is already off.
    pub fn remove(&self) -> io::Result<bool> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

pub fn desktop_entry(launch: &Launch, app_name: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={}\n\
         Exec={} --hidden\n\
         Icon=com.dishylink.app\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_value(app_name),
        launch.exec()
    )
}

/// One-line desktop-entry values cannot contain a newline or an unescaped
/// backslash; the app name comes from a translation, so keep a bad catalog from
/// turning the autostart file into a different entry.
fn desktop_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace(['\n', '\r'], " ")
}

/// Quote one `Exec` argument per the desktop-entry spec. Quoting even paths
/// without spaces keeps a path containing `%` or a quote from being read as a
/// field code or a malformed command.
fn quote_exec(executable: &Path) -> String {
    let text = executable.to_string_lossy();
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '\\' | '"' | '`' | '$' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            '\n' => quoted.push_str("\\n"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::paths::AUTOSTART_FILE;

    // SPEC-SHELL-006: start-at-login is an XDG autostart entry that launches
    // hidden, and removing it really removes it.
    #[test]
    fn installs_a_hidden_xdg_entry_and_removes_it() {
        let directory = tempdir().expect("temp dir");
        let autostart = Autostart::new(directory.path().join("autostart").join(AUTOSTART_FILE));
        assert!(!autostart.is_installed());

        autostart
            .install(
                &Launch::Executable("/opt/Dishylink App/dishy-gtk".into()),
                "Dishylink",
            )
            .expect("install autostart");
        assert!(autostart.is_installed());
        let entry = fs::read_to_string(autostart.path()).expect("read entry");
        assert!(entry.contains("Name=Dishylink"));
        assert!(entry.contains("Exec=\"/opt/Dishylink App/dishy-gtk\" --hidden"));
        assert!(entry.contains("X-GNOME-Autostart-enabled=true"));
        assert!(entry.contains("Type=Application"));

        assert!(autostart.remove().expect("remove autostart"));
        assert!(!autostart.is_installed());
        assert!(!autostart.remove().expect("already removed"));
    }

    // SPEC-PKG-003: inside the Flatpak sandbox the entry launches through
    // `flatpak run`, and is written to the host's autostart folder.
    #[test]
    fn a_sandboxed_entry_launches_through_flatpak_on_the_host() {
        let entry = desktop_entry(&Launch::Flatpak("com.dishylink.app".into()), "Dishylink");
        assert!(
            entry.contains("Exec=flatpak run --command=dishy-gtk com.dishylink.app --hidden\n")
        );
        assert_eq!(
            crate::paths::autostart_dir(Path::new("/sandbox/config"), Path::new("/home/me"), true),
            Path::new("/home/me/.config/autostart")
        );
        assert_eq!(
            crate::paths::autostart_dir(Path::new("/home/me/.cfg"), Path::new("/home/me"), false),
            Path::new("/home/me/.cfg/autostart")
        );
    }

    #[test]
    fn exec_quoting_covers_spaces_percent_and_quotes() {
        assert_eq!(
            desktop_entry(
                &Launch::Executable("/tmp/100%/dishy \"app\"".into()),
                "Dishylink"
            ),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Dishylink\n\
             Exec=\"/tmp/100%%/dishy \\\"app\\\"\" --hidden\n\
             Icon=com.dishylink.app\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
    }
}
