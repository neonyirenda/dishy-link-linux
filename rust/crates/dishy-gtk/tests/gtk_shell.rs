//! A real GTK window, when a display is available.
//!
//! CI runs this under Xvfb. On a headless developer machine it simply skips:
//! the pure tests still cover the decisions, but this one proves the window's
//! close policy and the dashboard's first transition are actually wired.
//!
//! GTK may only be initialized from one thread, so this is deliberately a
//! single test. Two `#[test]` functions would race for that one initialization.

use std::collections::BTreeMap;
use std::sync::Arc;

use dishy_cloud::AccountData;
use dishy_core::alert::{AlertEpisode, AlertSnapshot, AlertSource, AlertState, Severity};
use dishy_core::control::ControlAction;
use dishy_core::energy::Coverage;
use dishy_core::meter::ClientMonthlyLimit;
use dishy_core::network::{
    ClientRateTracker, MeshNode, NetworkSnapshot, RadioReading, WifiClient, WifiClientStats,
    WifiConfig,
};
use dishy_core::telemetry::ProtoF64;
use dishy_core::telemetry::{DishDeviceInfo, DishStatus};
use dishy_core::telemetry::{DishObstructionMap, TelemetrySample};
use dishy_core::usage::{UsageBucket, UsageRange, UsageSummary};

use dishy_gtk::cloud_login::build_sign_in_window;
use dishy_gtk::cloud_view::{CloudAction, CloudState};
use dishy_gtk::i18n::{Locale, Translator};
use dishy_gtk::network_view::NetworkCloudAction;
use dishy_gtk::preferences::{Theme, TimeFormat, ToolbarStyle, WindowBounds};
use dishy_gtk::recorder::{ControlReport, RecorderCommand};
use dishy_gtk::settings_view::{AppSettingsState, SettingsAction};
use dishy_gtk::usage_view::DataUsagePage;
use gtk::prelude::*;
use libadwaita::prelude::*;

fn collect_widgets(root: &gtk::Widget, found: &mut Vec<gtk::Widget>) {
    found.push(root.clone());
    let mut child = root.first_child();
    while let Some(widget) = child {
        collect_widgets(&widget, found);
        child = widget.next_sibling();
    }
}

fn find_widget(root: &gtk::Widget, name: &str) -> gtk::Widget {
    let mut widgets = Vec::new();
    collect_widgets(root, &mut widgets);
    widgets
        .into_iter()
        .find(|widget| widget.widget_name() == name)
        .unwrap_or_else(|| panic!("widget {name} exists"))
}

fn find_button(root: &gtk::Widget, predicate: impl Fn(&gtk::Button) -> bool) -> gtk::Button {
    let mut widgets = Vec::new();
    collect_widgets(root, &mut widgets);
    widgets
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(predicate)
        .expect("matching button exists")
}

fn labels(root: &gtk::Widget) -> Vec<String> {
    let mut widgets = Vec::new();
    collect_widgets(root, &mut widgets);
    widgets
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Label>().ok())
        .map(|label| label.label().to_string())
        .collect()
}

