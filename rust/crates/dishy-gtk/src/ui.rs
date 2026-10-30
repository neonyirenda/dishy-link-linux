//! The one application window.
//!
//! Milestone M5 fills the dashboard; M4 establishes the shell every later view
//! sits in: libadwaita's view stack, translated section labels, and the
//! close-to-hide rule that keeps the recorder alive.

use std::sync::Once;

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::AdwApplicationWindowExt;

use crate::alerts_view::AlertsPage;
use crate::alignment_view::AlignmentPage;
use crate::cloud_view::AccountPage;
use crate::dashboard_view::DashboardPage;
use crate::i18n::Translator;
use crate::network_view::NetworkPage;
use crate::preferences::{Theme, ToolbarStyle, WindowBounds};
use crate::settings_view::SettingsPage;
use crate::sky_view::SkyPage;
use crate::speed_test_view::SpeedTestPage;
use crate::usage_view::DataUsagePage;
use crate::window_bounds::{MIN_HEIGHT, MIN_WIDTH, default_bounds};

pub struct ShellWindow {
    pub window: adw::ApplicationWindow,
    pub glass: crate::theme::GlassBackdrop,
    pub stack: adw::ViewStack,
    pub theme_button: gtk::Button,
    pub notification_button: gtk::MenuButton,
    pub notification_badge: gtk::Label,
    pub header_status: gtk::Label,
    pub connectivity_indicator: gtk::Label,
    pub dashboard: DashboardPage,
    pub sky: SkyPage,
    pub data_usage: DataUsagePage,
    pub network: NetworkPage,
    pub speed_test: SpeedTestPage,
    pub alignment: AlignmentPage,
    pub account: AccountPage,
    pub alerts: AlertsPage,
    pub settings: SettingsPage,
    pub toolbar_layout: ToolbarLayout,
}

#[derive(Clone)]
pub struct ToolbarLayout {
    dock: gtk::Box,
    rail: gtk::Box,
    toolbar: adw::ToolbarView,
    rail_labels: Vec<gtk::Revealer>,
    rail_rows: Vec<gtk::Box>,
    navigation_buttons: NavigationButtons,
}

/// The dock's height plus its gap: what a page's end clears (SPEC-TOOLBAR-010).
const DOCK_CLEARANCE: i32 = 96;

impl ToolbarLayout {
    pub fn set_style(&self, style: ToolbarStyle) {
        self.dock.set_visible(style == ToolbarStyle::Dock);
        self.rail.set_visible(style == ToolbarStyle::Rail);
        // The dock floats over the content, as in Electron: pages pad their
        // own end instead, so the sky view can fill the window.
        if style == ToolbarStyle::Dock {
            self.toolbar.add_css_class("dishy-dock-clearance");
        } else {
            self.toolbar.remove_css_class("dishy-dock-clearance");
        }
    }

    pub fn is_dock_visible(&self) -> bool {
        self.dock.is_visible()
    }

    pub fn is_rail_visible(&self) -> bool {
        self.rail.is_visible()
    }

    pub fn dock_alignment(&self) -> (gtk::Align, gtk::Align) {
        (self.dock.halign(), self.dock.valign())
    }

    pub fn rail_alignment(&self) -> (gtk::Align, gtk::Align) {
        (self.rail.halign(), self.rail.valign())
    }

    pub fn content_bottom_clearance(&self) -> i32 {
        if self.toolbar.has_css_class("dishy-dock-clearance") {
            DOCK_CLEARANCE
        } else {
            0
        }
    }

    pub fn content_margin_bottom(&self) -> i32 {
        self.toolbar.margin_bottom()
    }

    pub fn set_rail_expanded(&self, expanded: bool) {
        set_rail_expansion(&self.rail, &self.rail_labels, &self.rail_rows, expanded);
    }

    pub fn rail_width_request(&self) -> i32 {
        self.rail.width_request()
    }

    pub fn rail_labels_revealed(&self) -> bool {
        self.rail_labels.iter().all(gtk::Revealer::reveals_child)
    }

    pub fn navigation_button(
        &self,
        style: ToolbarStyle,
        page_id: &str,
    ) -> Option<gtk::ToggleButton> {
        self.navigation_buttons
            .iter()
            .find(|(id, _, _, _)| id == page_id)
            .map(|(_, dock, rail, _)| {
                if style == ToolbarStyle::Dock {
                    dock.clone()
                } else {
                    rail.clone()
                }
            })
    }
}

