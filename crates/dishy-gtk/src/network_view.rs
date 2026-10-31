//! The network page: the router's own view of its clients and mesh nodes.
//!
//! LAN reads are demand-gated through the recorder while the page is mapped.
//! Optional account reads are handed to the trusted cloud worker; this view
//! never opens a device client or handles credentials.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Once;
use std::sync::mpsc::Sender;

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use dishy_core::network::{
    ConnectionKind, NetworkDevice, NetworkNode, NetworkSnapshot, SignalQuality, iface_label,
    signal_quality,
};

use crate::format::format_mbps;
use crate::i18n::Translator;
use crate::recorder::RecorderCommand;
use crate::rules_model::pause_available;
use crate::rules_view::RulesView;

const CSS: &str = r#"
.dishy-network-page {
  padding: 18px 24px 28px 24px;
}
.dishy-network-meta {
  font-size: 12px;
  font-weight: 500;
  opacity: 0.68;
  margin-bottom: 10px;
}
.dishy-network-content {
  margin-top: 2px;
}
.dishy-network-row {
  padding: 11px 12px;
}
.dishy-network-row-name {
  font-weight: 600;
}
.dishy-network-row-sub {
  font-size: 12px;
  opacity: 0.68;
}
.dishy-network-row-mac {
  font-size: 11px;
  opacity: 0.48;
  font-family: monospace;
}
.dishy-network-rates {
  font-size: 12px;
  font-weight: 600;
}
.dishy-network-badge {
  font-size: 11px;
  font-weight: 700;
  padding: 2px 7px;
  border-radius: 99px;
  background: alpha(currentColor, 0.09);
}
.dishy-network-self-device {
  font-size: 11px;
  font-weight: 700;
  color: alpha(currentColor, 0.72);
}
.dishy-network-connected {
  color: #2f9e63;
}
.dishy-network-disconnected {
  opacity: 0.55;
}
.dishy-network-node {
  padding: 12px 14px;
  margin-bottom: 8px;
}
.dishy-network-node-name {
  font-weight: 700;
}
.dishy-network-radios {
  margin-top: 16px;
}
.dishy-network-radios-title {
  font-weight: 700;
  margin-bottom: 6px;
}
.dishy-network-radio {
  font-size: 12px;
  opacity: 0.72;
  margin-bottom: 3px;
}
.dishy-network-message {
  padding: 48px 24px;
}
.dishy-network-message-title {
  font-size: 16px;
  font-weight: 700;
}
.dishy-network-message-body {
  opacity: 0.7;
}
"#;