// SPEC-SHELL-001: closing the window hides it rather than destroying it, which
// is what leaves the recorder and the rest of the application running for the
// tray's Quit action.
//
// SPEC-DASH-004: the dashboard opens on the searching hero and switches to the
// tiles and charts when the recorder delivers its first sample.
//
// SPEC-DASH-002: the range control accepts only the three chart windows.
//
// SPEC-SKY-006: the sky page waits without a survey, then reveals the scene
// once an obstruction reply arrives, without a display-dependent GL context.
//
// SPEC-USAGE-004: the data-usage page asks the recorder for a summary and
// reveals its bars when one arrives.
//
// SPEC-ODO-006: the data-usage page displays the per-device list and actions.
//
// SPEC-NET-001, SPEC-NET-005, SPEC-NET-006: the network page gates its reads on
// visibility, joins configured mesh nodes, and replaces its roster when the
// router is unreachable.
// SPEC-DASH-007: the six dashboard tiles use only 6-column or 3-column layouts.
// SPEC-USAGE-005, SPEC-NET-007: range and network segmented controls are centered.
// SPEC-ALERT-007, SPEC-TOOLBAR-006, SPEC-TOOLBAR-007, SPEC-TOOLBAR-008:
// the header bell opens a tab-driven popover, theme/status controls use
// cached state, and navigation icons align in both toolbar layouts.
#[test]
fn gtk_shell_hides_on_close_and_dashboard_reveals_on_data() {
    if gtk::init().is_err() {
        eprintln!("skipping GTK window test: no display");
        return;
    }
    if libadwaita::init().is_err() {
        eprintln!("skipping GTK window test: libadwaita unavailable");
        return;
    }

    let app = libadwaita::Application::builder()
        .application_id("com.dishylink.test")
        .build();
    if app.register(None::<&gtk::gio::Cancellable>).is_err() {
        eprintln!("skipping GTK window test: could not register test application");
        return;
    }
    let shell = dishy_gtk::ui::build_window(
        &app,
        &Translator::new(Locale::En),
        Theme::Dark,
        Some(WindowBounds::new(20, 20, 1200, 800)),
        ToolbarStyle::Dock,
    );

    assert!(shell.window.content().is_some());
    assert_eq!(
        shell.theme_button.icon_name().as_deref(),
        Some("weather-clear-night-symbolic")
    );
    assert_eq!(shell.header_status.label(), "");
    assert!(
        !shell
            .connectivity_indicator
            .has_css_class("dishy-status-online")
    );
    let translator = Translator::new(Locale::En);
    dishy_gtk::ui::set_status_indicator(
        &shell.connectivity_indicator,
        &translator.text("desktop.appName"),
        true,
        &translator,
    );
    assert!(
        shell
            .connectivity_indicator
            .has_css_class("dishy-status-online")
    );
    dishy_gtk::ui::set_status_indicator(
        &shell.connectivity_indicator,
        &translator.text("desktop.appName"),
        false,
        &translator,
    );
    assert!(
        !shell
            .connectivity_indicator
            .has_css_class("dishy-status-online")
    );
    let mut status_widgets = Vec::new();
    collect_widgets(&shell.window.clone().upcast(), &mut status_widgets);
    assert_eq!(
        status_widgets
            .iter()
            .filter(|widget| widget.widget_name() == "connectivity-indicator")
            .count(),
        1
    );
    assert!(
        status_widgets
            .iter()
            .all(|widget| widget.widget_name() != "dish-status-indicator"
                && widget.widget_name() != "router-status-indicator")
    );
    // SPEC-GLASS-001: the bundled typefaces are registered with Pango.
    let families: Vec<String> = gtk::Label::new(None)
        .pango_context()
        .list_families()
        .iter()
        .map(|family| family.name().to_string())
        .collect();
    assert!(
        families.iter().any(|name| name == "Barlow"),
        "Barlow registered"
    );
    assert!(
        families.iter().any(|name| name == "IBM Plex Mono"),
        "Plex Mono registered"
    );
    // SPEC-GLASS-002: the window wears the glass only while the scheme is dark.
    let style = libadwaita::StyleManager::default();
    style.set_color_scheme(libadwaita::ColorScheme::ForceDark);
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(shell.window.has_css_class("dishy-glass"));
    assert!(shell.glass.widget().get_visible());
    style.set_color_scheme(libadwaita::ColorScheme::ForceLight);
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(!shell.window.has_css_class("dishy-glass"));
    assert!(!shell.glass.widget().get_visible());
    style.set_color_scheme(libadwaita::ColorScheme::ForceDark);
    // SPEC-GLASS-004: see-through thins the backdrop's base.
    assert_eq!(shell.glass.base_alpha(), 1.0);
    shell.glass.set_see_through(true);
    assert!(shell.glass.base_alpha() < 1.0);
    shell.glass.set_see_through(false);
    // SPEC-GLASS-003: the backdrop takes the link's mood.
    shell.glass.set_mood(dishy_gtk::theme::LinkMood::Offline);
    assert_eq!(shell.glass.mood(), dishy_gtk::theme::LinkMood::Offline);
    // SPEC-GLASS-005: the theme CSS parses without errors.
    let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let provider = gtk::CssProvider::new();
    let seen = errors.clone();
    provider.connect_parsing_error(move |_, _, error| seen.borrow_mut().push(error.to_string()));
    provider.load_from_string(dishy_gtk::theme::THEME_CSS);
    assert!(errors.borrow().is_empty(), "{:?}", errors.borrow());

    // SPEC-ICON-001: the header bell is Electron's line glyph.
    assert_eq!(
        shell
            .notification_button
            .child()
            .map(|child| child.widget_name().to_string())
            .as_deref(),
        Some("line-icon-bell")
    );
    assert!(!shell.notification_badge.is_visible());
    assert!(shell.window.hides_on_close());
    assert_eq!(
        shell.dashboard.widget().visible_child_name().as_deref(),
        Some("searching")
    );

    shell.window.present();
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(shell.window.is_visible());
    let icon_theme =
        gtk::IconTheme::for_display(&gtk::gdk::Display::default().expect("GTK display"));
    // SPEC-ICON-001: each tab carries Electron's line glyph, centred.
    for (page, glyph) in [
        ("dashboard", "dashboard"),
        ("satellite", "planet"),
        ("datausage", "chart"),
        ("network", "network"),
        ("speedtest", "speedometer"),
        ("alignment", "crosshair"),
        ("account", "user"),
        ("settings", "settings"),
    ] {
        let rail_button = shell
            .toolbar_layout
            .navigation_button(ToolbarStyle::Rail, page)
            .expect("page has a rail button");
        let row = rail_button
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .expect("rail button contains a row");
        assert_eq!(row.valign(), gtk::Align::Center);
        let icon = row.first_child().expect("rail row contains its icon");
        assert_eq!(icon.widget_name(), format!("line-icon-{glyph}"));
        assert_eq!(icon.valign(), gtk::Align::Center);
        assert_eq!(icon.halign(), gtk::Align::Center);
        assert_eq!(row.halign(), gtk::Align::Center);
        assert_eq!(row.spacing(), 0);
    }
    assert!(icon_theme.has_icon("weather-clear-symbolic"));
    assert!(icon_theme.has_icon("preferences-system-notifications-symbolic"));
    dishy_gtk::ui::set_theme_button(
        &shell.theme_button,
        &Translator::new(Locale::En),
        Theme::System,
    );
    let automatic = shell
        .theme_button
        .child()
        .and_then(|child| child.downcast::<gtk::Label>().ok())
        .expect("automatic theme uses its A marker");
    assert_eq!(automatic.label(), "A");
    dishy_gtk::ui::set_theme_button(
        &shell.theme_button,
        &Translator::new(Locale::En),
        Theme::Dark,
    );
    for name in ["usage-range-control", "network-tabs", "settings-tabs"] {
        assert_eq!(
            find_widget(&shell.window.clone().upcast(), name).halign(),
            gtk::Align::Center
        );
    }
    assert!(!shell.alerts.is_visible());
    shell.notification_button.set_active(true);
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(shell.alerts.is_visible());
    assert_eq!(
        shell.stack.visible_child_name().as_deref(),
        Some("dashboard")
    );
    let alerts_tabs = find_widget(shell.alerts.widget().upcast_ref(), "alerts-tabs")
        .downcast::<gtk::StackSwitcher>()
        .expect("alerts tabs");
    assert_eq!(alerts_tabs.halign(), gtk::Align::Center);
    assert_eq!(shell.alerts.selected_view().as_deref(), Some("active"));
    let mut alert_widgets = Vec::new();
    collect_widgets(shell.alerts.widget().upcast_ref(), &mut alert_widgets);
    assert!(
        alert_widgets
            .iter()
            .all(|widget| widget.widget_name() != "alerts-view-select")
    );
    assert!(labels(shell.alerts.widget().upcast_ref()).contains(&"Alerts".to_owned()));
    find_button(shell.alerts.widget().upcast_ref(), |button| {
        labels(button.upcast_ref())
            .iter()
            .any(|label| label == "Status")
    })
    .emit_clicked();
    assert_eq!(shell.alerts.selected_view().as_deref(), Some("status"));
    shell.notification_button.set_active(false);
    // SPEC-TOOLBAR-003, SPEC-TOOLBAR-004, SPEC-TOOLBAR-005: the floating dock
    // and expanding rail share active-page state; switching layout is local.
    let selected_page = shell.stack.visible_child_name();
    assert_eq!(
        shell.toolbar_layout.dock_alignment(),
        (gtk::Align::Center, gtk::Align::End)
    );
    assert_eq!(
        shell.toolbar_layout.rail_alignment(),
        (gtk::Align::Start, gtk::Align::Center)
    );
    assert_eq!(shell.toolbar_layout.content_bottom_clearance(), 96);
    // SPEC-TOOLBAR-010: the clearance pads pages, not the content area, so
    // the sky view reaches the bottom of the window behind the dock.
    assert_eq!(shell.toolbar_layout.content_margin_bottom(), 0);
    let dashboard_dock = shell
        .toolbar_layout
        .navigation_button(ToolbarStyle::Dock, "dashboard")
        .expect("dock has Dashboard");
    assert!(dashboard_dock.is_active());
    let dock_contents = dashboard_dock.child().expect("dock tile contents");
    let dock_icon = dock_contents
        .first_child()
        .expect("dock tile uses its navigation icon");
    assert_eq!(dock_icon.widget_name(), "line-icon-dashboard");
    assert!(
        dock_contents
            .last_child()
            .is_some_and(|indicator| indicator.is_visible())
    );
    shell.toolbar_layout.set_style(ToolbarStyle::Rail);
    assert_eq!(shell.toolbar_layout.content_bottom_clearance(), 0);
    assert_eq!(shell.stack.visible_child_name(), selected_page);
    assert!(!shell.toolbar_layout.is_dock_visible());
    assert!(shell.toolbar_layout.is_rail_visible());
    while gtk::glib::MainContext::default().iteration(false) {}
    for page in [
        "dashboard",
        "satellite",
        "datausage",
        "network",
        "speedtest",
        "alignment",
        "account",
        "settings",
    ] {
        let button = shell
            .toolbar_layout
            .navigation_button(ToolbarStyle::Rail, page)
            .expect("rail navigation button");
        let icon = button
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
            .and_then(|row| row.first_child())
            .expect("rail button icon");
        let icon_bounds = icon
            .compute_bounds(&button)
            .expect("rail icon bounds relative to button");
        let icon_center = icon_bounds.x() + icon_bounds.width() / 2.0;
        assert!(
            (icon_center - button.width() as f32 / 2.0).abs() <= 1.0,
            "rail icon for {page} should be centered between the button borders"
        );
    }
    let network_link = shell
        .toolbar_layout
        .navigation_button(ToolbarStyle::Rail, "network")
        .expect("rail contains the Network icon");
    assert_eq!(network_link.tooltip_text().as_deref(), Some("Network"));
    assert_eq!(shell.toolbar_layout.rail_width_request(), 64);
    assert!(!shell.toolbar_layout.rail_labels_revealed());
    shell.toolbar_layout.set_rail_expanded(true);
    assert_eq!(shell.toolbar_layout.rail_width_request(), 214);
    assert!(shell.toolbar_layout.rail_labels_revealed());
    let expanded_row = network_link
        .child()
        .and_then(|child| child.downcast::<gtk::Box>().ok())
        .expect("expanded rail row");
    assert_eq!(expanded_row.halign(), gtk::Align::Fill);
    shell.toolbar_layout.set_rail_expanded(false);
    assert_eq!(expanded_row.halign(), gtk::Align::Center);
    network_link.emit_clicked();
    assert_eq!(shell.stack.visible_child_name().as_deref(), Some("network"));
    assert!(network_link.is_active());
    let dock_link = shell
        .toolbar_layout
        .navigation_button(ToolbarStyle::Dock, "network")
        .expect("dock contains the Network icon");
    assert!(dock_link.is_active());
    let selected_page = shell.stack.visible_child_name();
    shell.toolbar_layout.set_style(ToolbarStyle::Dock);
    assert_eq!(shell.stack.visible_child_name(), selected_page);
    assert!(shell.toolbar_layout.is_dock_visible());
    // SPEC-CTRL-008, SPEC-CTRL-009: maintenance buttons share a compact height,
    // and router form fields and Save actions use consistent dimensions.
    let factory_reset = find_widget(shell.settings.widget(), "router-factory-reset");
    assert_eq!(factory_reset.height_request(), 36);
    assert_eq!(factory_reset.valign(), gtk::Align::Center);
    let mut settings_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut settings_widgets);
    let maintenance_buttons: Vec<_> = settings_widgets
        .iter()
        .filter(|widget| widget.has_css_class("dishy-maintenance-action"))
        .collect();
    assert!(maintenance_buttons.len() >= 7);
    assert!(
        maintenance_buttons
            .iter()
            .all(|button| button.height_request() == 36 && button.valign() == gtk::Align::Center)
    );
    for name in [
        "router-address-entry",
        "router-dns-0",
        "router-dns-1",
        "router-dns-2",
        "router-dns-3",
        "router-subnet-password",
        "router-mesh-name",
    ] {
        let input = find_widget(shell.settings.widget(), name);
        assert_eq!(input.width_request(), 220, "{name} width");
        assert_eq!(input.height_request(), 36, "{name} height");
    }
    for name in [
        "router-address-save",
        "router-dns-save",
        "router-subnet-save",
        "router-mesh-save",
    ] {
        let button = find_widget(shell.settings.widget(), name);
        assert_eq!(button.width_request(), 80, "{name} width");
        assert_eq!(button.height_request(), 36, "{name} height");
    }
    assert!(!shell.toolbar_layout.is_rail_visible());

    // SPEC-CLOUD-008: the login window has the same visible top-right close
    // affordance as the account surface and returns to its parent window.
    let (login_window, close_login) = build_sign_in_window(
        &shell.window,
        "Sign in to Starlink",
        "Close",
        &gtk::Label::new(Some("Sign-in content")),
    );
    login_window.present();
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(login_window.is_visible());
    assert_eq!(close_login.label().as_deref(), Some("×"));
    close_login.emit_clicked();
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(!login_window.is_visible());
    assert!(shell.window.is_visible());

    let now_ms = 1_781_524_800_000;
    shell.dashboard.set_samples(Arc::new(vec![TelemetrySample {
        timestamp_ms: now_ms - 1_000,
        latency_ms: Some(24.0),
        drop_rate: 0.01,
        downlink_bps: 50_000_000.0,
        uplink_bps: 5_000_000.0,
        power_w: 42.0,
        router_latency_ms: Some(25.0),
        router_ping_success_percent: Some(99.0),
    }]));
    shell.dashboard.refresh(now_ms);
    // SPEC-DETAIL-001, SPEC-DETAIL-005, SPEC-DETAIL-006: each tile detail opens,
    // and the only recorder work it asks for is a local summary read.
    let (detail_commands, detail_rx) = std::sync::mpsc::channel();
    shell.dashboard.set_command_sender(Some(detail_commands));
    for kind in [
        dishy_gtk::stat_detail_model::DetailKind::Download,
        dishy_gtk::stat_detail_model::DetailKind::Latency,
        dishy_gtk::stat_detail_model::DetailKind::Power,
        dishy_gtk::stat_detail_model::DetailKind::PingSuccess,
    ] {
        shell.dashboard.open_detail(kind);
        assert_eq!(shell.dashboard.open_detail_kind(), Some(kind));
    }
    shell.dashboard.refresh(now_ms + 1_000);
    for command in detail_rx.try_iter() {
        assert!(
            matches!(
                command,
                dishy_gtk::recorder::RecorderCommand::DetailEnergy(_)
                    | dishy_gtk::recorder::RecorderCommand::LatencySummary(_)
            ),
            "a detail asked for {command:?}"
        );
    }
    // SPEC-SPIN-001: the loading spinner is 32 px and centred, never the
    // page's full allocation.
    shell
        .account
        .set_state(dishy_gtk::cloud_view::CloudState::Loading);
    let spinner = find_widget(shell.account.widget(), "account-loading");
    assert_eq!(spinner.size_request(), (32, 32));
    assert_eq!(
        (spinner.halign(), spinner.valign()),
        (gtk::Align::Center, gtk::Align::Center)
    );
    assert!(!spinner.hexpands() && !spinner.vexpands());

    // SPEC-ACCTDEV-001: the account page lists each dish and its routers from
    // the account data the cloud worker fetched.
    shell.account.set_state(dishy_gtk::cloud_view::CloudState::Ready {
        account: dishy_cloud::AccountData {
            identity: None,
            service_line: Some(serde_json::json!({ "content": { "userTerminals": [
                { "userTerminalId": "01000000-00000000-004c8bb9", "routers": [{ "routerId": "010000000000000000ab12cd" }] }
            ]}})),
            device_telemetry: serde_json::json!({}),
        },
        usage: serde_json::json!({}),
    });
    assert_eq!(
        shell.account.device_names(),
        ["STARLINK 4C8BB9", "Router AB12CD"]
    );
    // SPEC-CLOUD-006: the ready account page scrolls instead of growing, so a
    // long device list never pushes the navigation bar out of the window.
    let (minimum_height, _, _, _) = shell
        .account
        .widget()
        .measure(gtk::Orientation::Vertical, 900);
    assert!(
        minimum_height < 300,
        "account page demands {minimum_height}px"
    );
    for (name, page) in [
        (
            "dashboard",
            shell.dashboard.widget().clone().upcast::<gtk::Widget>(),
        ),
        ("data usage", shell.data_usage.widget().clone().upcast()),
        ("network", shell.network.widget().clone().upcast()),
        ("speed test", shell.speed_test.widget().clone().upcast()),
        ("alignment", shell.alignment.widget().clone().upcast()),
        ("settings", shell.settings.widget().clone().upcast()),
    ] {
        let (minimum_height, _, _, _) = page.measure(gtk::Orientation::Vertical, 900);
        assert!(
            minimum_height < 300,
            "{name} page demands {minimum_height}px"
        );
    }
    // SPEC-MERGE-004: a duplicate-device proposal shows on Data Usage, and the
    // answer goes to the recorder as a typed command.
    let (merge_commands, merge_rx) = std::sync::mpsc::channel();
    shell.data_usage.set_command_sender(Some(merge_commands));
    let total = |id: i64, last_seen_ms: i64| dishy_core::odometer::ClientTotal {
        client_id: Some(id),
        mac_address: format!("aa:00:00:00:00:0{id}"),
        name: Some("Laptop".into()),
        ip_address: None,
        rx_bytes: 1_000,
        tx_bytes: 0,
        since_ms: now_ms - 86_400_000,
        last_seen_ms,
        months: None,
    };
    shell
        .data_usage
        .set_client_totals(vec![total(1, now_ms - 3_600_000), total(2, now_ms)]);
    shell
        .data_usage
        .set_merge_candidates(vec![dishy_core::odometer::MergeCandidate {
            from_key: "1".into(),
            to_key: "2".into(),
            detail: "Laptop".into(),
            folds_bytes: true,
            result_rx_bytes: 2_000,
            result_tx_bytes: 0,
        }]);
    assert!(shell.data_usage.merge_prompt_visible());
    let _ = merge_rx.try_iter().count();
    shell.data_usage.answer_merge(true);
    assert!(merge_rx.try_iter().any(|command| matches!(
        command,
        dishy_gtk::recorder::RecorderCommand::MergeClients { ref from, ref to } if from == "1" && to == "2"
    )));
    assert!(
        !shell.data_usage.merge_prompt_visible(),
        "nothing left to review"
    );
    shell.data_usage.set_command_sender(None);
    // SPEC-BEAM-001: the speed test opens on the Starlink view, and the switch
    // moves to the gauge.
    let speed_root: gtk::Widget = shell.speed_test.widget().clone().upcast();
    let starlink_view = find_button(&speed_root, |button| {
        button.label().as_deref() == Some("Starlink")
    })
    .downcast::<gtk::ToggleButton>()
    .expect("a toggle");
    let gauge_view = find_button(&speed_root, |button| {
        button.label().as_deref() == Some("Gauge")
    })
    .downcast::<gtk::ToggleButton>()
    .expect("a toggle");
    assert!(starlink_view.is_active() && !gauge_view.is_active());
    assert!(shell.speed_test.is_beam_view());
    gauge_view.set_active(true);
    assert!(!starlink_view.is_active());
    assert!(!shell.speed_test.is_beam_view());
    starlink_view.set_active(true);
    assert!(shell.speed_test.is_beam_view());

    // SPEC-LOC-001: the device-location button asks once, reads Locating…
    // with both sources disabled, and a failure shows the catalog's note.
    let setup = dishy_gtk::location_setup::LocationSetup::new(&dishy_gtk::i18n::Translator::new(
        dishy_gtk::i18n::Locale::En,
    ));
    let device_requests = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = device_requests.clone();
    setup.connect_device_requested(move || seen.set(seen.get() + 1));
    let device_button = find_button(setup.widget().upcast_ref(), |button| {
        button.label().as_deref() == Some("Use this device location")
    });
    let ip_button = find_button(setup.widget().upcast_ref(), |button| {
        button.label().is_some_and(|label| label.contains("IP"))
    });
    device_button.emit_clicked();
    assert_eq!(device_requests.get(), 1);
    assert_eq!(device_button.label().as_deref(), Some("Locating…"));
    assert!(!device_button.is_sensitive() && !ip_button.is_sensitive());
    setup.device_location_finished(Err(()));
    assert!(device_button.is_sensitive() && ip_button.is_sensitive());
    assert_eq!(
        device_button.label().as_deref(),
        Some("Use this device location")
    );
    assert!(
        labels(setup.widget().upcast_ref())
            .iter()
            .any(|label| label.starts_with("This device can't resolve its position"))
    );

    // SPEC-LAPSE-002, SPEC-LAPSE-003: scrubbing to a snapshot shows history and
    // survives a live reply; the live stop returns to the live map.
    shell
        .sky
        .set_obstruction_snapshots(vec![dishy_core::store::ObstructionSnapshot {
            taken_at_ms: now_ms - 3_600_000,
            grid_size: 2,
            packed_cells: dishy_core::store::pack_cells(&[1.0, 0.5, 1.0, 0.5]),
            max_theta_deg: Some(80.0),
        }]);
    assert!(!shell.sky.is_viewing_history());
    shell.sky.scrub_to(Some(0));
    assert!(shell.sky.is_viewing_history());
    assert!(
        labels(shell.sky.widget().upcast_ref())
            .iter()
            .any(|label| label.starts_with("Viewing the obstruction map as of"))
    );
    shell.sky.set_obstruction(None);
    assert!(
        shell.sky.is_viewing_history(),
        "a live reply does not pull the view back"
    );
    shell.sky.scrub_to(None);
    assert!(!shell.sky.is_viewing_history());
    // SPEC-SOUND-004: the menu header counts alerts, and the mute toggle
    // reports a user's change but not the stored choice being restored.
    assert!(
        shell.alerts.header_text().contains("all healthy")
            || shell.alerts.header_text().contains("active alert")
    );
    let sound_changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = sound_changes.clone();
    shell
        .alerts
        .connect_sound_toggled(move |on| seen.borrow_mut().push(on));
    shell.alerts.set_sound_on(true);
    assert!(sound_changes.borrow().is_empty());
    shell.alerts.toggle_sound();
    assert_eq!(*sound_changes.borrow(), [false]);
    // SPEC-OUTAGE-005: the Events tab lists dish events and unmatched reports,
    // and opening the menu asks the recorder only for its local event log.
    let (event_commands, event_rx) = std::sync::mpsc::channel();
    shell.alerts.set_command_sender(Some(event_commands));
    shell
        .alerts
        .set_events(&dishy_gtk::recorder::EventsSnapshot {
            events: vec![dishy_core::telemetry::OutageEvent {
                start_ms: now_ms - 600_000,
                duration_ms: 12_000,
                cause: "EVENT_REASON_NO_PINGS".into(),
                severity: dishy_core::telemetry::Severity::Critical,
                band_switch: None,
            }],
            reports: Vec::new(),
        });
    assert_eq!(shell.alerts.event_row_count(), 1);
    assert!(
        labels(shell.alerts.popover().upcast_ref())
            .iter()
            .any(|label| label == "Ping Network Interruption")
    );
    shell.alerts.popover().emit_by_name::<()>("show", &[]);
    assert!(matches!(
        event_rx.try_recv(),
        Ok(dishy_gtk::recorder::RecorderCommand::GetEvents)
    ));
    shell.alerts.set_command_sender(None);
    shell.dashboard.set_command_sender(None);
    // SPEC-TERM-001: the terminal card appears once a status reply exists, lists
    // the dish's facts, and opens its expanded window.
    assert!(!shell.dashboard.terminal_visible());
    let terminal_status: dishy_core::telemetry::DishStatus =
        serde_json::from_value(serde_json::json!({
            "deviceInfo": { "id": "ut-test-dish", "hardwareVersion": "rev4_prod1" },
            "isSnrAboveNoiseFloor": true
        }))
        .expect("status");
    shell.dashboard.set_status(Some(terminal_status));
    shell.dashboard.refresh(now_ms + 2_000);
    assert!(shell.dashboard.terminal_visible());
    let dashboard_labels = labels(shell.dashboard.widget().upcast_ref());
    assert!(dashboard_labels.iter().any(|label| label == "Standard 4"));
    assert!(dashboard_labels.iter().any(|label| label == "ut-test-dish"));
    shell.dashboard.open_terminal();
    while gtk::glib::MainContext::default().iteration(false) {}
    let tile_grid = find_widget(&shell.window.clone().upcast(), "dashboard-stats-grid")
        .downcast::<gtk::FlowBox>()
        .expect("dashboard stat grid is a FlowBox");
    assert_eq!(
        tile_grid.min_children_per_line(),
        tile_grid.max_children_per_line()
    );
    // SPEC-DASHP-001: six tiles and two columns when wide, three and stacked
    // when narrow, switched by the breakpoint.
    shell.dashboard.set_narrow(false);
    assert_eq!(tile_grid.min_children_per_line(), 6);
    assert!(shell.dashboard.is_two_column());
    shell.dashboard.set_narrow(true);
    assert_eq!(tile_grid.min_children_per_line(), 3);
    assert!(!shell.dashboard.is_two_column());
    shell.dashboard.set_narrow(false);
    // SPEC-DASHP-002: every tile but sky obstructed carries the chevron.
    assert_eq!(
        shell.dashboard.tile_chevrons(),
        [true, true, true, true, true, false]
    );
    assert_eq!(
        shell.dashboard.widget().visible_child_name().as_deref(),
        Some("dashboard")
    );
    assert_eq!(shell.dashboard.window_minutes(), 15);
    shell.dashboard.set_window_minutes(60);
    assert_eq!(shell.dashboard.window_minutes(), 60);
    shell.dashboard.set_window_minutes(60);
    assert_eq!(shell.dashboard.window_minutes(), 60);
    shell.dashboard.set_window_minutes(999);
    assert_eq!(shell.dashboard.window_minutes(), 60);

    assert_eq!(
        shell.sky.widget().visible_child_name().as_deref(),
        Some("waiting")
    );
    let mut snr = vec![1.0_f64; 8 * 8];
    snr[0] = 0.0;
    shell.sky.set_obstruction(Some(DishObstructionMap {
        num_rows: 8,
        num_cols: 8,
        snr,
        max_theta_deg: Some(80.0),
        map_reference_frame: Some("FRAME_EARTH".into()),
    }));
    assert_eq!(
        shell.sky.widget().visible_child_name().as_deref(),
        Some("sky")
    );
    // SPEC-DASHP-003: the Obstructions card waits for a map, then shows the
    // dome, and its link opens the Sky page.
    shell.dashboard.set_obstruction(None);
    assert!(shell.dashboard.obstruction_waiting());
    let mut card_snr = vec![1.0_f64; 8 * 8];
    card_snr[0] = 0.0;
    shell.dashboard.set_obstruction(Some(DishObstructionMap {
        num_rows: 8,
        num_cols: 8,
        snr: card_snr,
        max_theta_deg: Some(80.0),
        map_reference_frame: Some("FRAME_EARTH".into()),
    }));
    assert!(!shell.dashboard.obstruction_waiting());
    let opened_sky = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = opened_sky.clone();
    shell.dashboard.connect_open_sky(move || seen.set(true));
    shell.dashboard.activate_open_sky();
    assert!(opened_sky.get());
    // SPEC-DASHP-004: the Events & outages card lists the recorder's events.
    shell
        .dashboard
        .set_events(&dishy_gtk::recorder::EventsSnapshot {
            events: vec![
                dishy_core::telemetry::OutageEvent {
                    start_ms: now_ms - 600_000,
                    duration_ms: 12_000,
                    cause: "EVENT_REASON_NO_PINGS".into(),
                    severity: dishy_core::telemetry::Severity::Critical,
                    band_switch: None,
                },
                dishy_core::telemetry::OutageEvent {
                    start_ms: now_ms - 300_000,
                    duration_ms: 1_000,
                    cause: "EVENT_REASON_OBSTRUCTED".into(),
                    severity: dishy_core::telemetry::Severity::Warning,
                    band_switch: None,
                },
            ],
            reports: Vec::new(),
        });
    assert_eq!(shell.dashboard.event_row_count(), 2);
    let event_labels = labels(shell.dashboard.widget().upcast_ref());
    assert!(event_labels.iter().any(|label| label == "2 events"));
    assert!(
        event_labels
            .iter()
            .any(|label| label == "Ping Network Interruption")
    );
    shell
        .dashboard
        .set_events(&dishy_gtk::recorder::EventsSnapshot::default());
    assert_eq!(shell.dashboard.event_row_count(), 0);

    assert_eq!(shell.data_usage.range(), UsageRange::Today);
    let (commands, requested) = std::sync::mpsc::channel();
    shell.data_usage.set_command_sender(Some(commands));
    assert!(matches!(
        requested.try_recv(),
        Ok(RecorderCommand::Usage(UsageRange::Today))
    ));
    assert!(matches!(
        requested.try_recv(),
        Ok(RecorderCommand::GetClientTotals)
    ));
    // SPEC-METER-005: the monthly cap adds no hardware request to the existing
    // data-usage demand gate.
    assert!(requested.try_recv().is_err());
    shell.data_usage.set_summary(UsageSummary {
        range: UsageRange::Today,
        total_kwh: 0.1,
        total_down_gb: 1.5,
        total_up_gb: 0.25,
        coverage: Coverage {
            sampled_seconds: 1_800,
            expected_seconds: 3_600,
            fraction: 0.5,
        },
        buckets: vec![UsageBucket {
            t: now_ms / 1_000 - 3_600,
            kwh: Some(0.1),
            down_gb: Some(1.5),
            up_gb: Some(0.25),
            sampled_seconds: 1_800,
            expected_seconds: 3_600,
        }],
    });
    assert!(shell.data_usage.shows_data());

    shell
        .data_usage
        .set_client_totals(vec![dishy_core::odometer::ClientTotal {
            client_id: Some(42),
            mac_address: "10:00:20:11:22:33".into(),
            name: Some("Neo's iPhone".into()),
            ip_address: Some("192.168.1.100".into()),
            rx_bytes: 1_500_000_000,
            tx_bytes: 500_000_000,
            since_ms: now_ms - 86_400_000,
            last_seen_ms: now_ms,
            months: None,
        }]);
    assert_eq!(shell.data_usage.device_count(), 1);

    let (device_actions, device_action_rx) = std::sync::mpsc::channel();
    shell.data_usage.set_command_sender(Some(device_actions));
    while device_action_rx.try_recv().is_ok() {}
    shell.data_usage.set_client_totals(vec![
        dishy_core::odometer::ClientTotal {
            client_id: Some(42),
            mac_address: "10:00:20:11:22:33".into(),
            name: Some("Neo's iPhone".into()),
            ip_address: Some("192.168.1.100".into()),
            rx_bytes: 1_500_000_000,
            tx_bytes: 500_000_000,
            since_ms: now_ms - 86_400_000,
            last_seen_ms: now_ms,
            months: None,
        },
        dishy_core::odometer::ClientTotal {
            client_id: Some(43),
            mac_address: "00:00:0c:44:55:66".into(),
            name: Some("Office laptop".into()),
            ip_address: Some("192.168.1.101".into()),
            rx_bytes: 100,
            tx_bytes: 50,
            since_ms: now_ms - 86_400_000,
            last_seen_ms: now_ms,
            months: None,
        },
    ]);
    assert_eq!(shell.data_usage.device_count(), 2);
    let rendered_labels = labels(shell.data_usage.widget());
    let high_usage_position = rendered_labels
        .iter()
        .position(|label| label == "Neo's iPhone")
        .expect("high usage device is rendered");
    let low_usage_position = rendered_labels
        .iter()
        .position(|label| label == "Office laptop")
        .expect("low usage device is rendered");
    assert!(
        high_usage_position < low_usage_position,
        "the higher usage row should render first"
    );

    // SPEC-METER-004: the monthly cap is a local recorder command, accepts
    // positive whole GB only, and the page marks a reached cap without a pause.
    let set_limit_tip = Translator::new(Locale::En)
        .text_with("devices.setMonthlyLimit", &[("name", "Neo's iPhone")]);
    find_button(shell.data_usage.widget(), |button| {
        button.tooltip_text().as_deref() == Some(set_limit_tip.as_str())
    })
    .emit_clicked();
    let limit_dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
        .find(|window| window.widget_name() == "monthly-limit-dialog" && window.is_visible())
        .expect("monthly cap editor opens");
    let dialog_content = limit_dialog.child().expect("dialog content");
    let mut dialog_widgets = Vec::new();
    collect_widgets(&dialog_content, &mut dialog_widgets);
    let limit_entry = dialog_widgets
        .iter()
        .find(|widget| widget.widget_name() == "monthly-limit-gb")
        .and_then(|widget| widget.clone().downcast::<gtk::Entry>().ok())
        .expect("monthly cap entry exists");
    let save_limit = find_button(&dialog_content, |button| {
        button.widget_name() == "monthly-limit-save"
    });
    limit_entry.set_text("0");
    assert!(!save_limit.is_sensitive());
    limit_entry.set_text("15");
    save_limit.emit_clicked();
    assert!(matches!(
        device_action_rx.try_recv(),
        Ok(RecorderCommand::SetClientMonthlyLimit { client_key, limit_bytes })
            if client_key == "42" && limit_bytes == 15_000_000_000
    ));
    shell.data_usage.set_client_monthly_limits(vec![
        ClientMonthlyLimit::new("42", 1_000_000_000).expect("valid cap"),
    ]);
    assert!(
        labels(shell.data_usage.widget())
            .iter()
            .any(|label| label.contains("Limit reached"))
    );
    shell
        .data_usage
        .set_monthly_limit_error(Some("disk full".into()));
    assert!(
        labels(shell.data_usage.widget())
            .iter()
            .any(|label| label.contains("Failed: disk full"))
    );
    shell.data_usage.set_client_monthly_limits(vec![
        ClientMonthlyLimit::new("42", 1_000_000_000).expect("valid cap"),
    ]);
    assert!(
        labels(shell.data_usage.widget())
            .iter()
            .any(|label| label.contains("Failed: disk full")),
        "a routine limits refresh must not hide a failed write"
    );
    shell.data_usage.set_monthly_limit_error(None);

    let edit_limit_tip = Translator::new(Locale::En)
        .text_with("devices.editMonthlyLimit", &[("name", "Neo's iPhone")]);
    find_button(shell.data_usage.widget(), |button| {
        button.tooltip_text().as_deref() == Some(edit_limit_tip.as_str())
    })
    .emit_clicked();
    let limit_dialog = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Window>().ok())
        .find(|window| window.widget_name() == "monthly-limit-dialog" && window.is_visible())
        .expect("monthly cap editor opens");
    let dialog_content = limit_dialog.child().expect("dialog content");
    find_button(&dialog_content, |button| {
        button.widget_name() == "monthly-limit-remove"
    })
    .emit_clicked();
    assert!(matches!(
        device_action_rx.try_recv(),
        Ok(RecorderCommand::RemoveClientMonthlyLimit(key)) if key == "42"
    ));
    let delete_with_limit_tip = Translator::new(Locale::En).text_with(
        "devices.deleteUsageAndLimitFor",
        &[("name", "Neo's iPhone")],
    );
    assert!(
        find_button(shell.data_usage.widget(), |button| {
            button.tooltip_text().as_deref() == Some(delete_with_limit_tip.as_str())
        })
        .is_sensitive()
    );
    shell.data_usage.set_client_monthly_limits(Vec::new());

    let reset_text =
        Translator::new(Locale::En).text_with("devices.resetUsageFor", &[("name", "Neo's iPhone")]);
    find_button(shell.data_usage.widget(), |button| {
        button.tooltip_text().as_deref() == Some(reset_text.as_str())
    })
    .emit_clicked();
    assert!(matches!(
        device_action_rx.try_recv(),
        Ok(RecorderCommand::ResetClientTotal(key)) if key == "42"
    ));

    let remove_text = Translator::new(Locale::En)
        .text_with("devices.deleteUsageFor", &[("name", "Office laptop")]);
    find_button(shell.data_usage.widget(), |button| {
        button.tooltip_text().as_deref() == Some(remove_text.as_str())
    })
    .emit_clicked();
    assert!(matches!(
        device_action_rx.try_recv(),
        Ok(RecorderCommand::RemoveClientTotal(key)) if key == "43"
    ));

    find_button(shell.data_usage.widget(), |button| {
        button.label().as_deref() == Some("Clear all")
    })
    .emit_clicked();
    find_button(shell.data_usage.widget(), |button| {
        button.label().as_deref() == Some("Clear all usage records and monthly limits?")
    })
    .emit_clicked();
    assert!(matches!(
        device_action_rx.try_recv(),
        Ok(RecorderCommand::ClearClientTotals)
    ));

    let demand_page = DataUsagePage::new(&Translator::new(Locale::En));
    let demand_window = gtk::Window::new();
    demand_window.set_child(Some(demand_page.widget()));
    let (demand_commands, demand_rx) = std::sync::mpsc::channel();
    demand_page.set_command_sender(Some(demand_commands));
    while demand_rx.try_recv().is_ok() {}
    demand_window.present();
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(matches!(
        demand_rx.try_recv(),
        Ok(RecorderCommand::DeviceUsageDemand(true))
    ));
    assert!(matches!(
        demand_rx.try_recv(),
        Ok(RecorderCommand::Usage(UsageRange::Today))
    ));
    assert!(matches!(
        demand_rx.try_recv(),
        Ok(RecorderCommand::GetClientTotals)
    ));
    demand_window.set_visible(false);
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(matches!(
        demand_rx.try_recv(),
        Ok(RecorderCommand::DeviceUsageDemand(false))
    ));

    assert!(shell.network.is_waiting());
    let (network_commands, network_requested) = std::sync::mpsc::channel();
    shell.network.set_command_sender(Some(network_commands));
    shell.network.set_active(true);
    assert!(matches!(
        network_requested.try_recv(),
        Ok(RecorderCommand::NetworkDemand(true))
    ));
    shell.network.set_active(false);
    assert!(matches!(
        network_requested.try_recv(),
        Ok(RecorderCommand::NetworkDemand(false))
    ));

    let router = WifiClient {
        role: Some("CONTROLLER".into()),
        mac_address: Some("router-mac".into()),
        device_id: Some("Router-0001".into()),
        ..WifiClient::default()
    };
    let mesh = WifiClient {
        role: Some("MESH".into()),
        mac_address: Some("mesh-mac".into()),
        device_id: Some("Router-0002".into()),
        ..WifiClient::default()
    };
    let unpaired_mesh = WifiClient {
        role: Some("MESH".into()),
        mac_address: Some("unpaired-mesh-mac".into()),
        device_id: Some("Router-0004".into()),
        ..WifiClient::default()
    };
    let client = WifiClient {
        role: Some("CLIENT".into()),
        client_id: Some(7),
        mac_address: Some("client-mac".into()),
        ip_address: Some("192.168.1.20".into()),
        iface: Some("RF_5GHZ".into()),
        signal_strength: Some(ProtoF64(-52.0)),
        rx_stats: Some(WifiClientStats {
            bytes: Some("1000000".into()),
            throughput_mbps_last_15s_avg: Some(ProtoF64(12.5)),
            ..WifiClientStats::default()
        }),
        tx_stats: Some(WifiClientStats {
            bytes: Some("500000".into()),
            throughput_mbps_last_15s_avg: Some(ProtoF64(2.5)),
            ..WifiClientStats::default()
        }),
        ..WifiClient::default()
    };
    let config = WifiConfig {
        mesh_configs: BTreeMap::from([
            (
                "Router-0002".into(),
                MeshNode {
                    display_name: Some("Living room".into()),
                    hardware_version: Some("rev4".into()),
                    ..MeshNode::default()
                },
            ),
            (
                "Router-0003".into(),
                MeshNode {
                    display_name: Some("Garage".into()),
                    ..MeshNode::default()
                },
            ),
        ]),
        ..WifiConfig::default()
    };
    let mut tracker = ClientRateTracker::default();
    let network_snapshot = NetworkSnapshot::from_roster(
        &[router, mesh, unpaired_mesh, client],
        Some(&config),
        vec![RadioReading {
            band: "RF_5GHZ".into(),
            temp_c: 48.0,
            duty_cycle: 100.0,
        }],
        &mut tracker,
        now_ms,
        true,
    );
    shell.network.set_snapshot(network_snapshot.clone());
    // SPEC-SELF-003, SPEC-SELF-004: cached preference marks the matching client
    // without issuing another network request.
    shell.network.set_self_client_id(Some(7));
    assert!(
        labels(shell.network.widget())
            .iter()
            .any(|label| label == "This device")
    );
    shell.network.set_self_client_id(None);
    assert!(
        !labels(shell.network.widget())
            .iter()
            .any(|label| label == "This device")
    );
    // SPEC-PAUSE-003, SPEC-PAUSE-004: a pause is offered only with an account,
    // an identified viewer, and a row that is not the viewer's own; offering it
    // sends nothing until confirmed.
    let (pause_actions, pause_rx) = std::sync::mpsc::channel();
    shell.network.set_cloud_action_sender(Some(pause_actions));
    let has_pause = |shell: &dishy_gtk::ui::ShellWindow| {
        let mut widgets = Vec::new();
        collect_widgets(shell.network.widget(), &mut widgets);
        widgets.iter().any(|widget| {
            widget
                .downcast_ref::<gtk::Button>()
                .and_then(|button| button.label())
                .is_some_and(|label| label == "Pause" || label == "Unpause")
        })
    };
    shell.network.set_self_client_id(Some(99));
    assert!(!has_pause(&shell), "no account, no pause");
    shell.network.set_cloud_connected(true);
    assert!(has_pause(&shell), "another device can be paused");
    shell.network.set_self_client_id(Some(7));
    assert!(
        !has_pause(&shell),
        "the viewer's own row never offers a pause"
    );
    shell.network.set_self_client_id(None);
    assert!(
        !has_pause(&shell),
        "an unidentified viewer is not offered pause"
    );
    assert!(
        pause_rx.try_iter().all(|action| !matches!(
            action,
            dishy_gtk::network_view::NetworkCloudAction::Pause { .. }
        )),
        "nothing is sent without confirmation"
    );
    shell.network.set_cloud_connected(false);
    assert!(matches!(
        network_requested.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    // SPEC-NETDETAIL-005: a device row opens its detail, which asks the
    // recorder only for that device's stored throughput.
    let (detail_commands, detail_rx) = std::sync::mpsc::channel();
    shell.network.set_command_sender(Some(detail_commands));
    let _ = detail_rx.try_iter().count();
    shell.network.open_device_detail(0);
    let opened = shell.network.open_device_detail_key().expect("detail open");
    let requests: Vec<_> = detail_rx.try_iter().collect();
    assert!(
        requests.iter().all(|command| matches!(
            command,
            dishy_gtk::recorder::RecorderCommand::GetDeviceThroughput(key) if *key == opened
        )),
        "{requests:?}"
    );
    shell.network.set_device_throughput(&opened, Vec::new());
    shell.network.set_command_sender(None);
    // SPEC-CLOUDCFG-005, SPEC-SELF-001, SPEC-SELF-004: Settings consumes the
    // cached network snapshot and adds no router read.
    let (no_poll_commands, no_poll_rx) = std::sync::mpsc::channel();
    shell.settings.set_command_sender(Some(no_poll_commands));
    shell.settings.set_network_snapshot(Some(&network_snapshot));
    assert!(matches!(
        no_poll_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    shell.settings.set_command_sender(None);
    assert_eq!(shell.network.device_count(), 1);
    assert_eq!(shell.network.node_count(), 4);
    assert!(!shell.network.is_unreachable());

    // SPEC-NETCLOUD-001, SPEC-NETCLOUD-002, SPEC-NETCLOUD-004, and
    // SPEC-NETCLOUD-005: demand and manual roster action, visible state,
    // source labeling, and no account-sourced live rates.
    let (cloud_actions, cloud_action_rx) = std::sync::mpsc::channel();
    shell.network.set_cloud_action_sender(Some(cloud_actions));
    shell.network.set_active(true);
    assert!(matches!(
        cloud_action_rx.try_recv(),
        Ok(NetworkCloudAction::Demand(true))
    ));
    shell.network.set_snapshot(NetworkSnapshot {
        reachable: false,
        devices: Vec::new(),
        nodes: Vec::new(),
        config: None,
        radios: Vec::new(),
    });
    assert!(shell.network.is_unreachable());
    find_button(shell.network.widget(), |button| {
        button.widget_name() == "network-cloud-roster"
    })
    .emit_clicked();
    assert!(matches!(
        cloud_action_rx.try_recv(),
        Ok(NetworkCloudAction::RequestRoster)
    ));
    assert!(
        labels(shell.network.widget())
            .iter()
            .any(|label| label == "Connecting…")
    );
    shell
        .network
        .set_cloud_roster_state(false, Some("Connect your Starlink account first.".into()));
    assert!(
        labels(shell.network.widget())
            .iter()
            .any(|label| label == "Connect your Starlink account first.")
    );
    let mut cloud_snapshot = network_snapshot.clone();
    cloud_snapshot.radios.clear();
    shell.network.set_cloud_snapshot(cloud_snapshot);
    let cloud_labels = labels(shell.network.widget());
    assert!(
        cloud_labels
            .iter()
            .any(|label| label.contains("These devices come from your Starlink account"))
    );
    assert!(
        cloud_labels
            .iter()
            .all(|label| !label.starts_with("↓ ") && !label.starts_with("↑ ")),
        "account-sourced roster must not present live throughput"
    );
    assert!(!shell.network.is_unreachable());
    shell.network.set_active(false);
    assert!(matches!(
        cloud_action_rx.try_recv(),
        Ok(NetworkCloudAction::Demand(false))
    ));
    shell.network.set_snapshot(NetworkSnapshot {
        reachable: false,
        devices: Vec::new(),
        nodes: Vec::new(),
        config: None,
        radios: Vec::new(),
    });

    // SPEC-METER-ALERT-003: dynamic monthly-limit alerts name the device in
    // both active and durable history rows.
    let alert_translator = Translator::new(Locale::En);
    shell.alerts.set_snapshot(
        &AlertSnapshot {
            active: vec![AlertState {
                source: AlertSource::System,
                key: "monthlyDataLimit:42".into(),
                severity: Severity::Warning,
                active: true,
                label: Some("Phone".into()),
            }],
            status_list: Vec::new(),
            history: vec![AlertEpisode {
                source: AlertSource::System,
                key: "monthlyDataLimit:43".into(),
                severity: Severity::Warning,
                start_ms: now_ms - 60_000,
                end_ms: Some(now_ms),
                label: Some("Office laptop".into()),
            }],
            transitions: Vec::new(),
            dish_reachable: Some(true),
            router_reachable: Some(true),
        },
        &alert_translator,
        now_ms,
    );
    let rendered_alerts = labels(shell.alerts.widget().upcast_ref());
    assert!(
        rendered_alerts
            .iter()
            .any(|label| label == "Phone reached its monthly data limit")
    );
    assert!(
        rendered_alerts
            .iter()
            .any(|label| label == "Office laptop reached its monthly data limit")
    );

    // SPEC-CTRL-001, SPEC-CTRL-003: the settings page exposes the three tabs
    // and the Starlink maintenance actions; stow follows the actuator flag.
    assert_eq!(shell.settings.dish_control_count(), 4);
    assert_eq!(shell.settings.router_control_count(), 2);
    let flat = DishStatus {
        device_info: Some(DishDeviceInfo {
            hardware_version: Some("rev3".into()),
            software_version: Some("2026.01".into()),
            ..DishDeviceInfo::default()
        }),
        has_actuators: Some("HAS_ACTUATORS_NO".into()),
        ..DishStatus::default()
    };
    shell.settings.set_status(Some(flat.clone()));
    assert!(!shell.settings.stow_visible());
    let motorized = DishStatus {
        has_actuators: Some("HAS_ACTUATORS_YES".into()),
        ..flat.clone()
    };
    shell.settings.set_status(Some(motorized.clone()));
    assert!(shell.settings.stow_visible());

    // SPEC-CTRL-002: no sender means no command; the GUI has no client of its
    // own to fall back to.
    shell
        .settings
        .dispatch_control(ControlAction::ClearObstructionMap);
    assert_eq!(shell.settings.last_action(), None);

    // SPEC-CLOUDCFG-006: account telemetry reverses the bypass action; without
    // a state reply, the UI still offers the recovery action (turn off).
    let (cloud_settings, cloud_settings_rx) = std::sync::mpsc::channel();
    shell.settings.set_action_sender(Some(cloud_settings));
    // SPEC-WIFIPW-003: the Wi-Fi password form waits for a signed-in account,
    // and a mismatch is refused without sending anything.
    let wifi_new = find_widget(shell.settings.widget(), "router-wifi-password-new")
        .downcast::<gtk::PasswordEntry>()
        .expect("new password field");
    let wifi_confirm = find_widget(shell.settings.widget(), "router-wifi-password-confirm")
        .downcast::<gtk::PasswordEntry>()
        .expect("confirm field");
    let wifi_save = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-wifi-password-save"
    });
    shell.settings.set_account_connected(false);
    assert!(!wifi_save.is_sensitive() && !wifi_new.is_sensitive());
    shell.settings.set_account_connected(true);
    assert!(wifi_save.is_sensitive() && wifi_new.is_sensitive() && wifi_confirm.is_sensitive());
    wifi_new.set_text("correct horse");
    wifi_confirm.set_text("correct hrose");
    wifi_save.emit_clicked();
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(
        labels(shell.settings.widget())
            .iter()
            .any(|label| label == "The passwords do not match")
    );
    wifi_new.set_text("");
    wifi_confirm.set_text("");
    let mut mesh_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut mesh_widgets);
    let mesh_selector = mesh_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-mesh-select")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("mesh selector exists");
    let mesh_name = mesh_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-mesh-name")
        .and_then(|widget| widget.clone().downcast::<gtk::Entry>().ok())
        .expect("mesh name field exists");
    let mesh_save = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-mesh-save"
    });
    assert!(!mesh_save.is_sensitive());
    mesh_selector.set_selected(1);
    assert_eq!(mesh_name.text(), "Living room");
    assert!(
        !mesh_save.is_sensitive(),
        "unchanged names must not dispatch"
    );
    mesh_name.set_text("   ");
    mesh_save.emit_clicked();
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    mesh_name.set_text("  Studio  ");
    assert!(mesh_save.is_sensitive());
    mesh_save.emit_clicked();
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateRouterConfig(
            dishy_cloud::router_config::RouterConfigUpdate::MeshName {
                device_id,
                display_name
            }
        )) if device_id == "Router-0002" && display_name == "Studio"
    ));
    assert!(!mesh_save.is_sensitive(), "a write is serialized");
    shell.settings.set_cloud_write_result(Ok(()));
    assert_eq!(mesh_name.text(), "Studio");
    assert!(shell.settings.last_toast().is_some());
    mesh_selector.set_selected(2);
    assert_eq!(mesh_name.text(), "Garage");
    mesh_name.set_text("Office");
    mesh_save.emit_clicked();
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateRouterConfig(
            dishy_cloud::router_config::RouterConfigUpdate::MeshName {
                device_id,
                display_name
            }
        )) if device_id == "Router-0003" && display_name == "Office"
    ));
    shell
        .settings
        .set_cloud_write_result(Err("gateway unavailable".into()));
    assert_eq!(mesh_name.text(), "Office", "failed writes retain the draft");
    assert!(
        shell
            .settings
            .last_toast()
            .as_deref()
            .is_some_and(|message| message.contains("gateway unavailable"))
    );
    // SPEC-DISHCFG-004, SPEC-DISHCFG-005: controls first reflect the recorder's
    // LAN config read; subsequent gestures emit typed partial changes through
    // the host-owned cloud action channel, never through a GUI-owned client.
    shell.settings.set_dish_config(&serde_json::json!({"snowMeltMode":"AUTO","powerSaveMode":false,"swupdateRebootHour":3,"swupdateThreeDayDeferralEnabled":false}));
    let mut dish_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut dish_widgets);
    let snow_melt = dish_widgets
        .iter()
        .find(|widget| widget.widget_name() == "dish-snow-melt")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("snow melt selector exists");
    snow_melt.set_selected(1);
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateDishConfig(
            dishy_cloud::dish_config::DishConfigUpdate {
                snow_melt_mode: Some(dishy_cloud::dish_config::SnowMeltMode::AlwaysOn),
                ..
            }
        ))
    ));
    shell.settings.set_dish_write_result(Ok(()));
    assert_eq!(shell.settings.last_toast().as_deref(), Some("Saved"));

    shell.settings.set_bypass_state(None);
    let bypass_button = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-bypass-toggle"
    });
    assert_eq!(bypass_button.label().as_deref(), Some("Turn off"));
    shell.settings.set_bypass_state(Some(false));
    assert_eq!(bypass_button.label().as_deref(), Some("Turn on"));
    shell.settings.set_bypass_state(Some(true));
    assert_eq!(bypass_button.label().as_deref(), Some("Turn off"));

    let (settings_commands, control_requests) = std::sync::mpsc::channel();
    shell.settings.set_command_sender(Some(settings_commands));
    // SPEC-DIAG-001, SPEC-DIAG-002, SPEC-DIAG-004: the explicit click sends
    // exactly one recorder request, serializes clicks, and reports both outcomes.
    let diagnostics_button = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "diagnostics-copy"
    });
    assert!(matches!(
        control_requests.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    diagnostics_button.emit_clicked();
    assert!(matches!(
        control_requests.try_recv(),
        Ok(RecorderCommand::GetDiagnostics)
    ));
    diagnostics_button.emit_clicked();
    assert!(matches!(
        control_requests.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert!(!diagnostics_button.is_sensitive());
    shell
        .settings
        .set_diagnostics_result(Err("dish diagnostics unavailable".into()));
    assert_eq!(shell.settings.last_toast().as_deref(), Some("Copy failed"));
    assert!(diagnostics_button.is_sensitive());
    shell.settings.set_diagnostics_result(Ok((
        serde_json::json!({"hardwareVersion":"rev3"}),
        serde_json::json!({"obstructionStats":{}}),
    )));
    assert_eq!(shell.settings.last_toast().as_deref(), Some("Copied ✓"));

    shell
        .settings
        .dispatch_control(ControlAction::ClearObstructionMap);
    assert_eq!(
        shell.settings.last_action(),
        Some(ControlAction::ClearObstructionMap)
    );
    assert!(matches!(
        control_requests.try_recv(),
        Ok(RecorderCommand::Control(ControlAction::ClearObstructionMap))
    ));
    shell.settings.set_control_result(ControlReport {
        action: ControlAction::ClearObstructionMap,
        result: Ok(()),
    });

    // SPEC-CLOUDCFG-001, SPEC-CLOUDCFG-003: DNS intent leaves the GTK page as
    // typed data only; the host cloud worker owns credential and gateway write.
    let mut settings_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut settings_widgets);
    let primary_dns = settings_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-dns-0")
        .and_then(|widget| widget.clone().downcast::<gtk::Entry>().ok())
        .expect("primary DNS entry exists");
    primary_dns.set_text("1.1.1.1");
    find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-dns-save"
    })
    .emit_clicked();
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateRouterConfig(
            dishy_cloud::router_config::RouterConfigUpdate::CustomDns { nameservers }
        )) if nameservers == ["1.1.1.1"]
    ));
    shell.settings.set_cloud_write_result(Ok(()));
    // SPEC-CLOUDCFG-008: when a bypassed router is unreachable on the LAN, its
    // confirmed reset uses the authenticated gateway instead of being hidden.
    shell.settings.set_router_reachable(Some(false));
    shell.settings.set_account_connected(false);
    let reset_button = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-factory-reset"
    });
    assert!(!reset_button.is_sensitive());
    shell.settings.set_account_connected(true);
    assert!(reset_button.is_sensitive());
    reset_button.emit_clicked();
    while gtk::glib::MainContext::default().iteration(false) {}
    let reset_confirmation = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<libadwaita::MessageDialog>().ok())
        .find(|dialog| dialog.is_visible())
        .expect("factory reset requires confirmation");
    assert_eq!(
        reset_confirmation.default_response().as_deref(),
        Some("cancel")
    );
    reset_confirmation.response("confirm");
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateRouterConfig(
            dishy_cloud::router_config::RouterConfigUpdate::FactoryReset
        ))
    ));
    shell.settings.set_cloud_write_result(Ok(()));
    assert_eq!(shell.settings.last_toast().as_deref(), Some("Saved"));
    // SPEC-CLOUDCFG-007: the subnet form validates the passphrase locally and
    // sends only the chosen official preset plus the user-entered password.
    let subnet_selector = settings_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-subnet-select")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("subnet selector exists");
    // libadwaita's AdwComboRow cannot represent "no selection"; index 0 is the
    // placeholder row, presets start at index 1. Saving with the placeholder
    // row shows a local error and dispatches nothing.
    assert_eq!(subnet_selector.selected(), 0);
    let subnet_password = settings_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-subnet-password")
        .and_then(|widget| widget.clone().downcast::<gtk::PasswordEntry>().ok())
        .expect("subnet password field exists");
    subnet_password.set_text("correct horse");
    let subnet_save = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "router-subnet-save"
    });
    subnet_save.emit_clicked();
    assert!(cloud_settings_rx.try_recv().is_err());
    // Index 5 is the fifth preset (10.0.0.1/16) after the placeholder row.
    subnet_selector.set_selected(5);
    subnet_password.set_text("short");
    subnet_save.emit_clicked();
    assert!(cloud_settings_rx.try_recv().is_err());
    subnet_password.set_text("correct horse");
    subnet_save.emit_clicked();
    while gtk::glib::MainContext::default().iteration(false) {}
    let confirmation = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<libadwaita::MessageDialog>().ok())
        .find(|dialog| dialog.is_visible())
        .expect("subnet change requires an explicit confirmation");
    assert_eq!(confirmation.default_response().as_deref(), Some("cancel"));
    confirmation.response("confirm");
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(matches!(
        cloud_settings_rx.try_recv(),
        Ok(SettingsAction::UpdateRouterConfig(dishy_cloud::router_config::RouterConfigUpdate::Subnet { subnet, password }))
            if subnet == "10.0.0.1/16" && password == "correct horse"
    ));
    shell.settings.set_cloud_write_result(Ok(()));
    // SPEC-CTRL-002: one command at a time; a second click is ignored while the
    // first is still in flight.
    shell
        .settings
        .dispatch_control(ControlAction::ClearObstructionMap);
    assert!(matches!(
        control_requests.try_recv(),
        Ok(RecorderCommand::Control(ControlAction::ClearObstructionMap))
    ));
    shell.settings.dispatch_control(ControlAction::RebootDish);
    assert!(control_requests.try_recv().is_err());
    shell.settings.set_control_result(ControlReport {
        action: ControlAction::ClearObstructionMap,
        result: Ok(()),
    });
    assert_eq!(
        shell.settings.last_toast().as_deref(),
        Some("Obstruction map cleared — the survey restarts now.")
    );

    // SPEC-CTRL-004: router controls wait for the existing status cycle.
    shell.settings.set_router_reachable(Some(false));
    shell.settings.dispatch_control(ControlAction::RebootRouter);
    assert!(control_requests.try_recv().is_err());
    shell.settings.set_router_reachable(Some(true));
    shell.settings.dispatch_control(ControlAction::RebootRouter);
    assert!(matches!(
        control_requests.try_recv(),
        Ok(RecorderCommand::Control(ControlAction::RebootRouter))
    ));

    // SPEC-CTRL-006: a failed write is reported as a failure, never as sent.
    shell.settings.set_control_result(ControlReport {
        action: ControlAction::RebootRouter,
        result: Err("timed out".into()),
    });
    assert_eq!(
        shell.settings.last_toast().as_deref(),
        Some("Failed: timed out")
    );

    // SPEC-CTRL-003: stow needs actuators as well as a present dish.
    shell.settings.set_status(Some(flat));
    shell
        .settings
        .dispatch_control(ControlAction::StowDish { unstow: false });
    assert!(control_requests.try_recv().is_err());
    shell.settings.set_status(Some(motorized));
    shell
        .settings
        .dispatch_control(ControlAction::StowDish { unstow: false });
    assert!(matches!(
        control_requests.try_recv(),
        Ok(RecorderCommand::Control(ControlAction::StowDish {
            unstow: false
        }))
    ));
    shell.settings.set_control_result(ControlReport {
        action: ControlAction::StowDish { unstow: false },
        result: Ok(()),
    });

    // SPEC-CTRL-005, SPEC-TIME-001, SPEC-TIME-002, SPEC-TIME-005: settings
    // seeding is silent; local choices emit typed preference intent only.
    let (actions, app_actions) = std::sync::mpsc::channel();
    shell.settings.set_action_sender(Some(actions));
    shell.settings.set_app_state(AppSettingsState {
        locale: Locale::En,
        notifications: false,
        autostart: false,
        tray_throughput: true,
        see_through_glass: false,
        show_tray: true,
        router_address: None,
        time_format: TimeFormat::Auto,
        toolbar_style: ToolbarStyle::Dock,
        self_client_id: None,
    });
    assert_eq!(app_actions.try_recv().ok(), None);
    let mut app_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut app_widgets);
    // SPEC-SHEXT-002: the Shell extension row words the Shell's state and
    // offers the matching action, which asks the host to install.
    let extension_button = find_button(shell.settings.widget(), |button| {
        button.widget_name() == "app-shell-extension-install"
    });
    shell
        .settings
        .set_shell_extension_status(dishy_gtk::shell_extension::ExtensionStatus::NotInstalled);
    assert_eq!(extension_button.label().as_deref(), Some("Install"));
    assert!(extension_button.is_visible());
    extension_button.emit_clicked();
    assert!(matches!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::InstallShellExtension)
    ));
    shell
        .settings
        .set_shell_extension_status(dishy_gtk::shell_extension::ExtensionStatus::NeedsLogin);
    assert!(
        labels(shell.settings.widget())
            .iter()
            .any(|label| label.contains("log out"))
    );
    shell
        .settings
        .set_shell_extension_status(dishy_gtk::shell_extension::ExtensionStatus::Unsupported);
    assert!(!extension_button.is_visible());

    // SPEC-PILL-004: the tray switch reflects the stored choice and sends
    // the change; SPEC-GLASS-004: so does see-through glass.
    for (name, seeded) in [("app-show-tray", true), ("app-see-through-glass", false)] {
        let row = app_widgets
            .iter()
            .find(|widget| widget.widget_name() == name)
            .and_then(|widget| widget.clone().downcast::<libadwaita::SwitchRow>().ok())
            .expect("switch exists");
        assert_eq!(row.is_active(), seeded, "{name}");
        row.set_active(!seeded);
        match (name, app_actions.try_recv().ok()) {
            ("app-show-tray", Some(SettingsAction::SetShowTray(false))) => {}
            ("app-see-through-glass", Some(SettingsAction::SetSeeThroughGlass(true))) => {}
            (name, other) => panic!("{name} sent {other:?}"),
        }
        row.set_active(seeded);
        let _ = app_actions.try_recv();
    }
    let time_format_row = app_widgets
        .iter()
        .find(|widget| widget.widget_name() == "app-time-format")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("time-format selector exists");
    let time_format_labels: Vec<_> = (0..time_format_row.model().unwrap().n_items())
        .filter_map(|index| {
            time_format_row
                .model()
                .and_then(|model| model.item(index))
                .and_then(|item| item.downcast::<gtk::StringObject>().ok())
                .map(|item| item.string().to_string())
        })
        .collect();
    assert_eq!(time_format_labels, ["Automatic", "12-hour", "24-hour"]);
    let toolbar_style_row = app_widgets
        .iter()
        .find(|widget| widget.widget_name() == "app-toolbar-style")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("toolbar-style selector exists");
    let toolbar_style_labels: Vec<_> = (0..toolbar_style_row.model().unwrap().n_items())
        .filter_map(|index| {
            toolbar_style_row
                .model()
                .and_then(|model| model.item(index))
                .and_then(|item| item.downcast::<gtk::StringObject>().ok())
                .map(|item| item.string().to_string())
        })
        .collect();
    assert_eq!(toolbar_style_labels, ["Dock", "Left rail"]);
    let self_device_row = app_widgets
        .iter()
        .find(|widget| widget.widget_name() == "app-self-device")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("self-device selector exists");
    // SPEC-SELF-001, SPEC-SELF-003: cached roster choices are offered, and a
    // saved-but-missing ID stays visible until the user explicitly clears it.
    assert_eq!(self_device_row.model().unwrap().n_items(), 2);
    assert_eq!(
        self_device_row
            .model()
            .unwrap()
            .item(1)
            .and_then(|item| item.downcast::<gtk::StringObject>().ok())
            .map(|item| item.string().to_string())
            .as_deref(),
        Some("client-mac · 192.168.1.20")
    );
    shell.settings.select_self_device(1);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetSelfClientId(Some(7)))
    );
    shell.settings.set_app_state(AppSettingsState {
        locale: Locale::En,
        notifications: false,
        autostart: false,
        tray_throughput: true,
        see_through_glass: false,
        show_tray: true,
        router_address: None,
        time_format: TimeFormat::Auto,
        toolbar_style: ToolbarStyle::Dock,
        self_client_id: Some(99),
    });
    assert_eq!(self_device_row.selected(), 1);
    assert_eq!(self_device_row.model().unwrap().n_items(), 3);
    shell.settings.set_network_snapshot(None);
    assert_eq!(self_device_row.selected(), 1);
    assert_eq!(self_device_row.model().unwrap().n_items(), 2);
    shell.settings.set_network_snapshot(Some(&network_snapshot));
    assert_eq!(self_device_row.selected(), 1);
    assert_eq!(self_device_row.model().unwrap().n_items(), 3);
    assert_eq!(
        self_device_row
            .model()
            .unwrap()
            .item(1)
            .and_then(|item| item.downcast::<gtk::StringObject>().ok())
            .map(|item| item.string().to_string())
            .as_deref(),
        Some("Not connected")
    );
    assert_eq!(app_actions.try_recv().ok(), None);
    shell.settings.select_self_device(0);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetSelfClientId(None))
    );
    shell.settings.select_notifications(true);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetNotifications(true))
    );
    shell.settings.select_autostart(true);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetAutostart(true))
    );
    shell.settings.select_tray_throughput(false);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetTrayThroughput(false))
    );
    shell.settings.select_time_format(TimeFormat::Hour24);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetTimeFormat(TimeFormat::Hour24))
    );
    shell.settings.select_toolbar_style(ToolbarStyle::Rail);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetToolbarStyle(ToolbarStyle::Rail))
    );
    shell
        .settings
        .set_dish_config(&serde_json::json!({"swupdateRebootHour":15}));
    shell.settings.set_app_state(AppSettingsState {
        locale: Locale::En,
        notifications: false,
        autostart: false,
        tray_throughput: true,
        see_through_glass: false,
        show_tray: true,
        router_address: None,
        time_format: TimeFormat::Hour12,
        toolbar_style: ToolbarStyle::Rail,
        self_client_id: None,
    });
    let mut time_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut time_widgets);
    let update_window = time_widgets
        .iter()
        .find(|widget| widget.widget_name() == "dish-update-window")
        .and_then(|widget| widget.clone().downcast::<libadwaita::ComboRow>().ok())
        .expect("update-window selector exists");
    assert_eq!(update_window.selected(), 2);
    let update_window_label = update_window
        .model()
        .and_then(|model| model.item(2))
        .and_then(|item| item.downcast::<gtk::StringObject>().ok())
        .map(|item| item.string().to_string());
    assert_eq!(update_window_label.as_deref(), Some("3:00 PM"));
    assert_eq!(app_actions.try_recv().ok(), None);
    shell.settings.select_locale(Locale::Fr);
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetLocale(Locale::Fr))
    );

    // SPEC-CTRL-007: the router endpoint can be changed while it is still
    // contacting the default address; saving does not wait for reachability.
    shell.settings.set_router_reachable(None);
    let mut settings_widgets = Vec::new();
    collect_widgets(shell.settings.widget(), &mut settings_widgets);
    let address_entry = settings_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-address-entry")
        .and_then(|widget| widget.clone().downcast::<gtk::Entry>().ok())
        .expect("router address entry is available before connection");
    let save_address = settings_widgets
        .iter()
        .find(|widget| widget.widget_name() == "router-address-save")
        .and_then(|widget| widget.clone().downcast::<gtk::Button>().ok())
        .expect("router address save button");
    address_entry.set_text("10.0.0.1");
    save_address.emit_clicked();
    assert_eq!(
        app_actions.try_recv().ok(),
        Some(SettingsAction::SetRouterAddress(Some("10.0.0.1".into())))
    );

    // SPEC-ALIGN-001, SPEC-ALIGN-005, SPEC-ALIGN-006, SPEC-ALIGN-007:
    // the alignment page starts waiting, then shows the verdict, instruments
    // (rotation and tilt dials with Cairo drawings), facts, explainer, and
    // satellite link from a cached status without sending any recorder command.
    shell.stack.set_visible_child_name("alignment");
    let alignment_status = DishStatus {
        alignment_stats: Some(dishy_core::telemetry::AlignmentStats {
            attitude_estimation_state: Some("FILTER_CONVERGED".into()),
            desired_boresight_azimuth_deg: ProtoF64(180.0),
            desired_boresight_elevation_deg: ProtoF64(70.0),
            attitude_uncertainty_deg: ProtoF64(0.5),
            ..Default::default()
        }),
        boresight_azimuth_deg: ProtoF64(180.0),
        boresight_elevation_deg: ProtoF64(70.0),
        gps_stats: Some(dishy_core::telemetry::GpsStats {
            gps_valid: true,
            gps_sats: 12,
            ..Default::default()
        }),
        ..Default::default()
    };
    shell.alignment.set_status(Some(alignment_status));
    // Check the page contains alignment-specific text. Like Electron's card it
    // has no "Alignment" heading; its first fact is the current rotation.
    let alignment_widgets = labels(shell.alignment.widget().upcast_ref());
    assert!(
        alignment_widgets
            .iter()
            .any(|label| label.contains("Current rotation"))
    );
    assert!(
        alignment_widgets
            .iter()
            .any(|label| label.contains("180.0°"))
    );
    assert!(
        alignment_widgets
            .iter()
            .any(|label| label.contains("70.0°"))
    );
    assert!(
        alignment_widgets
            .iter()
            .any(|label| label.contains("12 satellites"))
    );

    // SPEC-CLOUD-006: the account surface has explicit signed-out, loading,
    // ready, and retryable error states; actions go to the host boundary.
    // SPEC-CLOUD-007: opening the account page requests cloud data on demand.
    let (cloud_actions, cloud_requests) = std::sync::mpsc::channel();
    shell.account.set_action_sender(cloud_actions);
    shell.stack.set_visible_child_name("account");
    assert_eq!(cloud_requests.try_recv().ok(), Some(CloudAction::Refresh));
    let account_stack = {
        let mut widgets = Vec::new();
        collect_widgets(shell.account.widget(), &mut widgets);
        widgets
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Stack>().ok())
            .next()
            .expect("account state stack")
    };
    shell.account.set_state(CloudState::SignedOut);
    assert_eq!(
        account_stack.visible_child_name().as_deref(),
        Some("signed-out")
    );
    shell.account.set_state(CloudState::Authenticated);
    assert_eq!(
        account_stack.visible_child_name().as_deref(),
        Some("loading")
    );
    find_button(shell.account.widget(), |button| {
        button.widget_name() == "account-sign-in"
    })
    .emit_clicked();
    assert_eq!(cloud_requests.try_recv().ok(), Some(CloudAction::SignIn));
    shell.account.set_state(CloudState::Loading);
    assert_eq!(
        account_stack.visible_child_name().as_deref(),
        Some("loading")
    );
    let cloud_now = chrono::Utc::now().to_rfc3339();
    let cloud_timestamp = chrono::Utc::now().timestamp_millis();
    shell.account.set_state(CloudState::Ready {
        account: AccountData {
            identity: Some(serde_json::json!({"name": "Test User"})),
            service_line: Some(serde_json::json!({"content": {
                "serviceLineNumber": "SL-test",
                "subscription": {"productDescription": "Residential"},
                "userTerminals": [{
                    "userTerminalId": "dish-123",
                    "serialNumber": "KIT-1",
                    "dishSerialNumber": "DISH-1",
                    "lastConnected": cloud_now,
                    "routers": [{"routerId": "00aabb"}]
                }]
            }})),
            device_telemetry: serde_json::json!({
                "utdish-123": {"kind": "dish", "timestampMs": cloud_timestamp, "softwareVersion": "dish-v1"},
                "Router-00aabb": {"kind": "router", "timestampMs": cloud_timestamp, "hardwareVersion": "v3", "softwareVersion": "router-v1", "isRepeater": false, "hops": 0}
            }),
        },
        usage: serde_json::json!({"content": {"billingCyclesAnnotated": [{"totalAmountGB": 12.5}]}}),
    });
    assert_eq!(account_stack.visible_child_name().as_deref(), Some("ready"));
    assert!(
        labels(shell.account.widget())
            .iter()
            .any(|label| label.contains("12.50 GB"))
    );
    let account_labels = labels(shell.account.widget());
    assert!(
        account_labels
            .iter()
            .any(|label| label.contains("STARLINK 123"))
    );
    assert!(account_labels.iter().any(|label| label.contains("DISH-1")));
    assert!(
        account_labels
            .iter()
            .any(|label| label.contains("Main Router AABB"))
    );
    shell
        .account
        .set_state(CloudState::Error("temporary network failure".into()));
    assert_eq!(account_stack.visible_child_name().as_deref(), Some("error"));
    find_button(shell.account.widget(), |button| {
        button.widget_name() == "account-retry"
    })
    .emit_clicked();
    assert_eq!(cloud_requests.try_recv().ok(), Some(CloudAction::Refresh));
    shell.account.set_state(CloudState::Ready {
        account: AccountData {
            identity: Some(serde_json::json!({"name": "Test User"})),
            service_line: None,
            device_telemetry: serde_json::Value::Null,
        },
        usage: serde_json::Value::Null,
    });
    find_button(shell.account.widget(), |button| {
        button.widget_name() == "account-disconnect"
    })
    .emit_clicked();
    assert_eq!(
        cloud_requests.try_recv().ok(),
        Some(CloudAction::Disconnect)
    );

    shell.window.close();
    while gtk::glib::MainContext::default().iteration(false) {}
    assert!(!shell.window.is_visible());
}