/// The navigation sections, in Electron's order. Labels are catalog keys;
/// each page is built from its own view model in `build_window`.
const NAV_ITEMS: &[(&str, &str)] = &[
    ("dashboard", "navigation.dashboard"),
    ("satellite", "navigation.satelliteView"),
    ("datausage", "navigation.dataUsage"),
    ("network", "navigation.network"),
    ("speedtest", "navigation.speedTest"),
    ("alignment", "navigation.alignment"),
    ("account", "navigation.account"),
    ("settings", "navigation.settings"),
];

pub fn build_window(
    app: &adw::Application,
    translator: &Translator,
    theme: Theme,
    bounds: Option<WindowBounds>,
    toolbar_style: ToolbarStyle,
) -> ShellWindow {
    install_navigation_css();
    let (width, height) = bounds
        .map(|bounds| (bounds.width, bounds.height))
        .unwrap_or_else(default_bounds);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(translator.text("desktop.gtkWindowTitle"))
        .default_width(width)
        .default_height(height)
        .build();
    window.set_size_request(MIN_WIDTH, MIN_HEIGHT);
    // Closing the window is hiding it; the only quit action is the tray's.
    window.set_hide_on_close(true);

    let stack = adw::ViewStack::new();
    let dashboard = DashboardPage::new(translator, dishy_core::calendar::now_ms());
    stack.add_titled(
        dashboard.widget(),
        Some("dashboard"),
        &translator.text("navigation.dashboard"),
    );
    let sky = SkyPage::new(translator);
    stack.add_titled(
        sky.widget(),
        Some("satellite"),
        &translator.text("navigation.satelliteView"),
    );
    let data_usage = DataUsagePage::new(translator);
    stack.add_titled(
        data_usage.widget(),
        Some("datausage"),
        &translator.text("navigation.dataUsage"),
    );
    let network = NetworkPage::new(translator);
    stack.add_titled(
        network.widget(),
        Some("network"),
        &translator.text("navigation.network"),
    );
    let alignment = AlignmentPage::new(translator);
    stack.add_titled(
        alignment.widget(),
        Some("alignment"),
        &translator.text("navigation.alignment"),
    );
    let speed_test = SpeedTestPage::new(translator);
    stack.add_titled(
        speed_test.widget(),
        Some("speedtest"),
        &translator.text("navigation.speedTest"),
    );
    let settings = SettingsPage::new(translator);
    stack.add_titled(
        settings.widget(),
        Some("settings"),
        &translator.text("navigation.settings"),
    );
    let account = AccountPage::new(translator);
    let account_refresh = account.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        if stack.visible_child_name().as_deref() == Some("account") {
            account_refresh.request_refresh();
        }
    });
    stack.add_titled(
        account.widget(),
        Some("account"),
        &translator.text("navigation.account"),
    );
    let alerts = AlertsPage::new(translator);

    let header = adw::HeaderBar::new();

    let header_status = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    header_status.add_css_class("dim-label");
    header_status.set_widget_name("header-status");
    let connectivity_indicator = status_indicator(translator.text("desktop.appName"));
    connectivity_indicator.set_widget_name("connectivity-indicator");
    let status_area = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status_area.set_valign(gtk::Align::Center);
    status_area.append(&connectivity_indicator);
    status_area.append(&header_status);
    // Electron's top bar: logo and wordmark on the left, status on the right
    // beside the bell and theme toggle; no centred title.
    header.pack_start(&brand(translator));
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let theme_button = gtk::Button::new();
    theme_button.set_widget_name("theme-switcher");
    set_theme_button(&theme_button, translator, theme);
    header.pack_end(&theme_button);
    let notification_button = gtk::MenuButton::builder()
        .child(&crate::line_icons::line_icon(&crate::line_icons::BELL, 16))
        .tooltip_text(translator.text("alerts.title"))
        .build();
    notification_button.set_widget_name("notifications-button");
    notification_button.set_popover(Some(alerts.popover()));
    let notification_badge = gtk::Label::new(None);
    notification_badge.set_widget_name("notifications-badge");
    notification_badge.add_css_class("dishy-notification-badge");
    notification_badge.set_halign(gtk::Align::End);
    notification_badge.set_valign(gtk::Align::Start);
    notification_badge.set_visible(false);
    let notification_control = gtk::Overlay::new();
    notification_control.set_child(Some(&notification_button));
    notification_control.add_overlay(&notification_badge);
    header.pack_end(&notification_control);
    header.pack_end(&status_area);

    let toolbar = adw::ToolbarView::new();
    toolbar.set_hexpand(true);
    toolbar.set_vexpand(true);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    let layout = gtk::Overlay::new();
    layout.set_child(Some(&toolbar));

    let (dock, rail, rail_labels, rail_rows, nav_buttons) = build_navigation(translator, &stack);
    layout.add_overlay(&dock);
    layout.add_overlay(&rail);
    dock.set_halign(gtk::Align::Center);
    dock.set_valign(gtk::Align::End);
    dock.set_margin_bottom(24);
    rail.set_halign(gtk::Align::Start);
    rail.set_valign(gtk::Align::Center);
    rail.set_margin_start(12);
    rail.set_width_request(64);
    rail.set_visible(false);

    let motion = gtk::EventControllerMotion::new();
    let rail_enter = rail.clone();
    let labels_enter = rail_labels.clone();
    let rows_enter = rail_rows.clone();
    motion.connect_enter(move |_, _, _| {
        set_rail_expansion(&rail_enter, &labels_enter, &rows_enter, true);
    });
    let rail_leave = rail.clone();
    let labels_leave = rail_labels.clone();
    let rows_leave = rail_rows.clone();
    motion.connect_leave(move |_| {
        set_rail_expansion(&rail_leave, &labels_leave, &rows_leave, false);
    });
    rail.add_controller(motion);

    let buttons = nav_buttons;
    let buttons_for_updates = buttons.clone();
    stack.connect_visible_child_name_notify(move |stack| {
        set_navigation_active(&buttons_for_updates, stack.visible_child_name());
    });
    set_navigation_active(&buttons, stack.visible_child_name());
    let toolbar_layout = ToolbarLayout {
        dock,
        rail,
        toolbar: toolbar.clone(),
        rail_labels,
        rail_rows,
        navigation_buttons: buttons,
    };
    toolbar_layout.set_style(toolbar_style);
    toolbar_layout.set_rail_expanded(false);

    // Spec 0190: the dark glass sits under everything, and the window wears
    // `dishy-glass` only while the scheme is dark.
    crate::theme::install();
    let glass = crate::theme::GlassBackdrop::new();
    let root = gtk::Overlay::new();
    root.set_child(Some(glass.widget()));
    root.add_overlay(&layout);
    root.set_measure_overlay(&layout, true);
    window.set_content(Some(&root));
    let style = adw::StyleManager::default();
    let apply_glass = {
        let window = window.clone();
        let glass = glass.clone();
        move |dark: bool| {
            if dark {
                window.add_css_class("dishy-glass");
            } else {
                window.remove_css_class("dishy-glass");
            }
            glass.widget().set_visible(dark);
        }
    };
    apply_glass(style.is_dark());
    style.connect_dark_notify(move |style| apply_glass(style.is_dark()));

    ShellWindow {
        window,
        glass,
        stack,
        theme_button,
        notification_button,
        notification_badge,
        header_status,
        connectivity_indicator,
        dashboard,
        sky,
        data_usage,
        network,
        speed_test,
        alignment,
        account,
        alerts,
        settings,
        toolbar_layout,
    }
}