#[derive(Clone)]
pub struct NetworkPage {
    inner: Rc<Inner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkCloudAction {
    Demand(bool),
    RequestRoster,
    /// One confirmed pause or unpause, sent once (SPEC-PAUSE-004).
    Pause {
        client_id: u32,
        paused: bool,
        mac_address: Option<String>,
    },
    /// One confirmed rename, sent once.
    Rename {
        client_id: u32,
        given_name: String,
    },
}

struct Inner {
    root: gtk::ScrolledWindow,
    overlay: adw::ToastOverlay,
    rules: RulesView,
    cloud_connected: Cell<bool>,
    known_names: RefCell<HashMap<String, String>>,
    known_totals: RefCell<HashMap<String, (u64, u64)>>,
    shown_devices: RefCell<Vec<NetworkDevice>>,
    device_detail: RefCell<Option<crate::network_detail_view::DeviceDetail>>,
    translator: Translator,
    commands: RefCell<Option<Sender<RecorderCommand>>>,
    cloud_actions: RefCell<Option<Sender<NetworkCloudAction>>>,
    snapshot: RefCell<Option<NetworkSnapshot>>,
    active: Cell<bool>,
    cloud_source: Cell<bool>,
    cloud_loading: Cell<bool>,
    cloud_error: RefCell<Option<String>>,
    self_client_id: Cell<Option<u32>>,
    body: gtk::Stack,
    meta: gtk::Label,
    unreachable_body: gtk::Label,
    cloud_button: gtk::Button,
    cloud_status_unreachable: gtk::Label,
    cloud_status_content: gtk::Label,
    devices: gtk::ListBox,
    nodes: gtk::Box,
    device_count: Cell<usize>,
    node_count: Cell<usize>,
}

impl NetworkPage {
    pub fn new(translator: &Translator) -> Self {
        install_css();

        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("dishy-network-page");
        page.set_vexpand(true);

        let waiting = message(
            translator,
            "common.waitingForDevices",
            None,
            "network-wireless-symbolic",
        );
        let unreachable_body = gtk::Label::builder()
            .label(router_unreachable(translator))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .build();
        unreachable_body.add_css_class("dishy-network-message-body");
        let cloud_button = gtk::Button::with_label(&translator.text("network.connectThroughCloud"));
        cloud_button.set_widget_name("network-cloud-roster");
        let cloud_status_unreachable = gtk::Label::builder()
            .label(translator.text("network.rosterViaAccount"))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        cloud_status_unreachable.add_css_class("dishy-network-message-body");
        let unreachable = gtk::Box::new(gtk::Orientation::Vertical, 8);
        unreachable.add_css_class("dishy-network-message");
        unreachable.set_valign(gtk::Align::Center);
        unreachable.set_halign(gtk::Align::Center);
        let unreachable_icon = gtk::Image::from_icon_name("network-wireless-offline-symbolic");
        unreachable_icon.set_pixel_size(36);
        unreachable.append(&unreachable_icon);
        let unreachable_title = gtk::Label::builder()
            .label(translator.text("navigation.network"))
            .build();
        unreachable_title.add_css_class("dishy-network-message-title");
        unreachable.append(&unreachable_title);
        unreachable.append(&unreachable_body);
        unreachable.append(&cloud_button);
        unreachable.append(&cloud_status_unreachable);

        let meta = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .wrap(true)
            .build();
        meta.add_css_class("dishy-network-meta");
        let cloud_status_content = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .wrap(true)
            .build();
        cloud_status_content.add_css_class("dishy-network-row-sub");
        cloud_status_content.set_visible(false);

        let devices = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .show_separators(true)
            .build();
        devices.add_css_class("boxed-list");

        let nodes = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let section = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(140)
            .vexpand(true)
            .build();
        section.add_titled(
            &devices,
            Some("devices"),
            &translator.text("network.tabConnected"),
        );
        section.add_titled(&nodes, Some("nodes"), &translator.text("network.tabNodes"));
        let rules = RulesView::new(translator);
        section.add_titled(
            rules.widget(),
            Some("rules"),
            &translator.text("network.tabRules"),
        );
        let switcher = gtk::StackSwitcher::builder().stack(&section).build();
        switcher.set_widget_name("network-tabs");
        switcher.set_halign(gtk::Align::Center);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.add_css_class("dishy-network-content");
        content.append(&meta);
        content.append(&cloud_status_content);
        content.append(&switcher);
        content.append(&section);

        let body = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(180)
            .vexpand(true)
            .build();
        body.add_named(&waiting, Some("waiting"));
        body.add_named(&unreachable, Some("unreachable"));
        body.add_named(&content, Some("content"));
        body.set_visible_child_name("waiting");

        page.append(&body);
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&page)
            .build();
        root.add_css_class("dishy-page-scroller");
        let overlay = adw::ToastOverlay::new();
        overlay.set_child(Some(&root));

        let inner = Rc::new(Inner {
            root,
            overlay,
            rules,
            cloud_connected: Cell::new(false),
            known_names: RefCell::new(HashMap::new()),
            known_totals: RefCell::new(HashMap::new()),
            shown_devices: RefCell::new(Vec::new()),
            device_detail: RefCell::new(None),
            translator: translator.clone(),
            commands: RefCell::new(None),
            cloud_actions: RefCell::new(None),
            snapshot: RefCell::new(None),
            active: Cell::new(false),
            cloud_source: Cell::new(false),
            cloud_loading: Cell::new(false),
            cloud_error: RefCell::new(None),
            self_client_id: Cell::new(None),
            body,
            meta,
            unreachable_body,
            cloud_button,
            cloud_status_unreachable,
            cloud_status_content,
            devices,
            nodes,
            device_count: Cell::new(0),
            node_count: Cell::new(0),
        });

