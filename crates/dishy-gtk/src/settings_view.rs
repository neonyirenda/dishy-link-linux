//! The settings page: one-shot maintenance, cloud-backed router settings, and
//! the host preferences GTK owns.
//!
//! LAN maintenance writes use the recorder thread; cloud router writes send
//! typed intent to the trusted cloud worker. Neither path creates a device client
//! in this module or retries a write after an ambiguous response.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Once;
use std::sync::mpsc::Sender;

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use dishy_cloud::dish_config::{DishConfigUpdate, SnowMeltMode};
use dishy_cloud::router_config::{RouterConfigUpdate, validate_router_config_update};
use dishy_core::control::{ControlAction, ControlTarget};
use dishy_core::network::{NetworkSnapshot, WifiConfig};
use dishy_core::telemetry::DishStatus;

use crate::i18n::{Locale, SUPPORTED_LOCALES, Translator};
use crate::preferences::{TimeFormat, ToolbarStyle};
use crate::recorder::{ControlReport, RecorderCommand};

/// Why a Wi-Fi password change cannot be sent (spec 0185).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiPasswordRefusal {
    Mismatch,
    Length,
}

pub fn wifi_password_refusal(password: &str, confirm: &str) -> Option<WifiPasswordRefusal> {
    if password != confirm {
        return Some(WifiPasswordRefusal::Mismatch);
    }
    // The cloud crate owns the passphrase rule, so there is one source for it.
    let update = RouterConfigUpdate::WifiPassword {
        password: password.into(),
    };
    validate_router_config_update(&update)
        .is_err()
        .then_some(WifiPasswordRefusal::Length)
}

/// Officially supported subnet presets the cloud router form can dispatch
/// (SPEC-CLOUDCFG-007). The GTK combo shows a placeholder item at index 0;
/// presets start at index 1.
const SUBNET_PRESETS: [&str; 8] = [
    "192.168.1.1/24",
    "192.168.2.1/24",
    "192.168.3.1/24",
    "192.168.4.1/24",
    "10.0.0.1/16",
    "10.1.0.1/16",
    "10.2.0.1/16",
    "10.3.0.1/16",
];

const CSS: &str = r#"
.dishy-settings-tabs {
  margin: 12px;
}
.dishy-settings-online {
  color: #2f9e63;
  font-weight: 600;
}
.dishy-settings-offline {
  color: #c0392b;
  font-weight: 600;
}
.dishy-settings-value {
  font-family: monospace;
  font-size: 12px;
}
.dishy-maintenance-action {
  min-width: 96px;
  min-height: 36px;
  padding: 4px 12px;
}
.dishy-form-action {
  min-width: 80px;
  min-height: 36px;
  padding: 4px 10px;
}
"#;

/// Host preferences the App tab edits. Device writes travel separately on the
/// recorder command channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    SetNotifications(bool),
    SetAutostart(bool),
    SetTrayThroughput(bool),
    SetSeeThroughGlass(bool),
    SetShowTray(bool),
    InstallShellExtension,
    SetRouterAddress(Option<String>),
    UpdateRouterConfig(RouterConfigUpdate),
    UpdateDishConfig(DishConfigUpdate),
    SetLocale(Locale),
    SetTimeFormat(TimeFormat),
    SetToolbarStyle(ToolbarStyle),
    SetSelfClientId(Option<u32>),
}

/// The values the switches should show when the page is built or rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettingsState {
    pub locale: Locale,
    pub notifications: bool,
    pub autostart: bool,
    pub tray_throughput: bool,
    pub see_through_glass: bool,
    pub show_tray: bool,
    pub router_address: Option<String>,
    pub time_format: TimeFormat,
    pub toolbar_style: ToolbarStyle,
    pub self_client_id: Option<u32>,
}

#[derive(Clone)]
pub struct SettingsPage {
    inner: Rc<Inner>,
}