type NavigationButtons = Vec<(String, gtk::ToggleButton, gtk::ToggleButton, gtk::Widget)>;

fn build_navigation(
    translator: &Translator,
    stack: &adw::ViewStack,
) -> (
    gtk::Box,
    gtk::Box,
    Vec<gtk::Revealer>,
    Vec<gtk::Box>,
    NavigationButtons,
) {
    let dock = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    dock.add_css_class("dishy-toolbar-dock");
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 3);
    rail.add_css_class("dishy-toolbar-rail");
    let mut labels = Vec::new();
    let mut rows = Vec::new();
    let mut buttons = Vec::new();
    let mut dock_group: Option<gtk::ToggleButton> = None;
    let mut rail_group: Option<gtk::ToggleButton> = None;

    for (id, key) in NAV_ITEMS {
        let label_text = translator.text(key);
        let dock_button = gtk::ToggleButton::new();
        dock_button.set_widget_name(&format!("toolbar-dock-{id}"));
        dock_button.set_tooltip_text(Some(&label_text));
        dock_button.add_css_class("dishy-toolbar-button");
        let dock_contents = gtk::Box::new(gtk::Orientation::Vertical, 3);
        dock_contents.set_halign(gtk::Align::Center);
        dock_contents.set_valign(gtk::Align::Center);
        let glyph = crate::line_icons::for_page(id);
        dock_contents.append(&crate::line_icons::line_icon(glyph, 22));
        let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        indicator.set_size_request(5, 5);
        indicator.add_css_class("dishy-toolbar-indicator");
        indicator.set_visible(false);
        dock_contents.append(&indicator);
        dock_button.set_child(Some(&dock_contents));
        if let Some(group) = &dock_group {
            dock_button.set_group(Some(group));
        }
        dock_group = Some(dock_button.clone());

        let rail_button = gtk::ToggleButton::new();
        rail_button.set_widget_name(&format!("toolbar-rail-{id}"));
        rail_button.set_tooltip_text(Some(&label_text));
        rail_button.add_css_class("dishy-toolbar-button");
        rail_button.set_halign(gtk::Align::Fill);
        rail_button.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(gtk::Align::Center);
        row.set_valign(gtk::Align::Center);
        row.append(&crate::line_icons::line_icon(glyph, 22));
        let label = gtk::Label::new(Some(&label_text));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        revealer.set_transition_duration(160);
        revealer.set_reveal_child(false);
        revealer.set_child(Some(&label));
        labels.push(revealer.clone());
        row.append(&revealer);
        rows.push(row.clone());
        rail_button.set_child(Some(&row));
        if let Some(group) = &rail_group {
            rail_button.set_group(Some(group));
        }
        rail_group = Some(rail_button.clone());

        let page_stack = stack.clone();
        let page_name = (*id).to_string();
        dock_button.connect_clicked(move |_| page_stack.set_visible_child_name(&page_name));
        let page_stack = stack.clone();
        let page_name = (*id).to_string();
        rail_button.connect_clicked(move |_| page_stack.set_visible_child_name(&page_name));

        dock.append(&dock_button);
        rail.append(&rail_button);
        buttons.push((
            (*id).to_string(),
            dock_button,
            rail_button,
            indicator.upcast(),
        ));
    }

    (dock, rail, labels, rows, buttons)
}