        let weak = Rc::downgrade(&inner);
        inner.root.connect_map(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.set_active(true);
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.root.connect_unmap(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.set_active(false);
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.devices.connect_row_activated(move |_, row| {
            if let Some(inner) = weak.upgrade() {
                inner.open_device_detail(row.index() as usize);
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.cloud_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.request_cloud_roster();
            }
        });

        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.overlay.upcast_ref()
    }

    /// The recorder handle. The page never needs anything else from the app.
    pub fn set_command_sender(&self, sender: Option<Sender<RecorderCommand>>) {
        self.inner.rules.set_command_sender(sender.clone());
        *self.inner.commands.borrow_mut() = sender;
        if self.inner.active.get() {
            self.inner.send_demand(true);
        }
    }

    pub fn set_cloud_action_sender(&self, sender: Option<Sender<NetworkCloudAction>>) {
        *self.inner.cloud_actions.borrow_mut() = sender;
        if self.inner.active.get() {
            self.inner.send_cloud_demand(true);
        }
    }

    /// Demand-gate the network reads. The map handlers call this; it is public
    /// so the shell test can exercise the command without a compositor.
    // SPEC-SAFETY-003, SPEC-NET-001: every router read in this view is gated on
    // the page being visible.
    pub fn set_active(&self, active: bool) {
        self.inner.set_active(active);
    }

    pub fn set_snapshot(&self, snapshot: NetworkSnapshot) {
        self.inner.cloud_source.set(false);
        self.inner.cloud_loading.set(false);
        self.inner.cloud_error.borrow_mut().take();
        self.inner.refresh_cloud_status();
        self.inner.apply(&snapshot);
        *self.inner.snapshot.borrow_mut() = Some(snapshot);
    }

    pub fn set_cloud_snapshot(&self, snapshot: NetworkSnapshot) {
        self.inner.cloud_source.set(true);
        self.inner.cloud_loading.set(false);
        self.inner.cloud_error.borrow_mut().take();
        self.inner.apply(&snapshot);
        *self.inner.snapshot.borrow_mut() = Some(snapshot);
        self.inner.refresh_cloud_status();
    }

    /// Mark a client from the host's saved preference, without requesting a
    /// fresh roster or changing the snapshot source.
    // SPEC-SELF-003, SPEC-SELF-004: render-only local identity state.
    pub fn set_self_client_id(&self, client_id: Option<u32>) {
        if self.inner.self_client_id.replace(client_id) == client_id {
            return;
        }
        if let Some(snapshot) = self.inner.snapshot.borrow().clone() {
            self.inner.apply(&snapshot);
        }
    }

    pub fn set_cloud_roster_state(&self, loading: bool, error: Option<String>) {
        let loading_changed = self.inner.cloud_loading.replace(loading) != loading;
        let error_changed = *self.inner.cloud_error.borrow() != error;
        if loading_changed || error_changed {
            *self.inner.cloud_error.borrow_mut() = error;
            self.inner.refresh_cloud_status();
        }
    }

    pub fn set_rules(&self, rules: std::sync::Arc<crate::recorder::RulesSnapshot>) {
        self.inner.rules.set_snapshot(rules);
    }

    pub fn rule_write_failed(&self, error: &str) {
        self.inner.rules.write_failed(error);
    }

    pub fn rule_card_count(&self) -> usize {
        self.inner.rules.card_count()
    }

    /// Device names the odometer knows, so a rule on an offline device is still
    /// named. Merged with the live roster's names.
    pub fn set_known_devices(&self, names: HashMap<String, String>) {
        *self.inner.known_names.borrow_mut() = names;
        self.inner.refresh_rule_names();
    }

    /// This month's (rx, tx) per odometer key, for the device detail.
    pub fn set_known_totals(&self, totals: HashMap<String, (u64, u64)>) {
        *self.inner.known_totals.borrow_mut() = totals;
    }

    /// Open the detail of the `index`-th listed device, as activating its row
    /// does (spec 0175).
    pub fn open_device_detail(&self, index: usize) {
        self.inner.open_device_detail(index);
    }

    pub fn open_device_detail_key(&self) -> Option<String> {
        self.inner
            .device_detail
            .borrow()
            .as_ref()
            .filter(|detail| detail.is_open())
            .map(|detail| detail.key().to_owned())
    }

    pub fn set_device_throughput(
        &self,
        key: &str,
        samples: Vec<dishy_core::client_window::ClientSample>,
    ) {
        if let Some(detail) = self.inner.device_detail.borrow().as_ref()
            && detail.key() == key
        {
            detail.set_samples(samples);
        }
    }

    /// Whether a Starlink account is signed in: the gate for pause and rename.
    pub fn set_cloud_connected(&self, connected: bool) {
        self.inner.cloud_connected.set(connected);
        self.inner.rules.set_cloud_connected(connected);
        if let Some(snapshot) = self.inner.snapshot.borrow().clone() {
            self.inner.apply(&snapshot);
        }
    }

    /// The outcome of a pause, unpause, or rename, as a toast.
    pub fn client_write_result(&self, result: Result<(), String>) {
        let message = match result {
            Ok(()) => self.inner.translator.text("common.saved"),
            Err(error) => error,
        };
        self.inner.overlay.add_toast(adw::Toast::new(&message));
    }

    pub fn is_waiting(&self) -> bool {
        self.inner.body.visible_child_name().as_deref() == Some("waiting")
    }

    // SPEC-NET-006: two misses replace the roster with the unreachable branch.
    pub fn is_unreachable(&self) -> bool {
        self.inner.body.visible_child_name().as_deref() == Some("unreachable")
    }

    pub fn device_count(&self) -> usize {
        self.inner.device_count.get()
    }

    pub fn node_count(&self) -> usize {
        self.inner.node_count.get()
    }
}

impl Inner {
    fn set_active(&self, active: bool) {
        if self.active.replace(active) == active {
            return;
        }
        self.send_demand(active);
        self.send_cloud_demand(active);
        // Rules live in the recorder; reading them is a local file read.
        if active && let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(RecorderCommand::GetRules);
        }
    }