struct Inner {
    root: adw::ToastOverlay,
    self_weak: std::rc::Weak<Inner>,
    translator: Translator,
    commands: RefCell<Option<Sender<RecorderCommand>>>,
    actions: RefCell<Option<Sender<SettingsAction>>>,
    busy: Cell<bool>,
    updating: Cell<bool>,
    router_reachable: Cell<Option<bool>>,
    account_connected: Cell<bool>,
    dish_present: Cell<bool>,
    actuators: Cell<bool>,
    stowed: Cell<bool>,
    hardware_value: gtk::Label,
    software_value: gtk::Label,
    starlink_connecting: adw::ActionRow,
    stow_row: adw::ActionRow,
    stow_button: gtk::Button,
    router_value: gtk::Label,
    router_address: gtk::Entry,
    router_address_error: gtk::Label,
    router_config: RefCell<Option<WifiConfig>>,
    dns_entries: Vec<gtk::Entry>,
    dns_save_button: gtk::Button,
    dns_error: gtk::Label,
    subnet_row: adw::ComboRow,
    subnet_password: gtk::PasswordEntry,
    subnet_save_button: gtk::Button,
    subnet_error: gtk::Label,
    wifi_row: adw::ActionRow,
    wifi_new: gtk::PasswordEntry,
    wifi_confirm: gtk::PasswordEntry,
    wifi_save_button: gtk::Button,
    wifi_error: gtk::Label,
    /// A Wi-Fi password write is in flight, so its result clears the fields.
    wifi_pending: Cell<bool>,
    mesh_row: adw::ComboRow,
    mesh_model: gtk::StringList,
    mesh_name: gtk::Entry,
    mesh_save_button: gtk::Button,
    mesh_error: gtk::Label,
    mesh_nodes: RefCell<Vec<(String, String)>>,
    mesh_pending_name: RefCell<Option<(String, String)>>,
    dns_busy: Cell<bool>,
    dish_busy: Cell<bool>,
    dish_config_loaded: Cell<bool>,
    snow_melt_row: adw::ComboRow,
    sleep_row: adw::SwitchRow,
    update_window_row: adw::ComboRow,
    update_window_model: gtk::StringList,
    defer_updates_row: adw::SwitchRow,
    bypass_state: Cell<Option<bool>>,
    bypass_pending: Cell<Option<bool>>,
    bypass_button: gtk::Button,
    bypass_row: adw::ActionRow,
    dish_buttons: Vec<gtk::Button>,
    diagnostics_button: gtk::Button,
    diagnostics_busy: Cell<bool>,
    latest_status: RefCell<Option<DishStatus>>,
    latest_dish_config: RefCell<Option<serde_json::Value>>,
    router_buttons: Vec<gtk::Button>,
    language_row: adw::ComboRow,
    time_format_row: adw::ComboRow,
    toolbar_style_row: adw::ComboRow,
    self_device_row: adw::ComboRow,
    self_device_model: gtk::StringList,
    self_device_choices: RefCell<Vec<SelfDeviceChoice>>,
    self_device_id: Cell<Option<u32>>,
    self_devices: RefCell<Vec<(u32, String)>>,
    notifications_row: adw::SwitchRow,
    autostart_row: adw::SwitchRow,
    tray_row: adw::SwitchRow,
    glass_row: adw::SwitchRow,
    show_tray_row: adw::SwitchRow,
    shell_extension_row: adw::ActionRow,
    shell_extension_button: gtk::Button,
    last_toast: RefCell<Option<String>>,
    last_action: RefCell<Option<ControlAction>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelfDeviceChoice {
    None,
    Unavailable,
    Client(u32),
}

impl SettingsPage {
    pub fn new(translator: &Translator) -> Self {
        install_css();

        let inner = Rc::new_cyclic(|weak| {
            let starlink_page = adw::PreferencesPage::new();
            let status_group = adw::PreferencesGroup::builder()
                .title(translator.text("common.starlink"))
                .build();
            let hardware_value = value_label();
            let software_value = value_label();
            let hardware_row = adw::ActionRow::builder()
                .title(translator.text("account.hardwareVersion"))
                .build();
            hardware_row.add_suffix(&hardware_value);
            let software_row = adw::ActionRow::builder()
                .title(translator.text("account.softwareVersion"))
                .build();
            software_row.add_suffix(&software_value);
            let starlink_connecting = adw::ActionRow::builder()
                .title(translator.text("status.connecting"))
                .build();
            status_group.add(&hardware_row);
            status_group.add(&software_row);
            status_group.add(&starlink_connecting);
            starlink_page.add(&status_group);

            let dish_config_group = adw::PreferencesGroup::builder()
                .title(translator.text("settings.maintenance"))
                .build();
            let snow_melt_model = gtk::StringList::new(&[
                &translator.text("common.automatic"),
                &translator.text("common.alwaysOn"),
                &translator.text("common.off"),
            ]);
            let snow_melt_row = adw::ComboRow::builder()
                .title(translator.text("settings.snowMelt"))
                .model(&snow_melt_model)
                .build();
            snow_melt_row.set_widget_name("dish-snow-melt");
            dish_config_group.add(&snow_melt_row);
            let sleep_row = adw::SwitchRow::builder()
                .title(translator.text("settings.sleepSchedule"))
                .subtitle(translator.text("settings.sleepScheduleOff"))
                .build();
            sleep_row.set_widget_name("dish-sleep-schedule");
            dish_config_group.add(&sleep_row);
            let update_window_labels =
                formatted_update_window_labels(TimeFormat::Auto, translator.locale());
            let update_window_model = gtk::StringList::new(
                &update_window_labels
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
            let update_window_row = adw::ComboRow::builder()
                .title(translator.text("settings.softwareUpdates"))
                .model(&update_window_model)
                .build();
            update_window_row.set_widget_name("dish-update-window");
            dish_config_group.add(&update_window_row);
            let defer_updates_row = adw::SwitchRow::builder()
                .title(translator.text("settings.deferUpdates"))
                .subtitle(translator.text("settings.deferUpdatesDesc"))
                .build();
            defer_updates_row.set_widget_name("dish-defer-updates");
            dish_config_group.add(&defer_updates_row);
            starlink_page.add(&dish_config_group);

            let maintenance_group = adw::PreferencesGroup::builder()
                .title(translator.text("settings.maintenance"))
                .build();
            let mut dish_buttons = Vec::new();
            let mut add_dish_row = |action: ControlAction| {
                let copy = control_copy(action, translator);
                let button = action_button(&copy.verb, action.destructive());
                let row = adw::ActionRow::builder()
                    .title(copy.title)
                    .subtitle(copy.detail)
                    .subtitle_lines(0)
                    .build();
                row.add_suffix(&button);
                maintenance_group.add(&row);
                dish_buttons.push(button);
            };
            add_dish_row(ControlAction::ClearObstructionMap);
            add_dish_row(ControlAction::RebootDish);
            add_dish_row(ControlAction::FactoryResetDish);

            let stow_copy = control_copy(ControlAction::StowDish { unstow: false }, translator);
            let stow_button = action_button(&stow_copy.verb, true);
            let stow_row = adw::ActionRow::builder()
                .title(stow_copy.title)
                .subtitle(stow_copy.detail)
                .subtitle_lines(0)
                .build();
            stow_row.add_suffix(&stow_button);
            stow_row.set_visible(false);
            maintenance_group.add(&stow_row);
            dish_buttons.push(stow_button.clone());

            let diagnostics_button = action_button(&translator.text("settings.copy"), false);
            diagnostics_button.set_widget_name("diagnostics-copy");
            let diagnostics_row = adw::ActionRow::builder()
                .title(translator.text("settings.debugData"))
                .subtitle(translator.text("settings.debugDataDesc"))
                .subtitle_lines(0)
                .build();
            diagnostics_row.add_suffix(&diagnostics_button);
            maintenance_group.add(&diagnostics_row);
            starlink_page.add(&maintenance_group);

            let router_page = adw::PreferencesPage::new();
            let router_group = adw::PreferencesGroup::builder()
                .title(translator.text("common.router"))
                .build();
            let router_address = gtk::Entry::new();
            router_address.set_widget_name("router-address-entry");
            router_address.set_placeholder_text(Some(
                &dishy_core::gateway::default_router_address().to_string(),
            ));
            configure_entry(&router_address);
            let save_router_address =
                form_action_button(&translator.text("common.save"), "router-address-save");
            let address_row = adw::ActionRow::builder()
                .title(translator.text("routerAddress.title"))
                .subtitle(translator.text_with(
                    "routerAddress.tip",
                    &[(
                        "fallback",
                        &dishy_core::gateway::default_router_address().to_string(),
                    )],
                ))
                .subtitle_lines(0)
                .build();
            address_row.add_suffix(&router_address);
            address_row.add_suffix(&save_router_address);
            router_group.add(&address_row);
            let router_address_error = gtk::Label::new(None);
            router_address_error.set_xalign(0.0);
            router_address_error.add_css_class("error");
            router_group.add(&router_address_error);
            let mut dns_entries = Vec::new();
            for index in 0..4 {
                let entry = gtk::Entry::new();
                entry.set_widget_name(&format!("router-dns-{index}"));
                entry.set_input_purpose(gtk::InputPurpose::Url);
                configure_entry(&entry);
                let server_label = if index == 0 {
                    translator.text("dns.primary")
                } else {
                    translator.text("dns.backup")
                };
                let row = adw::ActionRow::builder()
                    .title(translator.text_with("dns.serverField", &[("label", &server_label)]))
                    .build();
                row.add_suffix(&entry);
                router_group.add(&row);
                dns_entries.push(entry);
            }
            let dns_save_button =
                form_action_button(&translator.text("common.save"), "router-dns-save");
            let dns_save_row = adw::ActionRow::builder()
                .title(translator.text("dns.title"))
                .subtitle(translator.text("dns.caption"))
                .subtitle_lines(0)
                .build();
            dns_save_row.add_suffix(&dns_save_button);
            router_group.add(&dns_save_row);
            let dns_error = gtk::Label::new(None);
            dns_error.set_xalign(0.0);
            dns_error.add_css_class("error");
            router_group.add(&dns_error);
            // libadwaita's AdwComboRow (GtkDropDown) cannot represent "no
            // selection", so index 0 is a placeholder row; presets start at 1.
            let subnet_choose = translator.text("subnet.choose");
            let subnet_items: Vec<&str> = std::iter::once(subnet_choose.as_str())
                .chain(SUBNET_PRESETS.iter().copied())
                .collect();
            let subnet_model = gtk::StringList::new(&subnet_items);
            let subnet_row = adw::ComboRow::builder()
                .title(translator.text("subnet.title"))
                .subtitle(translator.text("subnet.tip"))
                .subtitle_lines(0)
                .model(&subnet_model)
                .build();
            subnet_row.set_widget_name("router-subnet-select");
            subnet_row.set_selected(0);
            router_group.add(&subnet_row);
            let subnet_password = gtk::PasswordEntry::new();
            subnet_password.set_widget_name("router-subnet-password");
            configure_widget(subnet_password.upcast_ref());
            subnet_password
                .set_placeholder_text(Some(&translator.text("subnet.passwordPlaceholder")));
            let subnet_password_row = adw::ActionRow::builder()
                .title(translator.text("subnet.wifiPassword"))
                .build();
            subnet_password_row.add_suffix(&subnet_password);
            router_group.add(&subnet_password_row);
            let subnet_save_button =
                form_action_button(&translator.text("subnet.move"), "router-subnet-save");
            let subnet_save_row = adw::ActionRow::builder()
                .title(translator.text("subnet.title"))
                .subtitle(translator.text("subnet.caption"))
                .subtitle_lines(0)
                .build();
            subnet_save_row.add_suffix(&subnet_save_button);
            router_group.add(&subnet_save_row);
            let subnet_error = gtk::Label::new(None);
            subnet_error.set_xalign(0.0);
            subnet_error.add_css_class("error");
            router_group.add(&subnet_error);
            // Spec 0185: the Wi-Fi password, through the signed-in account.
            let password_field = |name: &str, placeholder: &str| {
                let entry = gtk::PasswordEntry::new();
                entry.set_widget_name(name);
                entry.set_show_peek_icon(true);
                configure_widget(entry.upcast_ref());
                entry.set_placeholder_text(Some(placeholder));
                entry
            };
            let wifi_new = password_field(
                "router-wifi-password-new",
                &translator.text("wifiPassword.newPassword"),
            );
            let wifi_confirm = password_field(
                "router-wifi-password-confirm",
                &translator.text("wifiPassword.confirmPassword"),
            );
            let wifi_new_row = adw::ActionRow::builder()
                .title(translator.text("wifiPassword.newPassword"))
                .build();
            wifi_new_row.add_suffix(&wifi_new);
            router_group.add(&wifi_new_row);
            let wifi_confirm_row = adw::ActionRow::builder()
                .title(translator.text("wifiPassword.confirmPassword"))
                .build();
            wifi_confirm_row.add_suffix(&wifi_confirm);
            router_group.add(&wifi_confirm_row);
            let wifi_save_button = form_action_button(
                &translator.text("wifiPassword.save"),
                "router-wifi-password-save",
            );
            let wifi_row = adw::ActionRow::builder()
                .title(translator.text("wifiPassword.title"))
                .subtitle(translator.text("wifiPassword.signInRequired"))
                .subtitle_lines(0)
                .build();
            wifi_row.add_suffix(&wifi_save_button);
            router_group.add(&wifi_row);
            let wifi_error = gtk::Label::new(None);
            wifi_error.set_xalign(0.0);
            wifi_error.add_css_class("error");
            router_group.add(&wifi_error);
            let mesh_choose = translator.text("network.meshRenameChoose");
            let mesh_model = gtk::StringList::new(&[&mesh_choose]);
            let mesh_row = adw::ComboRow::builder()
                .title(translator.text("network.renameNode"))
                .subtitle(translator.text("network.meshRenameNeedsNetwork"))
                .subtitle_lines(0)
                .model(&mesh_model)
                .build();
            mesh_row.set_widget_name("router-mesh-select");
            mesh_row.set_selected(0);
            router_group.add(&mesh_row);
            let mesh_name = gtk::Entry::new();
            mesh_name.set_widget_name("router-mesh-name");
            mesh_name.set_placeholder_text(Some(&translator.text("network.meshNamePlaceholder")));
            configure_entry(&mesh_name);
            let mesh_name_row = adw::ActionRow::builder()
                .title(translator.text("network.renameNode"))
                .build();
            mesh_name_row.add_suffix(&mesh_name);
            router_group.add(&mesh_name_row);
            let mesh_save_button =
                form_action_button(&translator.text("common.save"), "router-mesh-save");
            let mesh_save_row = adw::ActionRow::builder()
                .title(translator.text("network.renameNode"))
                .build();
            mesh_save_row.add_suffix(&mesh_save_button);
            router_group.add(&mesh_save_row);
            let mesh_error = gtk::Label::new(None);
            mesh_error.set_xalign(0.0);
            mesh_error.add_css_class("error");
            router_group.add(&mesh_error);
            let bypass_button = gtk::Button::with_label(&translator.text("bypass.turnOff"));
            bypass_button.set_widget_name("router-bypass-toggle");
            let bypass_row = adw::ActionRow::builder()
                .title(translator.text("bypass.title"))
                .subtitle(translator.text("bypass.unknownState"))
                .subtitle_lines(0)
                .build();
            bypass_row.add_suffix(&bypass_button);
            router_group.add(&bypass_row);
            let router_value = value_label();
            let router_row = adw::ActionRow::builder()
                .title(translator.text("common.router"))
                .build();
            router_row.add_suffix(&router_value);
            router_group.add(&router_row);
            router_page.add(&router_group);

            let router_maintenance = adw::PreferencesGroup::builder()
                .title(translator.text("settings.maintenance"))
                .build();
            let mut router_buttons = Vec::new();
            for action in [
                ControlAction::RebootRouter,
                ControlAction::FactoryResetRouter,
            ] {
                let copy = control_copy(action, translator);
                let button = action_button(&copy.verb, action.destructive());
                button.set_widget_name(if action == ControlAction::FactoryResetRouter {
                    "router-factory-reset"
                } else {
                    "router-reboot"
                });
                let row = adw::ActionRow::builder()
                    .title(copy.title)
                    .subtitle(copy.detail)
                    .subtitle_lines(0)
                    .build();
                row.add_suffix(&button);
                router_maintenance.add(&row);
                router_buttons.push(button);
            }
            router_page.add(&router_maintenance);

            let app_page = adw::PreferencesPage::new();
            let app_group = adw::PreferencesGroup::builder()
                .title(translator.text("common.app"))
                .build();
            let labels: Vec<&str> = SUPPORTED_LOCALES
                .iter()
                .map(|locale| locale.native_label())
                .collect();
            let language_model = gtk::StringList::new(&labels);
            let language_row = adw::ComboRow::builder()
                .title(translator.text("settings.language"))
                .subtitle(translator.text("settings.languageDesc"))
                .model(&language_model)
                .build();
            language_row.set_selected(locale_index(translator.locale()));
            app_group.add(&language_row);

            let time_format_model = gtk::StringList::new(&[
                &translator.text("settings.timeFormatAuto"),
                &translator.text("settings.timeFormat12h"),
                &translator.text("settings.timeFormat24h"),
            ]);
            let time_format_row = adw::ComboRow::builder()
                .title(translator.text("settings.timeFormat"))
                .subtitle(translator.text("settings.timeFormatDesc"))
                .model(&time_format_model)
                .build();
            time_format_row.set_widget_name("app-time-format");
            time_format_row.set_selected(0);
            app_group.add(&time_format_row);

            let toolbar_style_model = gtk::StringList::new(&[
                &translator.text("settings.dock"),
                &translator.text("settings.rail"),
            ]);
            let toolbar_style_row = adw::ComboRow::builder()
                .title(translator.text("settings.appToolbar"))
                .subtitle(translator.text("settings.appToolbarDesc"))
                .model(&toolbar_style_model)
                .build();
            toolbar_style_row.set_widget_name("app-toolbar-style");
            toolbar_style_row.set_selected(0);
            app_group.add(&toolbar_style_row);

            let self_device_model = gtk::StringList::new(&[&translator.text("common.none")]);
            let self_device_row = adw::ComboRow::builder()
                .title(translator.text("settings.yourDevice"))
                .subtitle(translator.text("settings.yourDeviceDesc"))
                .model(&self_device_model)
                .build();
            self_device_row.set_widget_name("app-self-device");
            self_device_row.set_selected(0);
            app_group.add(&self_device_row);

            let notifications_row = adw::SwitchRow::builder()
                .title(translator.text("desktop.notifyMe"))
                .subtitle(translator.text("notifications.enabledBody"))
                .build();
            app_group.add(&notifications_row);
            let autostart_row = adw::SwitchRow::builder()
                .title(translator.text("desktop.startAtLogin"))
                .build();
            app_group.add(&autostart_row);
            let tray_surface = translator.text("settings.tray");
            let tray_row = adw::SwitchRow::builder()
                .title(translator.text_with("settings.throughputIn", &[("surface", &tray_surface)]))
                .subtitle(
                    translator
                        .text_with("settings.throughputInDesc", &[("surface", &tray_surface)]),
                )
                .build();
            // Spec 0189: the tray can be switched off for the Shell pill.
            let show_tray_row = adw::SwitchRow::builder()
                .title(translator.text("settings.showTray"))
                .subtitle(translator.text("settings.showTrayDesc"))
                .subtitle_lines(0)
                .build();
            show_tray_row.set_widget_name("app-show-tray");
            app_group.add(&show_tray_row);
            // Spec 0192: install the Quick Settings pill without a terminal.
            let shell_extension_button = form_action_button(
                &translator.text("settings.shellExtensionInstall"),
                "app-shell-extension-install",
            );
            let shell_extension_row = adw::ActionRow::builder()
                .title(translator.text("settings.shellExtension"))
                .subtitle(translator.text("settings.shellExtensionDesc"))
                .subtitle_lines(0)
                .build();
            shell_extension_row.set_widget_name("app-shell-extension");
            shell_extension_row.add_suffix(&shell_extension_button);
            app_group.add(&shell_extension_row);
            app_group.add(&tray_row);
            // Spec 0190: opt-in, since it only looks right over a blur.
            let glass_row = adw::SwitchRow::builder()
                .title(translator.text("settings.seeThroughGlass"))
                .subtitle(translator.text("settings.seeThroughGlassDesc"))
                .subtitle_lines(0)
                .build();
            glass_row.set_widget_name("app-see-through-glass");
            app_group.add(&glass_row);
            app_page.add(&app_group);

            // SPEC-CTRL-001: the three tabs are the settings page's whole
            // surface, and every heading above came from the shared catalogs.
            let stack = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .transition_duration(150)
                .vexpand(true)
                .build();
            stack.add_titled(
                &starlink_page,
                Some("starlink"),
                &translator.text("common.starlink"),
            );
            stack.add_titled(
                &router_page,
                Some("router"),
                &translator.text("common.router"),
            );
            stack.add_titled(&app_page, Some("app"), &translator.text("common.app"));
            let switcher = gtk::StackSwitcher::builder()
                .stack(&stack)
                .halign(gtk::Align::Center)
                .build();
            switcher.set_widget_name("settings-tabs");
            switcher.add_css_class("dishy-settings-tabs");

            let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
            page.append(&switcher);
            page.append(&stack);
            let root = adw::ToastOverlay::new();
            root.set_child(Some(&page));

            let inner = Inner {
                root,
                self_weak: weak.clone(),
                translator: translator.clone(),
                commands: RefCell::new(None),
                actions: RefCell::new(None),
                busy: Cell::new(false),
                updating: Cell::new(false),
                router_reachable: Cell::new(None),
                account_connected: Cell::new(false),
                dish_present: Cell::new(false),
                actuators: Cell::new(false),
                stowed: Cell::new(false),
                hardware_value,
                software_value,
                starlink_connecting,
                stow_row,
                stow_button,
                router_value,
                router_address,
                router_address_error,
                router_config: RefCell::new(None),
                dns_entries,
                dns_save_button,
                dns_error,
                subnet_row,
                subnet_password,
                subnet_save_button: subnet_save_button.clone(),
                subnet_error,
                wifi_row,
                wifi_new,
                wifi_confirm,
                wifi_save_button: wifi_save_button.clone(),
                wifi_error,
                wifi_pending: Cell::new(false),
                mesh_row: mesh_row.clone(),
                mesh_model,
                mesh_name: mesh_name.clone(),
                mesh_save_button: mesh_save_button.clone(),
                mesh_error,
                mesh_nodes: RefCell::new(Vec::new()),
                mesh_pending_name: RefCell::new(None),
                dns_busy: Cell::new(false),
                dish_busy: Cell::new(false),
                dish_config_loaded: Cell::new(false),
                snow_melt_row,
                sleep_row,
                update_window_row,
                update_window_model,
                defer_updates_row,
                bypass_state: Cell::new(None),
                bypass_pending: Cell::new(None),
                bypass_button,
                bypass_row,
                dish_buttons,
                diagnostics_button: diagnostics_button.clone(),
                diagnostics_busy: Cell::new(false),
                latest_status: RefCell::new(None),
                latest_dish_config: RefCell::new(None),
                router_buttons,
                language_row,
                time_format_row: time_format_row.clone(),
                toolbar_style_row: toolbar_style_row.clone(),
                self_device_row: self_device_row.clone(),
                self_device_model,
                self_device_choices: RefCell::new(vec![SelfDeviceChoice::None]),
                self_device_id: Cell::new(None),
                self_devices: RefCell::new(Vec::new()),
                notifications_row,
                autostart_row,
                tray_row,
                glass_row,
                show_tray_row,
                shell_extension_row,
                shell_extension_button: shell_extension_button.clone(),
                last_toast: RefCell::new(None),
                last_action: RefCell::new(None),
            };

            for (index, action) in [
                ControlAction::ClearObstructionMap,
                ControlAction::RebootDish,
                ControlAction::FactoryResetDish,
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(button) = inner.dish_buttons.get(index) {
                    wire_control(button, weak, action);
                }
            }
            if let Some(button) = inner.dish_buttons.get(3) {
                // The stow button's action depends on the current dish state.
                let weak = weak.clone();
                button.connect_clicked(move |_| {
                    if let Some(inner) = weak.upgrade() {
                        let action = ControlAction::StowDish {
                            unstow: !inner.stowed.get(),
                        };
                        inner.confirm(action);
                    }
                });
            }
            let weak_diagnostics = weak.clone();
            diagnostics_button.connect_clicked(move |_| {
                if let Some(inner) = weak_diagnostics.upgrade() {
                    inner.request_diagnostics();
                }
            });
            wire_control(&inner.router_buttons[0], weak, ControlAction::RebootRouter);
            wire_control(
                &inner.router_buttons[1],
                weak,
                ControlAction::FactoryResetRouter,
            );

            let weak_language = weak.clone();
            inner.language_row.connect_selected_notify(move |row| {
                if let Some(inner) = weak_language.upgrade()
                    && !inner.updating.get()
                    && let Some(locale) = SUPPORTED_LOCALES.get(row.selected() as usize)
                {
                    inner.send_action(SettingsAction::SetLocale(*locale));
                }
            });
            let weak_time_format = weak.clone();
            inner.time_format_row.connect_selected_notify(move |row| {
                if let Some(inner) = weak_time_format.upgrade()
                    && !inner.updating.get()
                {
                    let format = match row.selected() {
                        1 => TimeFormat::Hour12,
                        2 => TimeFormat::Hour24,
                        _ => TimeFormat::Auto,
                    };
                    inner.send_action(SettingsAction::SetTimeFormat(format));
                }
            });
            let weak_toolbar_style = weak.clone();
            inner.toolbar_style_row.connect_selected_notify(move |row| {
                if let Some(inner) = weak_toolbar_style.upgrade()
                    && !inner.updating.get()
                {
                    let style = if row.selected() == 1 {
                        ToolbarStyle::Rail
                    } else {
                        ToolbarStyle::Dock
                    };
                    inner.send_action(SettingsAction::SetToolbarStyle(style));
                }
            });
            let weak_self_device = weak.clone();
            inner.self_device_row.connect_selected_notify(move |row| {
                if let Some(inner) = weak_self_device.upgrade()
                    && !inner.updating.get()
                {
                    inner.select_self_device(row.selected());
                }
            });
            let weak_notifications = weak.clone();
            inner.notifications_row.connect_active_notify(move |row| {
                if let Some(inner) = weak_notifications.upgrade()
                    && !inner.updating.get()
                {
                    inner.send_action(SettingsAction::SetNotifications(row.is_active()));
                }
            });
            let weak_autostart = weak.clone();
            inner.autostart_row.connect_active_notify(move |row| {
                if let Some(inner) = weak_autostart.upgrade()
                    && !inner.updating.get()
                {
                    inner.send_action(SettingsAction::SetAutostart(row.is_active()));
                }
            });
            let weak_tray = weak.clone();
            inner.tray_row.connect_active_notify(move |row| {
                if let Some(inner) = weak_tray.upgrade()
                    && !inner.updating.get()
                {
                    inner.send_action(SettingsAction::SetTrayThroughput(row.is_active()));
                }
            });
            let weak_show_tray = weak.clone();
            inner.show_tray_row.connect_active_notify(move |row| {
                if let Some(inner) = weak_show_tray.upgrade()
                    && !inner.updating.get()
                {
                    inner.send_action(SettingsAction::SetShowTray(row.is_active()));
                }
            });
            let weak_extension = weak.clone();
            shell_extension_button.connect_clicked(move |_| {
                if let Some(inner) = weak_extension.upgrade() {
                    inner.send_action(SettingsAction::InstallShellExtension);
                }
            });
            let weak_glass = weak.clone();
            inner.glass_row.connect_active_notify(move |row| {
                if let Some(inner) = weak_glass.upgrade()
                    && !inner.updating.get()
                {
                    inner.send_action(SettingsAction::SetSeeThroughGlass(row.is_active()));
                }
            });
            let weak_address = weak.clone();
            save_router_address.connect_clicked(move |_| {
                if let Some(inner) = weak_address.upgrade() {
                    let draft = inner.router_address.text().to_string();
                    let address = crate::preferences::normalize_ip_address(&draft);
                    if !draft.trim().is_empty() && address.is_none() {
                        inner
                            .router_address_error
                            .set_label(&inner.translator.text("routerAddress.refusedInvalid"));
                        return;
                    }
                    inner.router_address_error.set_label("");
                    inner.send_action(SettingsAction::SetRouterAddress(address));
                }
            });
            let weak_dns = weak.clone();
            inner.dns_save_button.connect_clicked(move |_| {
                if let Some(inner) = weak_dns.upgrade() {
                    let raw = inner
                        .dns_entries
                        .iter()
                        .map(|entry| entry.text().to_string())
                        .collect::<Vec<_>>();
                    match normalize_dns_fields(&raw) {
                        Ok(nameservers) => {
                            inner.dns_error.set_label("");
                            inner.dns_busy.set(true);
                            inner.refresh_dns_controls();
                            if !inner.send_action(SettingsAction::UpdateRouterConfig(
                                RouterConfigUpdate::CustomDns { nameservers },
                            )) {
                                inner.dns_busy.set(false);
                                inner
                                    .dns_error
                                    .set_label(&inner.translator.text("errors.cloudNotStarted"));
                                inner.refresh_dns_controls();
                            }
                        }
                        Err(()) => inner
                            .dns_error
                            .set_label(&inner.translator.text("errors.invalidDnsServer")),
                    }
                }
            });
            let weak_subnet = weak.clone();
            subnet_save_button.connect_clicked(move |_| {
                let Some(inner) = weak_subnet.upgrade() else {
                    return;
                };
                if inner.dns_busy.get() {
                    return;
                }
                let Some(subnet) = inner
                    .subnet_row
                    .selected()
                    .checked_sub(1)
                    .and_then(|index| SUBNET_PRESETS.get(index as usize))
                    .copied()
                else {
                    inner
                        .subnet_error
                        .set_label(&inner.translator.text("subnet.choose"));
                    return;
                };
                let update = RouterConfigUpdate::Subnet {
                    subnet: subnet.into(),
                    password: inner.subnet_password.text().to_string(),
                };
                if validate_router_config_update(&update).is_err() {
                    inner.subnet_error.set_label(
                        &inner
                            .translator
                            .text_with("subnet.refusedPassword", &[("min", "8"), ("max", "63")]),
                    );
                    return;
                }
                inner.subnet_error.set_label("");
                inner.confirm_subnet(update, subnet);
            });
            let weak_wifi = weak.clone();
            wifi_save_button.connect_clicked(move |_| {
                let Some(inner) = weak_wifi.upgrade() else {
                    return;
                };
                if inner.dns_busy.get() || !inner.account_connected.get() {
                    return;
                }
                let password = inner.wifi_new.text().to_string();
                match wifi_password_refusal(&password, &inner.wifi_confirm.text()) {
                    Some(WifiPasswordRefusal::Mismatch) => inner
                        .wifi_error
                        .set_label(&inner.translator.text("wifiPassword.mismatch")),
                    Some(WifiPasswordRefusal::Length) => inner.wifi_error.set_label(
                        &inner
                            .translator
                            .text_with("subnet.refusedPassword", &[("min", "8"), ("max", "63")]),
                    ),
                    None => {
                        inner.wifi_error.set_label("");
                        inner.confirm_wifi_password(password);
                    }
                }
            });
            let weak_bypass = weak.clone();
            inner.bypass_button.connect_clicked(move |_| {
                if let Some(inner) = weak_bypass.upgrade() {
                    inner.confirm_bypass();
                }
            });
            let weak_mesh_row = weak.clone();
            mesh_row.connect_selected_notify(move |row| {
                if let Some(inner) = weak_mesh_row.upgrade() {
                    inner.select_mesh_node(row.selected());
                }
            });
            let weak_mesh_name = weak.clone();
            mesh_name.connect_changed(move |_| {
                if let Some(inner) = weak_mesh_name.upgrade() {
                    inner.refresh_mesh_controls();
                }
            });
            let weak_mesh_save = weak.clone();
            mesh_save_button.connect_clicked(move |_| {
                if let Some(inner) = weak_mesh_save.upgrade() {
                    inner.save_mesh_name();
                }
            });
            let weak_snow = weak.clone();
            inner.snow_melt_row.connect_selected_notify(move |row| {
                let Some(inner) = weak_snow.upgrade() else {
                    return;
                };
                if inner.updating.get() || inner.dish_busy.get() {
                    return;
                }
                let mode = match row.selected() {
                    1 => SnowMeltMode::AlwaysOn,
                    2 => SnowMeltMode::AlwaysOff,
                    _ => SnowMeltMode::Auto,
                };
                inner.send_dish_update(DishConfigUpdate {
                    snow_melt_mode: Some(mode),
                    ..Default::default()
                });
            });
            let weak_sleep = weak.clone();
            inner.sleep_row.connect_active_notify(move |row| {
                let Some(inner) = weak_sleep.upgrade() else {
                    return;
                };
                if inner.updating.get() || inner.dish_busy.get() {
                    return;
                }
                let update = if row.is_active() {
                    DishConfigUpdate {
                        power_save_mode: Some(true),
                        power_save_start_minutes: Some(60),
                        power_save_duration_minutes: Some(360),
                        ..Default::default()
                    }
                } else {
                    DishConfigUpdate {
                        power_save_mode: Some(false),
                        ..Default::default()
                    }
                };
                inner.send_dish_update(update);
            });
            let weak_window = weak.clone();
            inner.update_window_row.connect_selected_notify(move |row| {
                let Some(inner) = weak_window.upgrade() else {
                    return;
                };
                if inner.updating.get() || inner.dish_busy.get() {
                    return;
                }
                let hour = [3, 9, 15, 21]
                    .get(row.selected() as usize)
                    .copied()
                    .unwrap_or(3);
                inner.send_dish_update(DishConfigUpdate {
                    swupdate_reboot_hour: Some(hour),
                    ..Default::default()
                });
            });
            let weak_defer = weak.clone();
            inner.defer_updates_row.connect_active_notify(move |row| {
                let Some(inner) = weak_defer.upgrade() else {
                    return;
                };
                if inner.updating.get() || inner.dish_busy.get() {
                    return;
                }
                inner.send_dish_update(DishConfigUpdate {
                    swupdate_three_day_deferral_enabled: Some(row.is_active()),
                    ..Default::default()
                });
            });

            inner.refresh_controls();
            inner
        });

        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    pub fn set_command_sender(&self, sender: Option<Sender<RecorderCommand>>) {
        *self.inner.commands.borrow_mut() = sender;
        self.inner.refresh_controls();
    }

    pub fn set_action_sender(&self, sender: Option<Sender<SettingsAction>>) {
        *self.inner.actions.borrow_mut() = sender;
        self.inner.refresh_dns_controls();
        self.inner.refresh_bypass_controls();
        self.inner.refresh_dish_controls();
        self.inner.refresh_mesh_controls();
    }

    /// Reuse the already-fetched cloud telemetry for bypass status.
    pub fn set_bypass_state(&self, bypassed: Option<bool>) {
        self.inner.bypass_state.set(bypassed);
        self.inner.refresh_bypass_controls();
    }

    /// Word the Shell extension's state and offer the matching action.
    pub fn set_shell_extension_status(&self, status: crate::shell_extension::ExtensionStatus) {
        use crate::shell_extension::ExtensionStatus;
        let translator = &self.inner.translator;
        let (state, action) = match status {
            ExtensionStatus::Unsupported => ("settings.shellExtensionUnsupported", None),
            ExtensionStatus::NotInstalled => (
                "settings.shellExtensionNotInstalled",
                Some("settings.shellExtensionInstall"),
            ),
            ExtensionStatus::NeedsLogin => ("settings.shellExtensionNeedsLogin", None),
            ExtensionStatus::Enabled => (
                "settings.shellExtensionEnabled",
                Some("settings.shellExtensionUpdate"),
            ),
            ExtensionStatus::Disabled => (
                "settings.shellExtensionDisabled",
                Some("settings.shellExtensionEnable"),
            ),
            ExtensionStatus::OutOfDate => (
                "settings.shellExtensionOutOfDate",
                Some("settings.shellExtensionUpdate"),
            ),
            ExtensionStatus::Failed => (
                "settings.shellExtensionFailed",
                Some("settings.shellExtensionUpdate"),
            ),
        };
        self.inner.shell_extension_row.set_subtitle(&format!(
            "{} {}",
            translator.text("settings.shellExtensionDesc"),
            translator.text(state)
        ));
        let button = &self.inner.shell_extension_button;
        button.set_visible(action.is_some());
        if let Some(action) = action {
            button.set_label(&translator.text(action));
        }
    }

    pub fn set_account_connected(&self, connected: bool) {
        self.inner.account_connected.set(connected);
        self.inner.refresh_controls();
        self.inner.refresh_dns_controls();
        self.inner.refresh_mesh_controls();
    }

    /// Seed mesh rename choices from the cached network reply only. This never
    /// asks the router for another roster or config read.
    pub fn set_network_snapshot(&self, snapshot: Option<&NetworkSnapshot>) {
        if let Some(snapshot) = snapshot
            && let Some(config) = snapshot.config.as_ref()
            && self.inner.router_config.borrow().as_ref() != Some(config)
        {
            self.inner.set_router_config(config);
        }
        let devices = snapshot
            .filter(|snapshot| snapshot.reachable)
            .into_iter()
            .flat_map(|snapshot| snapshot.devices.iter())
            .filter(|device| device.is_client)
            .filter_map(|device| {
                let client_id = u32::try_from(device.client_id?).ok()?;
                let label = device
                    .name
                    .as_deref()
                    .or(device.hostname.as_deref())
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        if device.mac_address.is_empty() {
                            self.inner.translator.text("network.unknownDevice")
                        } else {
                            device.mac_address.clone()
                        }
                    });
                let address = device.ip_address.as_deref().unwrap_or_default();
                let label = if address.is_empty() {
                    label
                } else {
                    format!("{label} · {address}")
                };
                Some((client_id, label))
            })
            .collect();
        self.inner.set_self_devices(devices);

        let nodes = snapshot
            .and_then(|snapshot| {
                let config = snapshot.config.as_ref()?;
                Some(
                    snapshot
                        .nodes
                        .iter()
                        .filter_map(|node| {
                            if node.role.as_deref() == Some("CONTROLLER") {
                                return None;
                            }
                            let id = node.device_id.as_deref()?;
                            if !id.starts_with("Router-")
                                || id.len() == "Router-".len()
                                || !config.mesh_configs.contains_key(id)
                            {
                                return None;
                            }
                            Some((id.to_owned(), config.mesh_name(id).unwrap_or(id).to_owned()))
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        self.inner.set_mesh_nodes(nodes);
    }

    /// Report the one-shot account-backed router write without sending the
    /// credential or cloud client into this GTK module.
    pub fn set_cloud_write_result(&self, result: Result<(), String>) {
        self.inner.dns_busy.set(false);
        let pending_mesh_name = self.inner.mesh_pending_name.borrow_mut().take();
        if result.is_ok()
            && let Some((device_id, display_name)) = pending_mesh_name
        {
            let updated = {
                let mut nodes = self.inner.mesh_nodes.borrow_mut();
                if let Some((_, name)) = nodes.iter_mut().find(|(id, _)| id == &device_id) {
                    *name = display_name.clone();
                    true
                } else {
                    false
                }
            };
            if updated {
                self.inner
                    .refresh_mesh_model(Some(&device_id), Some(&display_name));
            }
        }
        if result.is_ok()
            && let Some(bypassed) = self.inner.bypass_pending.get()
        {
            self.inner.bypass_state.set(Some(bypassed));
        }
        self.inner.bypass_pending.set(None);
        let wifi_changed = self.inner.wifi_pending.replace(false) && result.is_ok();
        if wifi_changed {
            self.inner.wifi_new.set_text("");
            self.inner.wifi_confirm.set_text("");
        }
        self.inner.refresh_dns_controls();
        self.inner.refresh_bypass_controls();
        let message = match result {
            Ok(()) if wifi_changed => self.inner.translator.text("wifiPassword.changed"),
            Ok(()) => self.inner.translator.text("common.saved"),
            Err(error) => self
                .inner
                .translator
                .text_with("common.failed", &[("message", &error)]),
        };
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
    }

    pub fn set_config_refresh_error(&self, error: &str) {
        let message = self
            .inner
            .translator
            .text_with("settings.routerConfigRefreshFailed", &[("message", error)]);
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
    }

    pub fn set_config_not_applied(&self, requested: &str, actual: &str) {
        let message = self.inner.translator.text_with(
            "settings.routerConfigNotApplied",
            &[("requested", requested), ("actual", actual)],
        );
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
    }

    pub fn set_dish_write_result(&self, result: Result<(), String>) {
        self.inner.dish_busy.set(false);
        self.inner.refresh_dish_controls();
        let message = match result {
            Ok(()) => self.inner.translator.text("common.saved"),
            Err(error) => self
                .inner
                .translator
                .text_with("common.failed", &[("message", &error)]),
        };
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
    }

    /// Finish a one-shot diagnostics request and report clipboard failures as
    /// failures rather than treating a completed read as a successful copy.
    // SPEC-DIAG-003, SPEC-DIAG-004: include only already-cached UI context.
    pub fn set_diagnostics_result(
        &self,
        result: Result<(serde_json::Value, serde_json::Value), String>,
    ) {
        let result = result.and_then(|(device_info, diagnostics)| {
            let document = diagnostics_document(
                &device_info,
                &diagnostics,
                self.inner.latest_status.borrow().as_ref(),
                self.inner.latest_dish_config.borrow().as_ref(),
            )
            .map_err(|error| error.to_string())?;
            set_diagnostics_clipboard(gtk::gdk::Display::default().as_ref(), &document)
        });
        if let Err(error) = &result {
            tracing::warn!("could not copy dish diagnostics: {error}");
        }
        self.inner.diagnostics_busy.set(false);
        self.inner.refresh_controls();
        let key = if result.is_ok() {
            "settings.copied"
        } else {
            "settings.copyFailed"
        };
        let message = self.inner.translator.text(key);
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
    }

    /// Render the latest LAN configuration reply; no new network request is
    /// initiated by the settings view.
    pub fn set_dish_config(&self, config: &serde_json::Value) {
        self.inner.latest_dish_config.replace(Some(config.clone()));
        self.inner.updating.set(true);
        let snow = match config
            .get("snowMeltMode")
            .and_then(serde_json::Value::as_str)
        {
            Some("ALWAYS_ON") => 1,
            Some("ALWAYS_OFF") => 2,
            _ => 0,
        };
        self.inner.snow_melt_row.set_selected(snow);
        self.inner.sleep_row.set_active(
            config
                .get("powerSaveMode")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
        let hour = config
            .get("swupdateRebootHour")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(3) as u32;
        self.inner.update_window_row.set_selected(match hour {
            9 => 1,
            15 => 2,
            21 => 3,
            _ => 0,
        });
        self.inner.defer_updates_row.set_active(
            config
                .get("swupdateThreeDayDeferralEnabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
        self.inner.dish_config_loaded.set(true);
        self.inner.updating.set(false);
        self.inner.refresh_dish_controls();
    }

    /// Seed the App tab from the preferences the host actually holds.
    // SPEC-CTRL-005: the switches show saved state; only user changes emit.
    pub fn set_app_state(&self, state: AppSettingsState) {
        self.inner.updating.set(true);
        self.inner
            .language_row
            .set_selected(locale_index(state.locale));
        self.inner
            .time_format_row
            .set_selected(time_format_index(state.time_format));
        self.inner
            .refresh_update_window_model(state.time_format, state.locale);
        self.inner
            .toolbar_style_row
            .set_selected(toolbar_style_index(state.toolbar_style));
        self.inner.self_device_id.set(state.self_client_id);
        self.inner.refresh_self_device_model();
        self.inner.notifications_row.set_active(state.notifications);
        self.inner.autostart_row.set_active(state.autostart);
        self.inner.tray_row.set_active(state.tray_throughput);
        self.inner.glass_row.set_active(state.see_through_glass);
        self.inner.show_tray_row.set_active(state.show_tray);
        self.inner
            .router_address
            .set_text(state.router_address.as_deref().unwrap_or(""));
        self.inner.router_address_error.set_label("");
        self.inner.updating.set(false);
    }

    pub fn select_self_device(&self, selected: u32) {
        self.inner.self_device_row.set_selected(selected);
    }

    /// Update the Starlink tab from the recorder's existing status reply. This
    /// does not fetch anything; it only changes which controls are meaningful.
    // SPEC-CTRL-003: stow is offered only when the status reports actuators.
    pub fn set_status(&self, status: Option<DishStatus>) {
        self.inner.latest_status.replace(status.clone());
        let present = status.is_some();
        let stowed = status.as_ref().is_some_and(|status| status.stow_requested);
        let actuators = status
            .as_ref()
            .and_then(|status| status.has_actuators.as_deref())
            == Some("HAS_ACTUATORS_YES");
        if let Some(status) = &status {
            let hardware = status
                .device_info
                .as_ref()
                .and_then(|info| info.hardware_version.as_deref())
                .unwrap_or("—");
            let software = status
                .device_info
                .as_ref()
                .and_then(|info| info.software_version.as_deref())
                .unwrap_or("—");
            self.inner.hardware_value.set_label(hardware);
            self.inner.software_value.set_label(software);
        } else {
            self.inner.hardware_value.set_label("—");
            self.inner.software_value.set_label("—");
        }
        self.inner.dish_present.set(present);
        self.inner.actuators.set(actuators);
        self.inner.stowed.set(stowed);
        self.inner.starlink_connecting.set_visible(!present);
        self.inner.stow_row.set_visible(actuators);
        if actuators {
            let action = ControlAction::StowDish { unstow: stowed };
            let copy = control_copy(action, &self.inner.translator);
            self.inner.stow_row.set_title(&copy.title);
            self.inner.stow_row.set_subtitle(&copy.detail);
            self.inner.stow_button.set_label(&copy.verb);
        }
        self.inner.refresh_controls();
    }

    /// `None` means the router has not answered yet; `Some(false)` is a failure.
    // SPEC-CTRL-004: router controls wait for the existing status cycle.
    pub fn set_router_reachable(&self, reachable: Option<bool>) {
        self.inner.router_reachable.set(reachable);
        let (text, class) = match reachable {
            None => (
                self.inner.translator.text("settings.contactingRouter"),
                None,
            ),
            Some(true) => (
                self.inner.translator.text("status.online"),
                Some("dishy-settings-online"),
            ),
            Some(false) => (
                self.inner.translator.text("settings.routerUnavailable"),
                Some("dishy-settings-offline"),
            ),
        };
        self.inner.router_value.set_label(&text);
        self.inner
            .router_value
            .remove_css_class("dishy-settings-online");
        self.inner
            .router_value
            .remove_css_class("dishy-settings-offline");
        if let Some(class) = class {
            self.inner.router_value.add_css_class(class);
        }
        self.inner.refresh_controls();
    }

    /// Show a control result as a toast. The page is the only place that turns
    /// the recorder's outcome into copy, so a failure cannot be mistaken for a
    /// sent command.
    // SPEC-CTRL-006: success names the action; failure carries the error.
    pub fn set_control_result(&self, report: ControlReport) {
        self.inner.busy.set(false);
        let message = match report.result {
            Ok(()) => self.inner.translator.text(report.action.success_key()),
            Err(error) => self
                .inner
                .translator
                .text_with("common.failed", &[("message", &error)]),
        };
        self.inner.last_toast.replace(Some(message.clone()));
        self.inner.root.add_toast(adw::Toast::new(&message));
        self.inner.refresh_controls();
    }

    /// Send a confirmed action. The confirmation dialog is the only production
    /// caller; the shell test calls this directly to prove the command travels
    /// through the recorder channel.
    pub fn dispatch_control(&self, action: ControlAction) {
        self.inner.dispatch(action);
    }

    pub fn last_toast(&self) -> Option<String> {
        self.inner.last_toast.borrow().clone()
    }

    pub fn last_action(&self) -> Option<ControlAction> {
        *self.inner.last_action.borrow()
    }

    pub fn dish_control_count(&self) -> usize {
        self.inner.dish_buttons.len()
    }

    pub fn router_control_count(&self) -> usize {
        self.inner.router_buttons.len()
    }

    pub fn stow_visible(&self) -> bool {
        self.inner.stow_row.is_visible()
    }

    /// Drive the same selection path a user takes. Tests use this instead of
    /// reaching into the private rows; production changes arrive from GTK.
    pub fn select_locale(&self, locale: Locale) {
        self.inner.language_row.set_selected(locale_index(locale));
    }

    pub fn select_notifications(&self, on: bool) {
        self.inner.notifications_row.set_active(on);
    }

    pub fn select_autostart(&self, on: bool) {
        self.inner.autostart_row.set_active(on);
    }

    pub fn select_tray_throughput(&self, on: bool) {
        self.inner.tray_row.set_active(on);
    }

    pub fn select_time_format(&self, format: TimeFormat) {
        self.inner
            .time_format_row
            .set_selected(time_format_index(format));
    }

    pub fn select_toolbar_style(&self, style: ToolbarStyle) {
        self.inner
            .toolbar_style_row
            .set_selected(toolbar_style_index(style));
    }
}

impl Inner {
    fn set_router_config(&self, config: &WifiConfig) {
        *self.router_config.borrow_mut() = Some(config.clone());
        let nameservers = if config.custom_dns_disabled {
            &[][..]
        } else {
            config.nameservers.as_slice()
        };
        for (index, entry) in self.dns_entries.iter().enumerate() {
            entry.set_text(nameservers.get(index).map(String::as_str).unwrap_or(""));
        }
        let selected = config
            .subnet()
            .and_then(|subnet| SUBNET_PRESETS.iter().position(|preset| *preset == subnet))
            .map(|index| index as u32 + 1)
            .unwrap_or(0);
        self.subnet_row.set_selected(selected);
    }

    fn set_self_devices(&self, mut devices: Vec<(u32, String)>) {
        devices.sort_by_key(|(client_id, _)| *client_id);
        devices.dedup_by_key(|(client_id, _)| *client_id);
        *self.self_devices.borrow_mut() = devices;
        self.refresh_self_device_model();
    }

    fn refresh_self_device_model(&self) {
        let was_updating = self.updating.replace(true);
        let selected_id = self.self_device_id.get();
        let devices = self.self_devices.borrow();
        let mut labels = vec![self.translator.text("common.none")];
        let mut choices = vec![SelfDeviceChoice::None];
        if let Some(selected_id) = selected_id
            && !devices
                .iter()
                .any(|(client_id, _)| *client_id == selected_id)
        {
            labels.push(self.translator.text("common.notConnected"));
            choices.push(SelfDeviceChoice::Unavailable);
        }
        for (client_id, label) in devices.iter() {
            labels.push(label.clone());
            choices.push(SelfDeviceChoice::Client(*client_id));
        }
        self.self_device_model.splice(
            0,
            self.self_device_model.n_items(),
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let selected_choice = match selected_id {
            None => SelfDeviceChoice::None,
            Some(client_id) if devices.iter().any(|(id, _)| *id == client_id) => {
                SelfDeviceChoice::Client(client_id)
            }
            Some(_) => SelfDeviceChoice::Unavailable,
        };
        let selected = choices
            .iter()
            .position(|choice| *choice == selected_choice)
            .unwrap_or(0) as u32;
        *self.self_device_choices.borrow_mut() = choices;
        self.self_device_row.set_selected(selected);
        self.updating.set(was_updating);
    }

    fn select_self_device(&self, selected: u32) {
        let choice = self
            .self_device_choices
            .borrow()
            .get(selected as usize)
            .copied()
            .unwrap_or(SelfDeviceChoice::None);
        let Some(client_id) = (match choice {
            SelfDeviceChoice::None => Some(None),
            SelfDeviceChoice::Unavailable => None,
            SelfDeviceChoice::Client(client_id) => Some(Some(client_id)),
        }) else {
            self.refresh_self_device_model();
            return;
        };
        if self.self_device_id.get() == client_id {
            return;
        }
        let previous = self.self_device_id.replace(client_id);
        if !self.send_action(SettingsAction::SetSelfClientId(client_id)) {
            self.self_device_id.set(previous);
            self.refresh_self_device_model();
            let message = self.translator.text("settings.deviceSaveFailed");
            self.root.add_toast(adw::Toast::new(&message));
        }
    }

    fn refresh_update_window_model(&self, format: TimeFormat, locale: Locale) {
        let selected = self.update_window_row.selected();
        let labels = formatted_update_window_labels(format, locale);
        self.update_window_model.splice(
            0,
            self.update_window_model.n_items(),
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        self.update_window_row
            .set_selected(if selected < 4 { selected } else { 0 });
    }

    // SPEC-DIAG-002: only the explicit button handler sends this recorder command.
    fn request_diagnostics(&self) {
        if self.diagnostics_busy.get() || !self.dish_present.get() {
            return;
        }
        let Some(sender) = self.commands.borrow().as_ref().cloned() else {
            return;
        };
        match sender.send(RecorderCommand::GetDiagnostics) {
            Ok(()) => self.diagnostics_busy.set(true),
            Err(error) => {
                tracing::warn!("could not request dish diagnostics: {error}");
                let message = self.translator.text("settings.copyFailed");
                self.last_toast.replace(Some(message.clone()));
                self.root.add_toast(adw::Toast::new(&message));
            }
        }
        self.refresh_controls();
    }

    fn send_action(&self, action: SettingsAction) -> bool {
        self.actions
            .borrow()
            .as_ref()
            .is_some_and(|sender| sender.send(action).is_ok())
    }

    fn set_mesh_nodes(&self, nodes: Vec<(String, String)>) {
        let selected_id = self.selected_mesh_node().map(|(id, _)| id);
        *self.mesh_nodes.borrow_mut() = nodes;
        self.refresh_mesh_model(selected_id.as_deref(), None);
    }

    fn refresh_mesh_model(&self, selected_id: Option<&str>, selected_name: Option<&str>) {
        let placeholder = self.translator.text("network.meshRenameChoose");
        let nodes = self.mesh_nodes.borrow();
        let labels = std::iter::once(placeholder.as_str())
            .chain(nodes.iter().map(|(_, name)| name.as_str()))
            .collect::<Vec<_>>();
        self.updating.set(true);
        self.mesh_model
            .splice(0, self.mesh_model.n_items(), &labels);
        let selected = selected_id
            .and_then(|id| nodes.iter().position(|(node_id, _)| node_id == id))
            .map(|index| index as u32 + 1)
            .unwrap_or(0);
        self.mesh_row.set_selected(selected);
        if let Some(name) = selected_name {
            self.mesh_name.set_text(name);
        } else if selected == 0 {
            self.mesh_name.set_text("");
        }
        self.mesh_error.set_label("");
        self.updating.set(false);
        drop(nodes);
        self.refresh_mesh_controls();
    }

    fn selected_mesh_node(&self) -> Option<(String, String)> {
        let selected = self.mesh_row.selected().checked_sub(1)? as usize;
        self.mesh_nodes.borrow().get(selected).cloned()
    }

    fn select_mesh_node(&self, selected: u32) {
        if self.updating.get() {
            return;
        }
        if let Some((_, name)) = selected
            .checked_sub(1)
            .and_then(|index| self.mesh_nodes.borrow().get(index as usize).cloned())
        {
            self.mesh_name.set_text(&name);
        } else {
            self.mesh_name.set_text("");
        }
        self.mesh_error.set_label("");
        self.refresh_mesh_controls();
    }

    fn save_mesh_name(&self) {
        if self.dns_busy.get() || !self.account_connected.get() {
            return;
        }
        let Some((device_id, existing_name)) = self.selected_mesh_node() else {
            self.mesh_error
                .set_label(&self.translator.text("network.meshRenameChoose"));
            return;
        };
        let display_name = self.mesh_name.text().trim().to_owned();
        let update = RouterConfigUpdate::MeshName {
            device_id: device_id.clone(),
            display_name: display_name.clone(),
        };
        if validate_router_config_update(&update).is_err() {
            self.mesh_error
                .set_label(&self.translator.text("errors.meshNameEmpty"));
            return;
        }
        if display_name == existing_name {
            return;
        }
        self.mesh_error.set_label("");
        self.dns_busy.set(true);
        self.mesh_pending_name
            .replace(Some((device_id, display_name)));
        self.refresh_dns_controls();
        if !self.send_action(SettingsAction::UpdateRouterConfig(update)) {
            self.dns_busy.set(false);
            self.mesh_pending_name.borrow_mut().take();
            self.mesh_error
                .set_label(&self.translator.text("errors.cloudNotStarted"));
            self.refresh_dns_controls();
        }
    }

    fn refresh_mesh_controls(&self) {
        let selected = self.selected_mesh_node();
        let has_nodes = !self.mesh_nodes.borrow().is_empty();
        self.mesh_row
            .set_subtitle(&self.translator.text(if has_nodes {
                "network.meshRenameHelp"
            } else {
                "network.meshRenameNeedsNetwork"
            }));
        self.mesh_row
            .set_sensitive(has_nodes && !self.dns_busy.get());
        self.mesh_name
            .set_sensitive(selected.is_some() && !self.dns_busy.get());
        let name = self.mesh_name.text();
        let changed = selected
            .as_ref()
            .is_some_and(|(_, existing)| name.trim() != existing);
        self.mesh_save_button.set_sensitive(
            selected.is_some()
                && changed
                && !name.trim().is_empty()
                && self.account_connected.get()
                && !self.dns_busy.get()
                && self.actions.borrow().is_some(),
        );
        self.mesh_save_button
            .set_label(&self.translator.text(if self.dns_busy.get() {
                "common.savingEllipsis"
            } else {
                "common.save"
            }));
    }

    fn send_dish_update(&self, update: DishConfigUpdate) {
        if !self.dish_config_loaded.get() || self.dish_busy.get() {
            return;
        }
        self.dish_busy.set(true);
        self.refresh_dish_controls();
        if !self.send_action(SettingsAction::UpdateDishConfig(update)) {
            self.dish_busy.set(false);
            self.refresh_dish_controls();
            let message = self.translator.text("errors.cloudNotStarted");
            self.last_toast.replace(Some(message.clone()));
            self.root.add_toast(adw::Toast::new(&message));
        }
    }

    fn refresh_dish_controls(&self) {
        let enabled = self.dish_config_loaded.get()
            && !self.dish_busy.get()
            && self.actions.borrow().is_some();
        self.snow_melt_row.set_sensitive(enabled);
        self.sleep_row.set_sensitive(enabled);
        self.update_window_row.set_sensitive(enabled);
        self.defer_updates_row.set_sensitive(enabled);
    }

    fn refresh_dns_controls(&self) {
        let label = if self.dns_busy.get() {
            self.translator.text("common.savingEllipsis")
        } else {
            self.translator.text("common.save")
        };
        self.dns_save_button
            .set_sensitive(!self.dns_busy.get() && self.actions.borrow().is_some());
        self.dns_save_button.set_label(&label);
        let subnet_enabled = !self.dns_busy.get() && self.actions.borrow().is_some();
        self.subnet_row.set_sensitive(subnet_enabled);
        self.subnet_password.set_sensitive(subnet_enabled);
        self.subnet_save_button.set_sensitive(subnet_enabled);
        let subnet_label = if self.dns_busy.get() {
            label
        } else {
            self.translator.text("subnet.move")
        };
        self.subnet_save_button.set_label(&subnet_label);
        // The Wi-Fi password is a cloud write: it needs the signed-in account.
        let signed_in = self.account_connected.get();
        let wifi_enabled = subnet_enabled && signed_in;
        self.wifi_new.set_sensitive(wifi_enabled);
        self.wifi_confirm.set_sensitive(wifi_enabled);
        self.wifi_save_button.set_sensitive(wifi_enabled);
        self.wifi_row
            .set_subtitle(&self.translator.text(if signed_in {
                "wifiPassword.caption"
            } else {
                "wifiPassword.signInRequired"
            }));
        self.refresh_mesh_controls();
    }

    fn refresh_bypass_controls(&self) {
        let bypassed = self.bypass_state.get();
        let turn_on = bypassed == Some(false);
        let label = self.translator.text(if turn_on {
            "bypass.turnOn"
        } else {
            "bypass.turnOff"
        });
        self.bypass_button.set_label(&label);
        let subtitle = self.translator.text(match bypassed {
            Some(true) => "bypass.onCaption",
            Some(false) => "bypass.offCaption",
            None => "bypass.unknownState",
        });
        self.bypass_row.set_subtitle(&subtitle);
        self.bypass_button
            .set_sensitive(!self.dns_busy.get() && self.actions.borrow().is_some());
    }

    fn confirm_bypass(&self) {
        if self.dns_busy.get() || self.actions.borrow().is_none() {
            return;
        }
        // With no trustworthy state, choose the safe recovery direction.
        let enabled = bypass_target(self.bypass_state.get());
        let detail = self.translator.text(if enabled {
            "bypass.confirmOn"
        } else {
            "bypass.confirmOff"
        });
        let confirm = self.translator.text(if enabled {
            "bypass.turnOn"
        } else {
            "bypass.turnOff"
        });
        let cancel = self.translator.text("common.cancel");
        let mut builder = adw::MessageDialog::builder()
            .heading(self.translator.text("bypass.title"))
            .body(detail);
        if let Some(parent) = self.root.root().and_downcast::<gtk::Window>() {
            builder = builder.transient_for(&parent);
        }
        let dialog = builder.build();
        dialog.add_response("cancel", &cancel);
        dialog.add_response("confirm", &confirm);
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        if enabled {
            dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        }
        let weak = self.self_weak.clone();
        dialog.choose(None::<&gtk::gio::Cancellable>, move |response| {
            if response != "confirm" {
                return;
            }
            if let Some(inner) = weak.upgrade() {
                inner.bypass_pending.set(Some(enabled));
                inner.dns_busy.set(true);
                inner.refresh_dns_controls();
                inner.refresh_bypass_controls();
                if !inner.send_action(SettingsAction::UpdateRouterConfig(
                    RouterConfigUpdate::Bypass { enabled },
                )) {
                    inner.dns_busy.set(false);
                    inner.bypass_pending.set(None);
                    inner.refresh_dns_controls();
                    inner.refresh_bypass_controls();
                    let message = inner.translator.text("errors.cloudNotStarted");
                    inner.last_toast.replace(Some(message.clone()));
                    inner.root.add_toast(adw::Toast::new(&message));
                }
            }
        });
    }

    /// Confirm, then send once (spec 0185). Every device drops off the Wi-Fi.
    fn confirm_wifi_password(&self, password: String) {
        let cancel = self.translator.text("common.cancel");
        let confirm = self.translator.text("wifiPassword.save");
        let heading = self.translator.text("wifiPassword.confirmTitle");
        let body = self.translator.text("wifiPassword.confirmBody");
        let mut builder = adw::MessageDialog::builder().heading(&heading).body(&body);
        if let Some(parent) = self.root.root().and_downcast::<gtk::Window>() {
            builder = builder.transient_for(&parent);
        }
        let dialog = builder.build();
        dialog.add_response("cancel", &cancel);
        dialog.add_response("confirm", &confirm);
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        let weak = self.self_weak.clone();
        dialog.choose(None::<&gtk::gio::Cancellable>, move |response| {
            if response != "confirm" {
                return;
            }
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.dns_busy.set(true);
            inner.wifi_pending.set(true);
            inner.refresh_dns_controls();
            let update = RouterConfigUpdate::WifiPassword { password };
            if !inner.send_action(SettingsAction::UpdateRouterConfig(update)) {
                inner.dns_busy.set(false);
                inner.wifi_pending.set(false);
                inner
                    .wifi_error
                    .set_label(&inner.translator.text("errors.cloudNotStarted"));
                inner.refresh_dns_controls();
            }
        });
    }

    fn confirm_subnet(&self, update: RouterConfigUpdate, subnet: &str) {
        let cancel = self.translator.text("common.cancel");
        let confirm = self.translator.text("subnet.move");
        let heading = self
            .translator
            .text_with("subnet.confirmTitle", &[("subnet", subnet)]);
        let body = self.translator.text("subnet.confirmBody");
        let mut builder = adw::MessageDialog::builder().heading(&heading).body(&body);
        if let Some(parent) = self.root.root().and_downcast::<gtk::Window>() {
            builder = builder.transient_for(&parent);
        }
        let dialog = builder.build();
        dialog.add_response("cancel", &cancel);
        dialog.add_response("confirm", &confirm);
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        let weak = self.self_weak.clone();
        dialog.choose(None::<&gtk::gio::Cancellable>, move |response| {
            if response != "confirm" {
                return;
            }
            if let Some(inner) = weak.upgrade() {
                inner.dns_busy.set(true);
                inner.refresh_dns_controls();
                if !inner.send_action(SettingsAction::UpdateRouterConfig(update)) {
                    inner.dns_busy.set(false);
                    inner
                        .subnet_error
                        .set_label(&inner.translator.text("errors.cloudNotStarted"));
                    inner.refresh_dns_controls();
                }
            }
        });
    }

    // SPEC-CTRL-002, SPEC-SAFETY-004: one write at a time, through the recorder.
    fn dispatch(&self, action: ControlAction) {
        if self.busy.get() {
            return;
        }
        // A dialog may have been opened before the target went away. Re-check
        // the state at send time, not just when the button was built.
        let target_ready = match action.target() {
            ControlTarget::Dish => self.dish_present.get(),
            ControlTarget::Router => self.router_reachable.get() == Some(true),
        };
        if !target_ready || (action.requires_actuators() && !self.actuators.get()) {
            return;
        }
        let Some(sender) = self.commands.borrow().as_ref().cloned() else {
            return;
        };
        match sender.send(RecorderCommand::Control(action)) {
            Ok(()) => {
                self.busy.set(true);
                self.last_action.replace(Some(action));
                self.refresh_controls();
            }
            Err(error) => {
                let message = self
                    .translator
                    .text_with("common.failed", &[("message", &error.to_string())]);
                self.last_toast.replace(Some(message.clone()));
                self.root.add_toast(adw::Toast::new(&message));
            }
        }
    }

    // SPEC-CTRL-006: every action names itself and defaults to Cancel.
    fn confirm(&self, action: ControlAction) {
        let copy = control_copy(action, &self.translator);
        let cancel = self.translator.text("common.cancel");
        let mut builder = adw::MessageDialog::builder()
            .heading(&copy.title)
            .body(&copy.detail);
        if let Some(parent) = self.root.root().and_downcast::<gtk::Window>() {
            builder = builder.transient_for(&parent);
        }
        let dialog = builder.build();
        dialog.add_response("cancel", &cancel);
        dialog.add_response("confirm", &copy.confirm);
        dialog.set_close_response("cancel");
        // Cancel is the default so Enter cannot send a reboot by accident.
        dialog.set_default_response(Some("cancel"));
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        let weak = self.self_weak.clone();
        dialog.choose(None::<&gtk::gio::Cancellable>, move |response| {
            if response != "confirm" {
                return;
            }
            if let Some(inner) = weak.upgrade() {
                if action == ControlAction::FactoryResetRouter
                    && inner.router_reachable.get() != Some(true)
                    && inner.account_connected.get()
                {
                    inner.dns_busy.set(true);
                    inner.refresh_dns_controls();
                    inner.refresh_bypass_controls();
                    if !inner.send_action(SettingsAction::UpdateRouterConfig(
                        RouterConfigUpdate::FactoryReset,
                    )) {
                        inner.dns_busy.set(false);
                        inner.refresh_dns_controls();
                        inner.refresh_bypass_controls();
                    }
                } else {
                    inner.dispatch(action);
                }
            }
        });
    }

    fn refresh_controls(&self) {
        let ready = !self.busy.get() && self.commands.borrow().is_some();
        for button in &self.dish_buttons {
            button.set_sensitive(ready && self.dish_present.get());
        }
        self.stow_button
            .set_sensitive(ready && self.dish_present.get() && self.actuators.get());
        self.diagnostics_button.set_sensitive(
            self.commands.borrow().is_some()
                && self.dish_present.get()
                && !self.diagnostics_busy.get(),
        );
        let router_ready = ready && self.router_reachable.get() == Some(true);
        if let Some(reboot) = self.router_buttons.first() {
            reboot.set_sensitive(router_ready);
        }
        if let Some(factory_reset) = self.router_buttons.get(1) {
            factory_reset.set_sensitive(router_ready || (ready && self.account_connected.get()));
        }
    }
}

// SPEC-DIAG-003: use the latest cached status/config; do not trigger reads here.
fn diagnostics_document(
    device_info: &serde_json::Value,
    diagnostics: &serde_json::Value,
    status: Option<&DishStatus>,
    config: Option<&serde_json::Value>,
) -> Result<String, serde_json::Error> {
    let mut document = serde_json::Map::new();
    document.insert(
        "capturedAt".into(),
        serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    document.insert("deviceInfo".into(), device_info.clone());
    document.insert("diagnostics".into(), diagnostics.clone());
    if let Some(status) = status {
        document.insert("status".into(), serde_json::to_value(status)?);
    }
    if let Some(config) = config {
        document.insert("config".into(), config.clone());
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(document))
}

// SPEC-TIME-003, SPEC-TIME-004: only the four choices' display labels vary.
fn formatted_update_window_labels(format: TimeFormat, locale: Locale) -> [String; 4] {
    let hour_24 =
        format != TimeFormat::Hour12 && (format == TimeFormat::Hour24 || locale != Locale::En);
    let translator = Translator::new(locale);
    [3, 9, 15, 21].map(|hour| {
        if hour_24 {
            format!("{hour:02}:00")
        } else {
            let suffix = translator.text(if hour < 12 {
                "settings.timeFormatAm"
            } else {
                "settings.timeFormatPm"
            });
            format!(
                "{}:00 {suffix}",
                if hour % 12 == 0 { 12 } else { hour % 12 }
            )
        }
    })
}

fn time_format_index(format: TimeFormat) -> u32 {
    match format {
        TimeFormat::Auto => 0,
        TimeFormat::Hour12 => 1,
        TimeFormat::Hour24 => 2,
    }
}

fn toolbar_style_index(style: ToolbarStyle) -> u32 {
    match style {
        ToolbarStyle::Dock => 0,
        ToolbarStyle::Rail => 1,
    }
}

// SPEC-DIAG-004: absence of a desktop clipboard is a failed copy.
fn set_diagnostics_clipboard(
    display: Option<&gtk::gdk::Display>,
    document: &str,
) -> Result<(), String> {
    let display = display.ok_or_else(|| "clipboard is unavailable".to_owned())?;
    display.clipboard().set_text(document);
    Ok(())
}

fn wire_control(button: &gtk::Button, weak: &std::rc::Weak<Inner>, action: ControlAction) {
    let weak = weak.clone();
    button.connect_clicked(move |_| {
        if let Some(inner) = weak.upgrade() {
            inner.confirm(action);
        }
    });
}

fn normalize_dns_fields(fields: &[String]) -> Result<Vec<String>, ()> {
    if fields.len() > 4 {
        return Err(());
    }
    let mut addresses = Vec::new();
    for field in fields {
        let candidate = field.trim();
        if candidate.is_empty() {
            continue;
        }
        let address = candidate.parse::<std::net::IpAddr>().map_err(|_| ())?;
        let canonical = address.to_string();
        if !addresses.contains(&canonical) {
            addresses.push(canonical);
        }
    }
    Ok(addresses)
}

fn bypass_target(current: Option<bool>) -> bool {
    current == Some(false)
}

fn action_button(label: &str, destructive: bool) -> gtk::Button {
    let text = gtk::Label::builder()
        .label(label)
        .single_line_mode(true)
        .xalign(0.5)
        .build();
    let button = gtk::Button::new();
    button.set_widget_name("maintenance-action");
    button.add_css_class("dishy-maintenance-action");
    button.set_size_request(-1, 36);
    button.set_hexpand(false);
    button.set_vexpand(false);
    button.set_valign(gtk::Align::Center);
    button.set_child(Some(&text));
    if destructive {
        button.add_css_class("destructive-action");
    }
    button
}

fn configure_widget(widget: &gtk::Widget) {
    widget.set_size_request(220, 36);
    widget.set_hexpand(false);
    widget.set_valign(gtk::Align::Center);
}

fn configure_entry(entry: &gtk::Entry) {
    entry.set_width_chars(22);
    entry.set_max_width_chars(22);
    configure_widget(entry.upcast_ref());
}

fn form_action_button(label: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_widget_name(name);
    button.add_css_class("dishy-form-action");
    button.set_size_request(80, 36);
    button.set_hexpand(false);
    button.set_valign(gtk::Align::Center);
    button
}

fn value_label() -> gtk::Label {
    let label = gtk::Label::new(Some("—"));
    label.add_css_class("dishy-settings-value");
    label.add_css_class("dim-label");
    label
}

fn locale_index(locale: Locale) -> u32 {
    SUPPORTED_LOCALES
        .iter()
        .position(|candidate| *candidate == locale)
        .unwrap_or(0) as u32
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlCopy {
    title: String,
    detail: String,
    confirm: String,
    verb: String,
}

fn control_copy(action: ControlAction, translator: &Translator) -> ControlCopy {
    let (title, detail, confirm, verb) = match action {
        ControlAction::ClearObstructionMap => (
            "settings.resetObstruction",
            "settings.resetObstructionDesc",
            "settings.confirmResetMap",
            "settings.reset",
        ),
        ControlAction::RebootDish => (
            "settings.rebootStarlink",
            "settings.rebootStarlinkDesc",
            "settings.rebootDish",
            "settings.reboot",
        ),
        ControlAction::FactoryResetDish => (
            "settings.factoryResetStarlink",
            "settings.factoryResetStarlinkDesc",
            "settings.factoryResetDish",
            "settings.factoryReset",
        ),
        ControlAction::StowDish { unstow: false } => (
            "settings.stowDish",
            "settings.stowDishDesc",
            "settings.confirmStow",
            "settings.stow",
        ),
        ControlAction::StowDish { unstow: true } => (
            "settings.unstowDish",
            "settings.unstowDishDesc",
            "settings.confirmUnstow",
            "settings.unstow",
        ),
        ControlAction::RebootRouter => (
            "settings.rebootRouter",
            "settings.rebootRouterDesc",
            "settings.rebootRouter",
            "settings.reboot",
        ),
        ControlAction::FactoryResetRouter => (
            "settings.factoryResetRouter",
            "settings.factoryResetRouterDesc",
            "settings.factoryResetRouter",
            "settings.factoryReset",
        ),
    };
    let mut detail = translator.text(detail);
    if matches!(
        action,
        ControlAction::FactoryResetDish | ControlAction::FactoryResetRouter
    ) {
        let warning = if matches!(action, ControlAction::FactoryResetDish) {
            "settings.factoryResetDishWarning"
        } else {
            "settings.factoryResetRouterWarning"
        };
        detail.push_str("\n\n");
        detail.push_str(&translator.text(warning));
    }
    ControlCopy {
        title: translator.text(title),
        detail,
        confirm: translator.text(confirm),
        verb: translator.text(verb),
    }
}

fn install_css() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

#[cfg(test)]
mod tests {

    // SPEC-WIFIPW-003: a mismatch or a bad length is refused before sending,
    // mismatch first so the user fixes the typo before counting characters.
    #[test]
    fn wifi_password_refusals_come_before_any_write() {
        assert_eq!(
            wifi_password_refusal("correct horse", "correct horse"),
            None
        );
        assert_eq!(
            wifi_password_refusal("correct horse", "correct hors"),
            Some(WifiPasswordRefusal::Mismatch)
        );
        assert_eq!(
            wifi_password_refusal("short", "short"),
            Some(WifiPasswordRefusal::Length)
        );
        assert_eq!(
            wifi_password_refusal("short", "shirt"),
            Some(WifiPasswordRefusal::Mismatch)
        );
        assert_eq!(
            wifi_password_refusal(&"x".repeat(64), &"x".repeat(64)),
            Some(WifiPasswordRefusal::Length)
        );
    }

    use super::*;

    // SPEC-CLOUDCFG-001: the settings form ignores blank backup fields but
    // rejects non-addresses and keeps only the first canonical duplicate.
    #[test]
    fn dns_form_normalizes_fields_and_refuses_invalid_input() {
        assert_eq!(
            normalize_dns_fields(&[
                " 1.1.1.1 ".into(),
                "".into(),
                "2606:4700:4700:0:0:0:0:1111".into(),
                "1.1.1.1".into(),
            ])
            .unwrap(),
            vec!["1.1.1.1", "2606:4700:4700::1111"]
        );
        assert!(normalize_dns_fields(&["dns.example".into()]).is_err());
    }

    // SPEC-CLOUDCFG-006: unknown bypass state defaults to the safe recovery
    // action (off); a known state offers only the inverse.
    #[test]
    fn bypass_confirmation_targets_recovery_when_state_is_unknown() {
        assert!(!bypass_target(None));
        assert!(bypass_target(Some(false)));
        assert!(!bypass_target(Some(true)));
    }

    // SPEC-CTRL-003, SPEC-CTRL-006: the stow row flips between fold and unfold
    // copy, and both paths require their own confirmation.
    #[test]
    fn stow_copy_follows_the_dish_state() {
        let translator = Translator::new(Locale::En);
        let folded = control_copy(ControlAction::StowDish { unstow: false }, &translator);
        assert_eq!(folded.title, "Stow dish");
        assert_eq!(folded.verb, "Stow");
        assert_eq!(folded.confirm, "Yes, stow");
        let unfolded = control_copy(ControlAction::StowDish { unstow: true }, &translator);
        assert_eq!(unfolded.title, "Unstow dish");
        assert_eq!(unfolded.verb, "Unstow");
        assert_eq!(unfolded.confirm, "Yes, unstow");
    }

    // SPEC-CTRL-006: factory resets carry their warning into the confirmation
    // detail rather than hiding it in a tooltip.
    #[test]
    fn factory_reset_copy_keeps_the_warning() {
        let translator = Translator::new(Locale::En);
        let copy = control_copy(ControlAction::FactoryResetRouter, &translator);
        assert!(copy.detail.contains("Not reversible"));
        assert!(copy.detail.contains("clear your WiFi network name"));
    }

    // SPEC-DIAG-003: support data includes fetched diagnostics and only cached
    // context, with no extra device reads during serialization.
    #[test]
    fn diagnostics_document_contains_device_data_and_cached_context() {
        let status = DishStatus {
            has_actuators: Some("HAS_ACTUATORS_NO".into()),
            ..DishStatus::default()
        };
        let config = serde_json::json!({"snowMeltMode":"AUTO"});
        let document = diagnostics_document(
            &serde_json::json!({"hardwareVersion":"rev3"}),
            &serde_json::json!({"obstructionStats":{"fractionObstructed":0.2}}),
            Some(&status),
            Some(&config),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&document).unwrap();
        assert!(value["capturedAt"].as_str().is_some());
        assert_eq!(value["deviceInfo"]["hardwareVersion"], "rev3");
        assert!(value["diagnostics"]["obstructionStats"].is_object());
        assert_eq!(value["status"]["hasActuators"], "HAS_ACTUATORS_NO");
        assert_eq!(value["config"]["snowMeltMode"], "AUTO");
    }

    // SPEC-DIAG-003: absent status/config are omitted rather than triggering
    // reads or emitting misleading empty objects.
    #[test]
    fn diagnostics_document_omits_unavailable_cached_context() {
        let document =
            diagnostics_document(&serde_json::json!({}), &serde_json::json!({}), None, None)
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&document).unwrap();
        assert!(value.get("status").is_none());
        assert!(value.get("config").is_none());
    }

    // SPEC-DIAG-004: no display means the payload is never reported as copied.
    #[test]
    fn diagnostics_copy_fails_without_a_clipboard_display() {
        assert!(set_diagnostics_clipboard(None, "{}").is_err());
    }

    // SPEC-TIME-003, SPEC-TIME-004: Automatic follows the interface language,
    // and a forced preference overrides it without changing the stored hours.
    #[test]
    fn update_window_labels_follow_time_format_without_changing_hours() {
        assert_eq!(
            formatted_update_window_labels(TimeFormat::Auto, Locale::En),
            ["3:00 AM", "9:00 AM", "3:00 PM", "9:00 PM"]
        );
        assert_eq!(
            formatted_update_window_labels(TimeFormat::Auto, Locale::Fr),
            ["03:00", "09:00", "15:00", "21:00"]
        );
        assert_eq!(
            formatted_update_window_labels(TimeFormat::Hour12, Locale::Fr),
            ["3:00 AM", "9:00 AM", "3:00 PM", "9:00 PM"]
        );
        assert_eq!(
            formatted_update_window_labels(TimeFormat::Hour12, Locale::Es),
            ["3:00 a. m.", "9:00 a. m.", "3:00 p. m.", "9:00 p. m."]
        );
        assert_eq!(
            formatted_update_window_labels(TimeFormat::Hour24, Locale::En),
            ["03:00", "09:00", "15:00", "21:00"]
        );
    }
}
