//! Launch flags and the window-activation rule.
//!
//! `GtkApplication` provides the single-instance handshake: a second process
//! sends `activate` to the first and exits before it can start a recorder. What
//! that activation means depends on why the first process started — a login
//! launch stays in the tray, while any later activation is the user asking for
//! the window.

use std::ffi::OsString;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Launch {
    pub hidden: bool,
}

impl Launch {
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            hidden: args.into_iter().any(|arg| arg.as_ref() == "--hidden"),
        }
    }

    /// Remove the app-specific flag before handing the remaining arguments to
    /// GTK, which would otherwise reject an unknown option. GApplication's
    /// argument slice is UTF-8, so non-UTF-8 arguments are made lossy here; the
    /// flags this app defines are all ASCII.
    pub fn gtk_args<I>(args: I) -> Vec<String>
    where
        I: IntoIterator<Item = OsString>,
    {
        args.into_iter()
            .filter(|arg| arg != &OsString::from("--hidden"))
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    Show,
    StayHidden,
}

/// Distinguishes the initial `activate` from a second launch.
#[derive(Debug, Clone, Copy)]
pub struct ActivationGate {
    hidden_at_startup: bool,
    activated: bool,
}

impl ActivationGate {
    pub fn new(hidden_at_startup: bool) -> Self {
        Self {
            hidden_at_startup,
            activated: false,
        }
    }

    pub fn activate(&mut self) -> Activation {
        let first = !self.activated;
        self.activated = true;
        if first && self.hidden_at_startup {
            Activation::StayHidden
        } else {
            Activation::Show
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_flag_is_parsed_and_not_forwarded_to_gtk() {
        let launch = Launch::from_args(["dishy-gtk", "--hidden"]);
        assert!(launch.hidden);
        assert_eq!(
            Launch::gtk_args([OsString::from("dishy-gtk"), OsString::from("--hidden")]),
            vec!["dishy-gtk".to_string()]
        );
    }

    // SPEC-SHELL-002: a later activation (the second launch, or the desktop
    // shortcut while the tray copy runs) brings up the first instance's window.
    // SPEC-SHELL-007: no tray host is not fatal either; the same activation path
    // reopens the window.
    #[test]
    fn a_second_activation_shows_the_first_instance() {
        let mut login_launch = ActivationGate::new(true);
        assert_eq!(login_launch.activate(), Activation::StayHidden);
        assert_eq!(login_launch.activate(), Activation::Show);

        let mut ordinary_launch = ActivationGate::new(false);
        assert_eq!(ordinary_launch.activate(), Activation::Show);
    }
}