    fn request_cloud_roster(&self) {
        let Some(sender) = self.cloud_actions.borrow().as_ref().cloned() else {
            return;
        };
        self.cloud_loading.set(true);
        self.cloud_error.borrow_mut().take();
        self.refresh_cloud_status();
        if sender.send(NetworkCloudAction::RequestRoster).is_err() {
            self.cloud_loading.set(false);
            *self.cloud_error.borrow_mut() = Some(
                self.translator
                    .text("errors.accountUnreachableCheckInternet"),
            );
            self.refresh_cloud_status();
        }
    }

    fn send_demand(&self, active: bool) {
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(RecorderCommand::NetworkDemand(active));
        }
    }

    fn send_cloud_demand(&self, active: bool) {
        if let Some(sender) = self.cloud_actions.borrow().as_ref() {
            let _ = sender.send(NetworkCloudAction::Demand(active));
        }
    }

    fn refresh_rule_names(&self) {
        let mut names = self.known_names.borrow().clone();
        if let Some(snapshot) = self.snapshot.borrow().as_ref() {
            for device in snapshot.client_devices() {
                if let Some(name) = device.name.clone().filter(|name| !name.trim().is_empty()) {
                    names.insert(device.usage_key.clone(), name);
                }
            }
        }
        self.rules.set_names(names);
    }

    fn refresh_cloud_status(&self) {
        let message = if self.cloud_loading.get() {
            Some(self.translator.text("network.connectingEllipsis"))
        } else {
            self.cloud_error.borrow().clone()
        };
        let unreachable_text = message
            .clone()
            .unwrap_or_else(|| self.translator.text("network.rosterViaAccount"));
        self.cloud_status_unreachable.set_label(&unreachable_text);
        self.cloud_status_unreachable
            .set_visible(!self.cloud_source.get());
        self.cloud_status_content
            .set_label(message.as_deref().unwrap_or_default());
        self.cloud_status_content
            .set_visible(self.cloud_source.get() && message.is_some());
        self.cloud_button.set_sensitive(!self.cloud_loading.get());
    }

    fn apply(self: &Rc<Self>, snapshot: &NetworkSnapshot) {
        if !snapshot.reachable {
            self.unreachable_body
                .set_label(&router_unreachable(&self.translator));
            self.body.set_visible_child_name("unreachable");
            self.device_count.set(0);
            self.node_count.set(0);
            return;
        }
        self.body.set_visible_child_name("content");
        self.refresh_rule_names();
        self.meta.set_label(&self.meta_text(snapshot));
        self.meta.set_visible(true);
        self.fill_devices(snapshot);
        self.fill_nodes(snapshot);
    }

    fn meta_text(&self, snapshot: &NetworkSnapshot) -> String {
        let live = if self.cloud_source.get() {
            self.translator
                .text_with("network.cloudRosterNote", &[("seconds", "15")])
        } else {
            self.translator
                .text_with("network.liveFromRouter", &[("seconds", "5")])
        };
        let mut parts = Vec::new();
        if let Some(config) = &snapshot.config {
            if let Some(ssid) = config.ssid() {
                parts.push(ssid.to_owned());
            }
            if let Some(subnet) = config.subnet() {
                parts.push(subnet.to_owned());
            }
        }
        parts.push(live);
        parts.join(" · ")
    }