fn set_navigation_active(buttons: &NavigationButtons, active_page: Option<gtk::glib::GString>) {
    if let Some(active_page) = active_page {
        for (id, dock, rail, indicator) in buttons {
            let active = id == active_page.as_str();
            dock.set_active(active);
            rail.set_active(active);
            indicator.set_visible(active);
        }
    }
}

fn set_rail_expansion(
    rail: &gtk::Box,
    labels: &[gtk::Revealer],
    rows: &[gtk::Box],
    expanded: bool,
) {
    rail.set_width_request(if expanded { 214 } else { 64 });
    rail.set_halign(gtk::Align::Start);
    for label in labels {
        label.set_reveal_child(expanded);
    }
    for (row, label) in rows.iter().zip(labels) {
        row.set_halign(if expanded {
            gtk::Align::Fill
        } else {
            gtk::Align::Center
        });
        row.set_spacing(if expanded { 12 } else { 0 });
        label.set_halign(if expanded {
            gtk::Align::Fill
        } else {
            gtk::Align::Center
        });
    }
}

fn install_navigation_css() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(
            "
            .dishy-dock-clearance scrolledwindow.dishy-page-scroller > viewport > *,
            .dishy-dock-clearance preferencespage > scrolledwindow > viewport > *,
            .dishy-dock-clearance statuspage > scrolledwindow > viewport > * {
              margin-bottom: 96px;
            }
            .dishy-dock-clearance .dishy-sky-panel {
              margin-bottom: 80px;
            }
            .dishy-toolbar-dock, .dishy-toolbar-rail {
              background-color: @theme_bg_color;
              border: 1px solid alpha(@theme_fg_color, 0.08);
              box-shadow: none;
            }
            .dishy-toolbar-dock {
              border-radius: 999px;
              padding: 5px 9px;
            }
            .dishy-toolbar-rail {
              border-radius: 25px;
              padding: 9px;
            }
            button.dishy-toolbar-button {
              min-width: 44px;
              min-height: 44px;
              padding: 6px;
              border: 0;
              border-radius: 999px;
              background: transparent;
              color: alpha(@theme_fg_color, 0.72);
              box-shadow: none;
            }
            button.dishy-toolbar-button:hover {
              background: alpha(@theme_fg_color, 0.08);
              color: @theme_fg_color;
            }
            button.dishy-toolbar-button:checked {
              background: alpha(@theme_fg_color, 0.10);
              color: @theme_fg_color;
            }
            .dishy-toolbar-rail button.dishy-toolbar-button {
              padding: 0;
            }
            .dishy-toolbar-indicator {
              border-radius: 999px;
              background: @theme_fg_color;
            }
            .dishy-notification-badge {
              min-width: 16px;
              min-height: 16px;
              padding: 0 3px;
              border-radius: 999px;
              background: @error_color;
              color: white;
              font-size: 10px;
              font-weight: 700;
            }
            .dishy-status-indicator {
              color: @error_color;
              font-size: 12px;
            }
            .dishy-status-online {
              color: #2f9e63;
            }
            .dishy-alert-tabs {
              margin-bottom: 8px;
            }
            ",
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}

