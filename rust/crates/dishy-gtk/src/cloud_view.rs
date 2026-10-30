//! Account page presentation and messages. Credentials never enter this module.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use dishy_cloud::AccountData;
use gtk::prelude::*;
use libadwaita::prelude::*;

use crate::i18n::Translator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAction {
    SignIn,
    Refresh,
    Disconnect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CloudState {
    SignedOut,
    Loading,
    /// The login window may close now; account detail reads are still running.
    Authenticated,
    Ready {
        account: AccountData,
        usage: serde_json::Value,
    },
    Error(String),
}

#[derive(Clone)]
pub struct AccountPage {
    root: gtk::Box,
    state: gtk::Stack,
    title: gtk::Label,
    details: gtk::Label,
    devices: gtk::Box,
    error: gtk::Label,
    translator: Translator,
    action_sender: Rc<RefCell<Option<Sender<CloudAction>>>>,
    lan_online: Rc<RefCell<std::collections::HashSet<String>>>,
}

impl AccountPage {
    pub fn new(translator: &Translator) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(24);
        root.set_margin_bottom(24);
        root.set_margin_start(24);
        root.set_margin_end(24);
        let state = gtk::Stack::new();
        state.set_vexpand(true);
        let signed_out = gtk::Box::new(gtk::Orientation::Vertical, 10);
        signed_out.set_valign(gtk::Align::Center);
        let prompt = gtk::Label::new(Some(&translator.text("connect.title")));
        prompt.add_css_class("title-2");
        let sign_in = gtk::Button::with_label(&translator.text("connect.signInButton"));
        sign_in.set_widget_name("account-sign-in");
        signed_out.append(&prompt);
        signed_out.append(&sign_in);
        state.add_named(&signed_out, Some("signed-out"));

        // A spinner draws at whatever size it is given. Centre alignment keeps
        // it at its own 32 px inside the page the stack hands it.
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        spinner.set_widget_name("account-loading");
        state.add_named(&spinner, Some("loading"));

        let ready = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.add_css_class("title-2");
        title.set_hexpand(true);
        let refresh = gtk::Button::with_label(&translator.text("common.tryAgain"));
        refresh.set_widget_name("account-refresh");
        let disconnect = gtk::Button::with_label(&translator.text("account.disconnect"));
        disconnect.set_widget_name("account-disconnect");
        header.append(&title);
        header.append(&refresh);
        header.append(&disconnect);
        let details = gtk::Label::new(None);
        details.set_xalign(0.0);
        details.set_yalign(0.0);
        details.set_selectable(true);
        details.set_wrap(true);
        details.add_css_class("card");
        ready.append(&header);
        ready.append(&details);
        let devices = gtk::Box::new(gtk::Orientation::Vertical, 5);
        ready.append(&devices);
        // Scrolls rather than grows: the device list can be long, and a page
        // taller than the window pushes the navigation bar out of reach.
        let ready_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&ready)
            .build();
        ready_scroller.add_css_class("dishy-page-scroller");
        state.add_named(&ready_scroller, Some("ready"));

        let failed = gtk::Box::new(gtk::Orientation::Vertical, 8);
        failed.set_valign(gtk::Align::Center);
        let error = gtk::Label::new(None);
        error.set_wrap(true);
        error.add_css_class("error");
        let retry = gtk::Button::with_label(&translator.text("common.tryAgain"));
        retry.set_widget_name("account-retry");
        failed.append(&error);
        failed.append(&retry);
        state.add_named(&failed, Some("error"));
        root.append(&state);

        let page = Self {
            root,
            state,
            title,
            details,
            devices,
            error,
            translator: translator.clone(),
            action_sender: Rc::new(RefCell::new(None)),
            lan_online: Rc::new(RefCell::new(std::collections::HashSet::new())),
        };
        let sign_sender = page.action_sender.clone();
        sign_in.connect_clicked(move |_| send_action(&sign_sender, CloudAction::SignIn));
        let refresh_sender = page.action_sender.clone();
        refresh.connect_clicked(move |_| send_action(&refresh_sender, CloudAction::Refresh));
        let retry_sender = page.action_sender.clone();
        retry.connect_clicked(move |_| send_action(&retry_sender, CloudAction::Refresh));
        let disconnect_sender = page.action_sender.clone();
        disconnect
            .connect_clicked(move |_| send_action(&disconnect_sender, CloudAction::Disconnect));
        page.set_state(CloudState::SignedOut);
        page
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    pub fn set_action_sender(&self, sender: Sender<CloudAction>) {
        *self.action_sender.borrow_mut() = Some(sender);
    }

    pub fn request_refresh(&self) {
        send_action(&self.action_sender, CloudAction::Refresh);
    }

    pub fn set_state(&self, next: CloudState) {
        match next {
            CloudState::SignedOut => self.state.set_visible_child_name("signed-out"),
            CloudState::Loading | CloudState::Authenticated => {
                self.state.set_visible_child_name("loading")
            }
            CloudState::Ready {
                account: data,
                usage,
            } => {
                let identity = data.identity.as_ref();
                let line = data.service_line.as_ref().and_then(|v| v.get("content"));
                self.title.set_label(
                    identity
                        .and_then(|v| v.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| {
                            identity
                                .and_then(|v| v.get("email"))
                                .and_then(serde_json::Value::as_str)
                        })
                        .unwrap_or(&self.translator.text("account.account")),
                );
                let email = identity
                    .and_then(|v| v.get("email"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("—");
                let plan = line
                    .and_then(|v| v.pointer("/subscription/productDescription"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("—");
                let service = line
                    .and_then(|v| v.get("serviceLineNumber"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("—");
                let address = line
                    .and_then(|v| v.pointer("/serviceAddress/formattedAddress"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("—");
                let device_count = line
                    .and_then(|v| v.get("userTerminals"))
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                while let Some(child) = self.devices.first_child() {
                    self.devices.remove(&child);
                }
                let usage_gb = usage
                    .pointer("/content/billingCyclesAnnotated")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|cycles| cycles.last())
                    .and_then(|cycle| cycle.get("totalAmountGB"))
                    .and_then(serde_json::Value::as_f64);
                let bucket = usage
                    .pointer("/content/dataBuckets/0/name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.translator.text("cloudUsage.data"));
                let usage_limit = usage
                    .pointer("/content/servicePlan/usageLimitGB")
                    .and_then(serde_json::Value::as_f64);
                let usage_text = usage_gb
                    .map(|gb| {
                        let limit = usage_limit
                            .map(|value| {
                                if value >= 100_000.0 {
                                    self.translator.text_with(
                                        "cloudUsage.includedUnlimited",
                                        &[("allowance", "100 TB")],
                                    )
                                } else {
                                    self.translator.text_with(
                                        "cloudUsage.ofIncluded",
                                        &[("allowance", &format!("{value:.0} GB"))],
                                    )
                                }
                            })
                            .unwrap_or_else(|| "—".to_owned());
                        format!(
                            "{}: {gb:.2} GB {bucket} ({limit})",
                            self.translator.text("cloudUsage.data")
                        )
                    })
                    .unwrap_or_else(|| self.translator.text("cloudUsage.noCycle"));
                self.details.set_label(&format!(
                    "{}: {email}\n{}: {plan}\n{}: {service}\n{}: {address}\n{}: {device_count}\n{usage_text}",
                    self.translator.text("account.email"),
                    self.translator.text("account.plan"),
                    self.translator.text("account.serviceLine"),
                    self.translator.text("account.address"),
                    self.translator.text("account.devices"),
                ));
                // SPEC-ACCTDEV-001: the device list, from the account data in hand.
                for device in crate::account_devices_model::account_devices(
                    data.service_line.as_ref(),
                    &data.device_telemetry,
                    &self.lan_online.borrow(),
                    chrono::Utc::now().timestamp_millis(),
                    &self.translator,
                ) {
                    self.append_device(&device);
                }
                self.state.set_visible_child_name("ready");
            }
            CloudState::Error(message) => {
                self.error.set_label(&message);
                self.state.set_visible_child_name("error");
            }
        }
    }

    /// One device as an expandable row: name and status, then its fields.
    fn append_device(&self, device: &crate::account_devices_model::AccountDevice) {
        use crate::account_devices_model::{DeviceKindTag, DeviceStatus};
        let status = match device.status {
            DeviceStatus::Online => "online",
            DeviceStatus::Offline => "offline",
            DeviceStatus::Inactive => "inactive",
        };
        let row = libadwaita::ExpanderRow::builder()
            .title(gtk::glib::markup_escape_text(&device.name))
            .subtitle(self.translator.text(&format!("account.status.{status}")))
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(match device.kind {
            DeviceKindTag::Dish => "network-wireless-symbolic",
            DeviceKindTag::Router => "network-wired-symbolic",
        }));
        if device.status != DeviceStatus::Online {
            row.add_css_class("dim-label");
        }
        for (label, value) in &device.fields {
            let field = libadwaita::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(label))
                .build();
            let value = gtk::Label::builder()
                .label(value)
                .selectable(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            value.add_css_class("monospace");
            field.add_suffix(&value);
            row.add_row(&field);
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        list.append(&row);
        list.set_margin_bottom(6);
        self.devices.append(&list);
    }

    /// Telemetry ids answering on this LAN, so their dots read live.
    pub fn set_lan_online(&self, ids: std::collections::HashSet<String>) {
        *self.lan_online.borrow_mut() = ids;
    }

    pub fn device_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut child = self.devices.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget
                .first_child()
                .and_then(|row| row.downcast::<libadwaita::ExpanderRow>().ok())
            {
                names.push(row.title().to_string());
            }
            child = widget.next_sibling();
        }
        names
    }
}

fn send_action(sender: &Rc<RefCell<Option<Sender<CloudAction>>>>, action: CloudAction) {
    if let Some(sender) = sender.borrow().as_ref() {
        let _ = sender.send(action);
    }
}