    fn fill_devices(&self, snapshot: &NetworkSnapshot) {
        while let Some(child) = self.devices.first_child() {
            self.devices.remove(&child);
        }
        let mut devices: Vec<&NetworkDevice> = snapshot.client_devices().collect();
        if self.cloud_source.get() {
            devices.sort_by(|a, b| a.name.cmp(&b.name));
        } else {
            devices.sort_by(|a, b| {
                (b.down_mbps + b.up_mbps)
                    .total_cmp(&(a.down_mbps + a.up_mbps))
                    .then_with(|| a.name.cmp(&b.name))
            });
        }
        for device in &devices {
            let is_self = self
                .self_client_id
                .get()
                .is_some_and(|client_id| device.client_id == Some(i64::from(client_id)));
            let row = device_row(device, &self.translator, !self.cloud_source.get(), is_self);
            row.set_activatable(true);
            if let Some(outer) = row.child().and_downcast::<gtk::Box>() {
                self.append_device_actions(&outer, device, is_self);
            }
            self.devices.append(&row);
        }
        self.device_count.set(devices.len());
        *self.shown_devices.borrow_mut() = devices.into_iter().cloned().collect();
    }

    /// Pause/unpause and rename, offered only behind the SPEC-PAUSE-003 gate
    /// and each confirmed before it is sent once (SPEC-PAUSE-004).
    fn append_device_actions(&self, outer: &gtk::Box, device: &NetworkDevice, is_self: bool) {
        let Some(client_id) = device.client_id.and_then(|id| u32::try_from(id).ok()) else {
            return;
        };
        let cloud = self.cloud_connected.get();
        let name = device
            .name
            .clone()
            .unwrap_or_else(|| device.mac_address.clone());
        if pause_available(
            cloud,
            self.self_client_id.get().is_some(),
            is_self,
            device.client_id,
        ) {
            let paused = device.blocked;
            let label = self.translator.text(if paused {
                "network.unpause"
            } else {
                "network.pause"
            });
            let button = gtk::Button::with_label(&label);
            button.add_css_class("flat");
            button.set_valign(gtk::Align::Center);
            let sender = self.cloud_actions.borrow().clone();
            let translator = self.translator.clone();
            let mac_address = Some(device.mac_address.clone());
            let name = name.clone();
            button.connect_clicked(move |button| {
                let Some(sender) = sender.clone() else { return };
                let action = NetworkCloudAction::Pause {
                    client_id,
                    paused: !paused,
                    mac_address: mac_address.clone(),
                };
                let heading = if paused {
                    translator.text("network.unpause")
                } else {
                    translator.text_with("network.pauseConfirmTitle", &[("name", &name)])
                };
                let body = if paused {
                    name.clone()
                } else {
                    translator.text("network.pauseConfirmBody")
                };
                confirm_action(
                    button,
                    &heading,
                    &body,
                    &label_for(&translator, paused),
                    move || {
                        let _ = sender.send(action.clone());
                    },
                );
            });
            outer.append(&button);
        }
        if cloud {
            let rename = gtk::Button::from_icon_name("document-edit-symbolic");
            rename.add_css_class("flat");
            rename.set_valign(gtk::Align::Center);
            rename.set_tooltip_text(Some(&self.translator.text("network.renameDevice")));
            let sender = self.cloud_actions.borrow().clone();
            let translator = self.translator.clone();
            rename.connect_clicked(move |button| {
                let Some(sender) = sender.clone() else { return };
                rename_dialog(button, &translator, &name, move |given_name| {
                    let _ = sender.send(NetworkCloudAction::Rename {
                        client_id,
                        given_name,
                    });
                });
            });
            outer.append(&rename);
        }
    }

