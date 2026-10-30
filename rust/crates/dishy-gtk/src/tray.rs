//! The StatusNotifierItem tray.
//!
//! Linux desktops are not consistent about tray hosts: GNOME needs an extension,
//! while KDE and most others provide `org.kde.StatusNotifierWatcher` directly.
//! Starting the tray is therefore best-effort. If it fails, the recorder thread
//! and the `GtkApplication` single-instance handshake still let a second launch
//! reopen the window; nothing about collection depends on this item existing.

use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
use ksni::{ToolTip, Tray};

use crate::i18n::Translator;
use crate::paths::APP_ID;
use crate::throughput::readout;

/// The Dishylink mark as a symbolic icon (spec 0188), so the panel can
/// recolour it.
const TRAY_ICON: &str = "com.dishylink.app-symbolic";

pub const TRAY_ICON_SVG: &str = include_str!("../assets/icons/com.dishylink.app-symbolic.svg");

/// Write the mark into an icon theme under `base` and return `base`, which the
/// StatusNotifier host then searches. A cache directory works in dev runs and
/// in the Flatpak alike.
pub fn install_tray_icon(base: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let apps = base.join("hicolor/scalable/apps");
    std::fs::create_dir_all(&apps)?;
    let file = apps.join(format!("{TRAY_ICON}.svg"));
    if std::fs::read_to_string(&file).ok().as_deref() != Some(TRAY_ICON_SVG) {
        std::fs::write(&file, TRAY_ICON_SVG)?;
    }
    Ok(base.to_path_buf())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Quit,
    SetNotifications(bool),
    SetAutostart(bool),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrayState {
    pub notifications: bool,
    pub autostart: bool,
}

pub struct DishyTray {
    commands: Sender<TrayCommand>,
    translator: Translator,
    state: TrayState,
    throughput: String,
    icon_theme_path: Option<std::path::PathBuf>,
}

impl DishyTray {
    pub fn new(
        commands: Sender<TrayCommand>,
        translator: Translator,
        state: TrayState,
        throughput: String,
    ) -> Self {
        Self {
            commands,
            translator,
            state,
            throughput,
            icon_theme_path: None,
        }
    }

    pub fn set_icon_theme_path(&mut self, path: std::path::PathBuf) {
        self.icon_theme_path = Some(path);
    }

    pub fn refresh(
        &mut self,
        state: TrayState,
        throughput: String,
        translator: Option<Translator>,
    ) {
        self.state = state;
        self.throughput = throughput;
        if let Some(translator) = translator {
            self.translator = translator;
        }
    }

    fn command(&self, command: TrayCommand) {
        let _ = self.commands.send(command);
    }

    fn app_name(&self) -> String {
        self.translator.text("desktop.appName")
    }

    fn item(&self, label: String, command: TrayCommand) -> MenuItem<Self> {
        StandardItem {
            label,
            activate: Box::new(move |tray: &mut Self| tray.command(command)),
            ..Default::default()
        }
        .into()
    }

    fn checkmark(&self, label: String, checked: bool, command: TrayCommand) -> MenuItem<Self> {
        CheckmarkItem {
            label,
            checked,
            activate: Box::new(move |tray: &mut Self| {
                // The menu is rebuilt from the host's preference after this
                // click; flipping here keeps the item responsive in the gap.
                match command {
                    TrayCommand::SetNotifications(on) => {
                        tray.state.notifications = !on;
                        tray.command(TrayCommand::SetNotifications(!on));
                    }
                    TrayCommand::SetAutostart(on) => {
                        tray.state.autostart = !on;
                        tray.command(TrayCommand::SetAutostart(!on));
                    }
                    TrayCommand::Open | TrayCommand::Quit => tray.command(command),
                }
            }),
            ..Default::default()
        }
        .into()
    }
}

impl Tray for DishyTray {
    fn id(&self) -> String {
        APP_ID.to_string()
    }

    fn title(&self) -> String {
        if self.throughput.is_empty() {
            self.app_name()
        } else {
            format!("{} — {}", self.app_name(), self.throughput)
        }
    }

    fn icon_name(&self) -> String {
        TRAY_ICON.to_string()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: self.icon_name(),
            title: self.app_name(),
            description: self.throughput.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.command(TrayCommand::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items = Vec::new();
        if !self.throughput.is_empty() {
            items.push(
                StandardItem {
                    label: self.throughput.clone(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            items.push(MenuItem::Separator);
        }
        items.extend([
            self.item(
                self.translator.text("desktop.gtkOpenApp"),
                TrayCommand::Open,
            ),
            MenuItem::Separator,
            self.checkmark(
                self.translator.text("desktop.notifyMe"),
                self.state.notifications,
                TrayCommand::SetNotifications(self.state.notifications),
            ),
            self.checkmark(
                self.translator.text("desktop.startAtLogin"),
                self.state.autostart,
                TrayCommand::SetAutostart(self.state.autostart),
            ),
            MenuItem::Separator,
            self.item(self.translator.text("desktop.gtkQuit"), TrayCommand::Quit),
        ]);
        items
    }
}

/// Start the tray, or return `None` when this session has no StatusNotifier
/// host. `assume_sni_available` keeps the service alive through a shell restart
/// instead of making a transient missing watcher fatal.
pub fn start(
    commands: Sender<TrayCommand>,
    translator: Translator,
    state: TrayState,
) -> Option<Handle<DishyTray>> {
    let mut tray = DishyTray::new(commands, translator, state, readout(0.0, 0.0));
    let base = gtk::glib::user_cache_dir().join("dishylink/icons");
    match install_tray_icon(&base) {
        Ok(path) => tray.set_icon_theme_path(path),
        Err(error) => tracing::warn!("tray icon not written, panel will use its default: {error}"),
    }
    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::warn!("tray unavailable, continuing headless: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::i18n::Locale;

    fn labels(tray: &DishyTray) -> Vec<String> {
        tray.menu()
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Standard(item) => Some(item.label),
                MenuItem::Checkmark(item) => Some(item.label),
                MenuItem::Separator | MenuItem::SubMenu(_) | MenuItem::RadioGroup(_) => None,
            })
            .collect()
    }

    // SPEC-SHELL-007: the tray carries the live rate, and its wording follows
    // the same catalogs as the window.
    #[test]
    fn tray_menu_uses_the_catalog_and_reports_throughput() {
        let (sender, receiver) = mpsc::channel();
        let tray = DishyTray::new(
            sender,
            Translator::new(Locale::Es),
            TrayState::default(),
            readout(23_500_000.0, 1_200_000.0),
        );
        let labels = labels(&tray);
        assert!(labels.iter().any(|label| label == "Abrir Dishylink Linux"));
        assert!(
            labels
                .iter()
                .any(|label| label == "Avisarme de las alertas")
        );
        assert!(
            labels
                .iter()
                .any(|label| label == "Iniciar al iniciar sesión")
        );
        assert!(
            labels
                .iter()
                .any(|label| label == "Salir de Dishylink Linux")
        );
        assert_eq!(labels[0], "↓23.5 Mb/s ↑1.2 Mb/s");
        assert!(tray.title().contains("↓23.5 Mb/s"));
        assert_eq!(receiver.try_recv().ok(), None);
    }

    // SPEC-CTRL-005: hiding the throughput readout leaves the tray useful and
    // removes the empty disabled row instead of painting a blank line.
    #[test]
    fn tray_without_throughput_keeps_only_actions() {
        let (sender, _receiver) = mpsc::channel();
        let tray = DishyTray::new(
            sender,
            Translator::new(Locale::En),
            TrayState::default(),
            String::new(),
        );
        let labels = labels(&tray);
        assert_eq!(labels[0], "Open Dishylink Linux");
        assert!(!labels.iter().any(|label| label.is_empty()));
        assert_eq!(tray.title(), "Dishylink Linux");
    }

    #[test]
    fn menu_clicks_are_handed_to_the_gtk_host() {
        let (sender, receiver) = mpsc::channel();
        let mut tray = DishyTray::new(
            sender,
            Translator::new(Locale::En),
            TrayState::default(),
            readout(0.0, 0.0),
        );
        tray.activate(0, 0);
        assert_eq!(
            receiver.try_recv().expect("open command"),
            TrayCommand::Open
        );
    }

    // SPEC-ICON-003: the tray shows the symbolic Dishylink mark, written where
    // its icon theme path points.
    #[test]
    fn the_tray_uses_the_symbolic_mark() {
        let directory = tempfile::tempdir().expect("temp dir");
        let base = install_tray_icon(directory.path()).expect("written");
        let written = std::fs::read_to_string(
            base.join("hicolor/scalable/apps/com.dishylink.app-symbolic.svg"),
        )
        .expect("icon file");
        assert_eq!(written, TRAY_ICON_SVG);
        assert!(
            !written.contains("stroke"),
            "symbolic icons recolour fills only"
        );
        let (sender, _) = mpsc::channel();
        let mut tray = DishyTray::new(
            sender,
            Translator::new(Locale::En),
            TrayState::default(),
            String::new(),
        );
        tray.set_icon_theme_path(base.clone());
        assert_eq!(tray.icon_name(), "com.dishylink.app-symbolic");
        assert_eq!(tray.icon_theme_path(), base.to_string_lossy());
    }
}