pub fn theme_tooltip(translator: &Translator, theme: Theme) -> String {
    let current = translator.text(&format!("theme.{}", theme.name()));
    let next = translator.text(&format!("theme.{}", theme.next().name()));
    translator.text_with(
        "status.switchTheme",
        &[("theme", &current), ("next", &next)],
    )
}

pub fn theme_icon_name(theme: Theme) -> Option<&'static str> {
    match theme {
        Theme::Dark => Some("weather-clear-night-symbolic"),
        Theme::Light => Some("weather-clear-symbolic"),
        Theme::System => None,
    }
}

pub fn set_theme_button(button: &gtk::Button, translator: &Translator, theme: Theme) {
    button.set_tooltip_text(Some(&theme_tooltip(translator, theme)));
    match theme {
        Theme::System => {
            let automatic = gtk::Label::new(Some("A"));
            automatic.add_css_class("heading");
            button.set_child(Some(&automatic));
        }
        _ => {
            if let Some(icon) = theme_icon_name(theme) {
                button.set_icon_name(icon);
            }
        }
    }
}

/// The Dishylink mark (`AppLogo.tsx`: two orbits, a satellite and its dimmer
/// twin, the receiver) and the letter-spaced wordmark.
fn brand(translator: &Translator) -> gtk::Box {
    let logo = gtk::DrawingArea::builder()
        .content_width(28)
        .content_height(28)
        .valign(gtk::Align::Center)
        .build();
    logo.set_widget_name("app-logo");
    logo.set_draw_func(|area, cr, width, height| {
        let color = area.color();
        let (r, g, b) = (
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
        );
        let scale = f64::from(width.min(height)) / 220.0;
        cr.translate(f64::from(width) / 2.0, f64::from(height) / 2.0);
        cr.scale(scale, scale);
        let ring = |cr: &gtk::cairo::Context, alpha: f64, line: f64| {
            cr.set_source_rgba(r, g, b, alpha);
            cr.set_line_width(line);
            let _ = cr.stroke();
        };
        cr.arc(0.0, 0.0, 95.0, 0.0, std::f64::consts::TAU);
        ring(cr, 0.85, 7.0);
        let _ = cr.save();
        cr.rotate(std::f64::consts::FRAC_PI_4);
        cr.scale(95.0, 105.0);
        cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
        let _ = cr.restore();
        ring(cr, 0.4, 3.0);
        for (x, y, radius, alpha) in [(67.0, -67.0, 13.0, 1.0), (-67.0, 67.0, 8.0, 0.55)] {
            cr.set_source_rgba(r, g, b, alpha);
            cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        cr.arc(0.0, 0.0, 17.0, 0.0, std::f64::consts::TAU);
        ring(cr, 1.0, 13.0);
    });
    let wordmark = gtk::Label::new(Some(&translator.text("desktop.appName")));
    wordmark.add_css_class("dishy-wordmark");
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 11);
    brand.set_valign(gtk::Align::Center);
    brand.append(&logo);
    brand.append(&wordmark);
    brand
}