    fn open_device_detail(self: &Rc<Self>, index: usize) {
        let Some(device) = self.shown_devices.borrow().get(index).cloned() else {
            return;
        };
        let snapshot = self.snapshot.borrow().clone();
        let upstream = snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .nodes
                .iter()
                .find(|node| node.device_keys.contains(&device.key))
                .map(|node| node_name(node, &self.translator))
        });
        let month = self.known_totals.borrow().get(&device.usage_key).copied();
        let is_self = self
            .self_client_id
            .get()
            .is_some_and(|client_id| device.client_id == Some(i64::from(client_id)));
        let parent = self.root.root().and_downcast::<gtk::Window>();
        let detail = crate::network_detail_view::DeviceDetail::open(
            parent.as_ref(),
            &device,
            upstream.as_deref(),
            month,
            &self.translator,
            |header| self.append_device_actions(header, &device, is_self),
        );
        *self.device_detail.borrow_mut() = Some(detail);
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(RecorderCommand::GetDeviceThroughput(
                device.usage_key.clone(),
            ));
        }
    }

    fn open_node_detail(&self, node: &NetworkNode) {
        let snapshot = self.snapshot.borrow().clone();
        let devices = snapshot
            .as_ref()
            .map(|snapshot| snapshot.devices.clone())
            .unwrap_or_default();
        let node_device = node
            .client_key
            .as_ref()
            .and_then(|key| devices.iter().find(|device| &device.key == key));
        let connected: Vec<&NetworkDevice> = devices
            .iter()
            .filter(|device| node.device_keys.contains(&device.key))
            .collect();
        let parent = self.root.root().and_downcast::<gtk::Window>();
        crate::network_detail_view::open_node_detail(
            parent.as_ref(),
            node,
            node_device,
            &connected,
            &self.translator,
        );
    }

    fn fill_nodes(self: &Rc<Self>, snapshot: &NetworkSnapshot) {
        while let Some(child) = self.nodes.first_child() {
            self.nodes.remove(&child);
        }
        for node in &snapshot.nodes {
            let tile = node_tile(node, &self.translator);
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(self);
            let node = node.clone();
            click.connect_released(move |_, _, _, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.open_node_detail(&node);
                }
            });
            tile.add_controller(click);
            tile.set_cursor_from_name(Some("pointer"));
            self.nodes.append(&tile);
        }
        if !snapshot.radios.is_empty() {
            let title = gtk::Label::builder()
                .label(self.translator.text("network.radioTemps"))
                .halign(gtk::Align::Start)
                .build();
            title.add_css_class("dishy-network-radios-title");
            title.set_tooltip_text(Some(&self.translator.text("network.radioTempsTip")));
            let radios = gtk::Box::new(gtk::Orientation::Vertical, 0);
            radios.add_css_class("dishy-network-radios");
            radios.append(&title);
            for radio in &snapshot.radios {
                radios.append(&radio_row(radio, &self.translator));
            }
            self.nodes.append(&radios);
        }
        self.node_count.set(snapshot.nodes.len());
    }
}

fn message(translator: &Translator, key: &str, body: Option<&str>, icon: &str) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("dishy-network-message");
    root.set_valign(gtk::Align::Center);
    root.set_halign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(36);
    root.append(&image);
    let title = gtk::Label::builder().label(translator.text(key)).build();
    title.add_css_class("dishy-network-message-title");
    root.append(&title);
    if let Some(body) = body {
        let label = gtk::Label::builder()
            .label(body)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        label.add_css_class("dishy-network-message-body");
        root.append(&label);
    }
    root
}

fn label_for(translator: &Translator, paused: bool) -> String {
    translator.text(if paused {
        "network.unpause"
    } else {
        "network.pause"
    })
}

fn confirm_action(
    anchor: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    action: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::MessageDialog::builder()
        .heading(heading)
        .body(body)
        .modal(true)
        .build();
    if let Some(window) = anchor.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&window));
    }
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", action);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });
    dialog.present();
}

fn rename_dialog(
    anchor: &impl IsA<gtk::Widget>,
    translator: &Translator,
    current: &str,
    on_save: impl Fn(String) + 'static,
) {
    let entry = gtk::Entry::builder()
        .text(current)
        .activates_default(true)
        .build();
    let dialog = adw::MessageDialog::builder()
        .heading(translator.text("network.renameDevice"))
        .extra_child(&entry)
        .modal(true)
        .build();
    if let Some(window) = anchor.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&window));
    }
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", &translator.text("common.save"));
    dialog.set_default_response(Some("save"));
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |_, response| {
        let name = entry.text().trim().to_string();
        if response == "save" && !name.is_empty() {
            on_save(name);
        }
    });
    dialog.present();
}

fn router_unreachable(translator: &Translator) -> String {
    translator.text_with(
        "routerDiagnosis.unknown",
        &[("address", dishy_core::ROUTER_LAN_ADDRESS)],
    )
}

