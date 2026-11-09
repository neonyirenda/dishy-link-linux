//! The device and node detail windows (spec 0175). Widgets only; the facts
//! and chart series come from `network_detail_model`.

use std::cell::Cell;
use std::rc::Rc;

use dishy_core::client_window::ClientSample;
use dishy_core::network::{NetworkDevice, NetworkNode};
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::chart::ChartCanvas;
use crate::i18n::Translator;
use crate::network_detail_model::{
    DEVICE_WINDOWS, device_facts, empty_note, node_facts, throughput_series,
};

fn facts_group(title: Option<&str>, facts: &[(String, String)]) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    if let Some(title) = title {
        group.set_title(title);
    }
    for (label, value) in facts {
        let row = adw::ActionRow::builder()
            .title(gtk::glib::markup_escape_text(label))
            .build();
        let value = gtk::Label::builder()
            .label(value)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .tooltip_text(value)
            .build();
        value.add_css_class("monospace");
        row.add_suffix(&value);
        group.add(&row);
    }
    group
}

fn detail_window(parent: Option<&gtk::Window>, title: &str, body: &gtk::Box) -> adw::Window {
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(18);
    body.set_margin_end(18);
    let window = adw::Window::builder()
        .modal(true)
        .default_width(560)
        .default_height(680)
        .title(title)
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(body)
            .build(),
    ));
    window.set_content(Some(&toolbar));
    window
}

/// An open device detail: the chart refreshes as the recorder answers.
#[derive(Clone)]
pub struct DeviceDetail {
    window: adw::Window,
    key: String,
    chart: ChartCanvas,
    empty: gtk::Label,
    refusal_caption: gtk::Box,
    window_minutes: Rc<Cell<u32>>,
    samples: Rc<std::cell::RefCell<Vec<ClientSample>>>,
    translator: Translator,
}

impl DeviceDetail {
    /// `actions` receives the header box and the caption box under it, so the
    /// page can add rename and pause behind its own gate, and the reason a
    /// pause is not offered (SPEC-PAUSE-007).
    pub fn open(
        parent: Option<&gtk::Window>,
        device: &NetworkDevice,
        upstream_name: Option<&str>,
        month: Option<(u64, u64)>,
        translator: &Translator,
        actions: impl FnOnce(&gtk::Box, &gtk::Box),
    ) -> Self {
        let vendor = dishy_core::oui::OuiRegistry::bundled().vendor_for_mac(&device.mac_address);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::builder()
            .label(crate::network_view::device_name(device, translator))
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .build();
        name.add_css_class("title-3");
        header.append(&name);
        let refusal_caption = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&header);
        body.append(&refusal_caption);
        actions(&header, &refusal_caption);

        let heading = gtk::Label::builder()
            .label(translator.text("metrics.throughput"))
            .xalign(0.0)
            .tooltip_text(translator.text("network.throughputTip"))
            .build();
        heading.add_css_class("heading");
        let windows = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        windows.add_css_class("linked");
        windows.set_halign(gtk::Align::Start);
        let chart = ChartCanvas::new(180, DEVICE_WINDOWS[0], translator);
        let empty = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        empty.add_css_class("dim-label");
        body.append(&heading);
        body.append(&windows);
        body.append(chart.widget());
        body.append(&empty);
        body.append(&facts_group(
            None,
            &device_facts(device, upstream_name, vendor, month, translator),
        ));

        let window = detail_window(parent, &translator.text("network.viewFullDetails"), &body);
        let detail = Self {
            window,
            key: device.usage_key.clone(),
            chart,
            empty,
            refusal_caption,
            window_minutes: Rc::new(Cell::new(DEVICE_WINDOWS[0])),
            samples: Rc::new(std::cell::RefCell::new(Vec::new())),
            translator: translator.clone(),
        };
        let mut first: Option<gtk::ToggleButton> = None;
        for minutes in DEVICE_WINDOWS {
            let label = if minutes < 60 {
                format!("{minutes}M")
            } else {
                format!("{}H", minutes / 60)
            };
            let button = gtk::ToggleButton::with_label(&label);
            match &first {
                Some(first) => button.set_group(Some(first)),
                None => {
                    button.set_active(true);
                    first = Some(button.clone());
                }
            }
            let this = detail.clone();
            button.connect_toggled(move |button| {
                if button.is_active() {
                    this.window_minutes.set(minutes);
                    this.redraw();
                }
            });
            windows.append(&button);
        }
        detail.redraw();
        detail.window.present();
        detail
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    /// The refusal caption's text, when the pause gate refused this device
    /// (SPEC-PAUSE-007).
    pub fn refusal_text(&self) -> Option<String> {
        let mut child = self.refusal_caption.first_child();
        while let Some(widget) = child {
            if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
                return Some(label.label().to_string());
            }
            child = widget.next_sibling();
        }
        None
    }

    pub fn set_samples(&self, samples: Vec<ClientSample>) {
        *self.samples.borrow_mut() = samples;
        self.redraw();
    }

    fn redraw(&self) {
        let minutes = self.window_minutes.get();
        let now = dishy_core::calendar::now_ms();
        let series = throughput_series(&self.samples.borrow(), minutes, now, &self.translator);
        let empty = series.iter().all(|series| series.points.is_empty());
        self.chart.set_data(series, minutes, now);
        self.chart.widget().set_visible(!empty);
        self.empty.set_label(&empty_note(minutes, &self.translator));
        self.empty.set_visible(empty);
    }
}

/// The node detail: its facts and the devices connected through it.
pub fn open_node_detail(
    parent: Option<&gtk::Window>,
    node: &NetworkNode,
    node_device: Option<&NetworkDevice>,
    connected: &[&NetworkDevice],
    translator: &Translator,
) -> adw::Window {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    let name = gtk::Label::builder()
        .label(crate::network_view::node_name(node, translator))
        .xalign(0.0)
        .build();
    name.add_css_class("title-3");
    body.append(&name);
    if !node.connected {
        let down = gtk::Label::builder()
            .label(translator.text("network.nodeUnreachable"))
            .xalign(0.0)
            .wrap(true)
            .build();
        down.add_css_class("warning");
        body.append(&down);
    }
    body.append(&facts_group(
        None,
        &node_facts(node, node_device, translator),
    ));
    let devices = adw::PreferencesGroup::builder()
        .title(translator.text("network.connectedDevices"))
        .build();
    if connected.is_empty() {
        devices.set_description(Some(&translator.text("network.noDevicesOnNode")));
    }
    for device in connected {
        devices.add(
            &adw::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(
                    &crate::network_view::device_name(device, translator),
                ))
                .subtitle(gtk::glib::markup_escape_text(&device.band))
                .build(),
        );
    }
    body.append(&devices);
    let window = detail_window(parent, &translator.text("network.viewFullDetails"), &body);
    window.present();
    window
}