fn status_indicator(device: String) -> gtk::Label {
    let indicator = gtk::Label::new(Some("●"));
    indicator.add_css_class("dishy-status-indicator");
    indicator.set_tooltip_text(Some(&device));
    indicator
}

pub fn set_status_indicator(
    indicator: &gtk::Label,
    app_name: &str,
    online: bool,
    translator: &Translator,
) {
    indicator.remove_css_class("dishy-status-online");
    if online {
        indicator.add_css_class("dishy-status-online");
    }
    let status = if online {
        translator.text("status.online")
    } else {
        translator.text("common.notConnected")
    };
    indicator.set_tooltip_text(Some(&format!("{app_name}: {status}")));
}

pub fn devices_reachable(dish: Option<bool>, router: Option<bool>) -> bool {
    dish == Some(true) && router == Some(true)
}

pub fn header_status_text(
    status: Option<&dishy_core::telemetry::DishStatus>,
    reachable: Option<bool>,
    translator: &Translator,
) -> String {
    // Electron's top bar leads with the connection word.
    let word = translator.text(match reachable {
        Some(true) => "status.online",
        Some(false) => "status.dishUnreachable",
        None => "status.connecting",
    });
    let Some(status) = status else {
        return word;
    };
    let country = status
        .device_info
        .as_ref()
        .and_then(|info| info.country_code.as_deref())
        .filter(|code| !code.trim().is_empty());
    let uptime = status
        .device_state
        .as_ref()
        .and_then(|state| state.uptime_s)
        .map(|seconds| {
            translator.text_with("status.uptime", &[("duration", &format_uptime(seconds.0))])
        });
    [Some(word), country.map(str::to_owned), uptime]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn notification_badge_text(active_count: usize) -> Option<String> {
    match active_count {
        0 => None,
        1..=9 => Some(active_count.to_string()),
        _ => Some("9+".to_owned()),
    }
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    #[test]
    fn theme_tooltip_names_both_the_current_and_next_choice() {
        let text = theme_tooltip(&Translator::new(Locale::En), Theme::Light);
        assert_eq!(text, "Color theme: Light. Switch to Dark.");
    }

    #[test]
    fn theme_icon_follows_the_selected_mode() {
        assert_eq!(
            theme_icon_name(Theme::Dark),
            Some("weather-clear-night-symbolic")
        );
        assert_eq!(
            theme_icon_name(Theme::Light),
            Some("weather-clear-symbolic")
        );
        assert_eq!(theme_icon_name(Theme::System), None);
    }

    #[test]
    // SPEC-TOOLBAR-009: the single connectivity light is green only when both
    // cached device reachability states are positive.
    fn connectivity_requires_both_cached_devices() {
        assert!(!devices_reachable(None, None));
        assert!(!devices_reachable(Some(true), None));
        assert!(!devices_reachable(None, Some(true)));
        assert!(!devices_reachable(Some(true), Some(false)));
        assert!(!devices_reachable(Some(false), Some(true)));
        assert!(devices_reachable(Some(true), Some(true)));
    }

    #[test]
    fn header_status_formats_cached_country_and_uptime() {
        let translator = Translator::new(Locale::En);
        let status = dishy_core::telemetry::DishStatus {
            device_info: Some(dishy_core::telemetry::DishDeviceInfo {
                country_code: Some("ZM".into()),
                ..Default::default()
            }),
            device_state: Some(dishy_core::telemetry::DeviceState {
                uptime_s: Some(dishy_core::telemetry::U64Value(84_420)),
            }),
            ..Default::default()
        };
        // SPEC-DASHP-005: the connection word leads, as Electron's top bar.
        assert_eq!(
            header_status_text(Some(&status), Some(true), &translator),
            "online · ZM · up 23h 27m"
        );
        assert_eq!(
            header_status_text(Some(&status), Some(false), &translator),
            "dish unreachable · ZM · up 23h 27m"
        );
        assert_eq!(header_status_text(None, None, &translator), "connecting");
    }

    #[test]
    // SPEC-ALERT-007: the header badge hides at zero and caps large counts.
    fn notification_badge_is_hidden_at_zero_and_caps_large_counts() {
        assert_eq!(notification_badge_text(0), None);
        assert_eq!(notification_badge_text(1), Some("1".into()));
        assert_eq!(notification_badge_text(9), Some("9".into()));
        assert_eq!(notification_badge_text(10), Some("9+".into()));
    }
}