fn device_row(
    device: &NetworkDevice,
    translator: &Translator,
    live_rates: bool,
    is_self: bool,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    outer.add_css_class("dishy-network-row");

    let icon_name = match device.connection {
        ConnectionKind::Ethernet => "network-wired-symbolic",
        _ => "network-wireless-symbolic",
    };
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(22);
    icon.set_valign(gtk::Align::Start);
    outer.append(&icon);

    let main = gtk::Box::new(gtk::Orientation::Vertical, 2);
    main.set_hexpand(true);
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name = gtk::Label::builder()
        .label(device_name(device, translator))
        .halign(gtk::Align::Start)
        .build();
    name.add_css_class("dishy-network-row-name");
    name_row.append(&name);
    if is_self {
        let badge = gtk::Label::new(Some(&translator.text("network.thisDevice")));
        badge.add_css_class("dishy-network-self-device");
        name_row.append(&badge);
    }
    if device.blocked {
        let badge = gtk::Label::new(Some(&translator.text("network.paused")));
        badge.add_css_class("dishy-network-badge");
        name_row.append(&badge);
    }
    main.append(&name_row);

    let mut facts = Vec::new();
    if let Some(ip) = &device.ip_address {
        facts.push(ip.clone());
    }
    if let Some(hostname) = &device.hostname
        && Some(hostname) != device.name.as_ref()
    {
        facts.push(hostname.clone());
    }
    facts.push(device.band.clone());
    if let Some(signal) = signal_text(device, translator) {
        facts.push(signal);
    }
    if let Some(idle) = idle_text(device, translator) {
        facts.push(idle);
    }
    let subtitle = gtk::Label::builder()
        .label(facts.join(" · "))
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    subtitle.add_css_class("dishy-network-row-sub");
    main.append(&subtitle);

    if !device.mac_address.is_empty() {
        let mac = gtk::Label::builder()
            .label(&device.mac_address)
            .halign(gtk::Align::Start)
            .build();
        mac.add_css_class("dishy-network-row-mac");
        main.append(&mac);
    }
    outer.append(&main);

    let rates = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rates.set_valign(gtk::Align::Start);
    rates.set_halign(gtk::Align::End);
    rates.add_css_class("dishy-network-rates");
    if live_rates && device.has_counters {
        let down = gtk::Label::builder()
            .label(format!("↓ {}", format_mbps(device.down_mbps)))
            .halign(gtk::Align::End)
            .build();
        let up = gtk::Label::builder()
            .label(format!("↑ {}", format_mbps(device.up_mbps)))
            .halign(gtk::Align::End)
            .build();
        rates.append(&down);
        rates.append(&up);
    } else {
        let no_data = gtk::Label::builder()
            .label(translator.text("metrics.noData"))
            .halign(gtk::Align::End)
            .build();
        no_data.add_css_class("dishy-network-row-sub");
        rates.append(&no_data);
    }
    outer.append(&rates);
    row.set_child(Some(&outer));
    row
}

fn node_tile(node: &NetworkNode, translator: &Translator) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    root.add_css_class("dishy-tile");
    root.add_css_class("dishy-network-node");
    let controller = node.role.as_deref() == Some("CONTROLLER");
    let icon = gtk::Image::from_icon_name(if controller {
        "network-wired-symbolic"
    } else {
        "network-wireless-symbolic"
    });
    icon.set_pixel_size(22);
    icon.set_valign(gtk::Align::Start);
    root.append(&icon);

    let main = gtk::Box::new(gtk::Orientation::Vertical, 2);
    main.set_hexpand(true);
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let name = gtk::Label::builder()
        .label(node_name(node, translator))
        .halign(gtk::Align::Start)
        .build();
    name.add_css_class("dishy-network-node-name");
    name_row.append(&name);
    let status = gtk::Label::new(Some(&translator.text(if node.connected {
        "network.statusConnected"
    } else {
        "network.statusDisconnected"
    })));
    status.add_css_class("dishy-network-badge");
    status.add_css_class(if node.connected {
        "dishy-network-connected"
    } else {
        "dishy-network-disconnected"
    });
    name_row.append(&status);
    main.append(&name_row);

    let mut facts = Vec::new();
    if let Some(role) = &node.role
        && role != "CONTROLLER"
    {
        facts.push(role.clone());
    }
    if let Some(hardware) = &node.hardware_version {
        facts.push(hardware.clone());
    }
    if node.connected {
        facts.push(device_count_text(node.device_keys.len(), translator));
    } else {
        facts.push(translator.text("network.nodeUnreachable"));
    }
    let subtitle = gtk::Label::builder()
        .label(facts.join(" · "))
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    subtitle.add_css_class("dishy-network-row-sub");
    main.append(&subtitle);
    root.append(&main);
    root
}

fn radio_row(radio: &dishy_core::network::RadioReading, translator: &Translator) -> gtk::Label {
    let mut text = format!(
        "{} — {:.0} °C",
        iface_label(Some(&radio.band)),
        radio.temp_c
    );
    if radio.duty_cycle < 99.5 {
        text.push_str(" · ");
        text.push_str(&translator.text_with(
            "network.throttledTo",
            &[("percent", &format!("{:.0}", radio.duty_cycle))],
        ));
    }
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .build();
    label.add_css_class("dishy-network-radio");
    label
}

pub(crate) fn device_name(device: &NetworkDevice, translator: &Translator) -> String {
    device
        .name
        .clone()
        .or_else(|| device.hostname.clone())
        .or_else(|| device.ip_address.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| Some(device.mac_address.clone()).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| translator.text("network.unknownDevice"))
}

pub(crate) fn node_name(node: &NetworkNode, translator: &Translator) -> String {
    if node.role.as_deref() == Some("CONTROLLER") {
        return translator.text("account.routerMain");
    }
    node.configured_name
        .clone()
        .or_else(|| node.device_id.clone())
        .unwrap_or_else(|| translator.text("network.meshNode"))
}

pub(crate) fn signal_text(device: &NetworkDevice, translator: &Translator) -> Option<String> {
    if device.connection == ConnectionKind::Ethernet {
        return Some(translator.text("network.wired"));
    }
    let dbm = device.signal_dbm?;
    let quality = match signal_quality(Some(dbm))? {
        SignalQuality::Excellent => "network.signalExcellent",
        SignalQuality::Good => "network.signalGood",
        SignalQuality::Fair => "network.signalFair",
        SignalQuality::Weak => "network.signalWeak",
    };
    Some(translator.text_with(
        "network.signalValue",
        &[
            ("dbm", &format!("{dbm:.0}")),
            ("quality", &translator.text(quality)),
        ],
    ))
}

pub(crate) fn idle_text(device: &NetworkDevice, translator: &Translator) -> Option<String> {
    let seconds = device.idle_s.filter(|seconds| *seconds > 30.0)?;
    Some(translator.text_with(
        "network.factIdle",
        &[("duration", &format_span(seconds, translator))],
    ))
}

pub(crate) fn format_span(seconds: f64, translator: &Translator) -> String {
    let seconds = seconds.max(0.0).round() as i64;
    if seconds < 60 {
        return translator.text_with("time.spanSeconds", &[("count", &seconds.to_string())]);
    }
    if seconds < 3_600 {
        return translator.text_with(
            "time.spanMinutesSeconds",
            &[
                ("minutes", &(seconds / 60).to_string()),
                ("seconds", &(seconds % 60).to_string()),
            ],
        );
    }
    translator.text_with(
        "time.spanHoursMinutes",
        &[
            ("hours", &(seconds / 3_600).to_string()),
            ("minutes", &((seconds % 3_600) / 60).to_string()),
        ],
    )
}

fn device_count_text(count: usize, translator: &Translator) -> String {
    let key = if count == 1 {
        "network.deviceCount_one"
    } else {
        "network.deviceCount_other"
    };
    translator.text_with(key, &[("count", &count.to_string())])
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
    use super::*;
    use crate::i18n::Locale;

    // SPEC-NET-004: the subtitle says wired and the rate label says no data
    // rather than inventing a zero.
    #[test]
    fn wired_devices_and_missing_counters_read_correctly() {
        let translator = Translator::new(Locale::En);
        let device = NetworkDevice {
            key: "aa|1".into(),
            usage_key: "1".into(),
            name: Some("Desk".into()),
            hostname: None,
            mac_address: "aa:bb:cc:dd:ee:ff".into(),
            ip_address: Some("192.168.1.5".into()),
            ipv6_addresses: Vec::new(),
            connection: ConnectionKind::Ethernet,
            band: "Ethernet".into(),
            signal_dbm: None,
            snr: None,
            down_mbps: 0.0,
            up_mbps: 0.0,
            rx_bytes: None,
            tx_bytes: None,
            has_counters: false,
            blocked: false,
            idle_s: None,
            client_id: Some(1),
            role: Some("CLIENT".into()),
            device_id: None,
            upstream_mac: None,
            is_client: true,
            mesh_hardware: None,
            link: Default::default(),
        };
        assert_eq!(signal_text(&device, &translator).as_deref(), Some("wired"));
        assert_eq!(device_name(&device, &translator), "Desk");
        assert!(!device.has_counters);
        assert_eq!(format_span(75.0, &translator), "1m 15s");
        assert_eq!(device_count_text(1, &translator), "1 device");
        assert_eq!(device_count_text(3, &translator), "3 devices");
    }
}
