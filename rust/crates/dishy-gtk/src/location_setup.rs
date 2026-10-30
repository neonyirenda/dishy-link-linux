//! The observer-location form shown on the sky page.
//!
//! Port of `components/satellite/LocationSetup.tsx`. The dish's own
//! `get_location` answers PermissionDenied on consumer plans, and this port
//! adds no device read for it (spec 0101), so the location comes from pasted
//! coordinates or a city-level IP guess. The widget owns no preferences and
//! makes no request. It reports what the user asked for, and the page routes
//! that to the preferences store or the ephemeris worker.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use dishy_sky::ObserverLocation;
use gtk::glib;
use gtk::prelude::*;

use crate::i18n::Translator;
use crate::observer_location::parse_coordinate_text;

const CSS: &str = r#"
.dishy-location-setup {
  background-color: rgba(0, 0, 0, 0.45);
  border-radius: 10px;
  border: 1px solid rgba(139, 151, 168, 0.14);
  padding: 12px 13px;
}
.dishy-location-help {
  font-size: 12.5px;
  color: alpha(currentColor, 0.66);
}
.dishy-location-setup entry {
  font-family: monospace;
  font-size: 12px;
  border-radius: 999px;
}
.dishy-location-source {
  font-size: 12.5px;
  font-weight: 600;
  color: alpha(currentColor, 0.66);
  padding: 0;
  min-height: 0;
}
.dishy-location-error {
  font-size: 12px;
  color: #ff8b8b;
}
"#;

#[derive(Default)]
struct Handlers {
    saved: Option<Box<dyn Fn(ObserverLocation)>>,
    ip_requested: Option<Box<dyn Fn()>>,
    device_requested: Option<Box<dyn Fn()>>,
}

#[derive(Clone)]
pub struct LocationSetup {
    root: gtk::Box,
    entry: gtk::Entry,
    ip_button: gtk::Button,
    device_button: gtk::Button,
    error_label: gtk::Label,
    translator: Translator,
    handlers: Rc<RefCell<Handlers>>,
}

impl LocationSetup {
    pub fn new(translator: &Translator) -> Self {
        install_css();
        let help = gtk::Label::builder().xalign(0.0).wrap(true).build();
        help.set_markup(&help_markup(&translator.text("satellite.locationHelp")));
        help.add_css_class("dishy-location-help");

        let entry = gtk::Entry::builder()
            .placeholder_text("6.5244, 3.3792")
            .hexpand(true)
            .build();
        entry.update_property(&[gtk::accessible::Property::Label(
            &translator.text("satellite.coordinatesField"),
        )]);
        let save_button = gtk::Button::with_label(&translator.text("common.save"));
        save_button.add_css_class("suggested-action");
        save_button.add_css_class("pill");

        let ip_button = gtk::Button::builder()
            .label(translator.text("satellite.approximateFromIp"))
            .halign(gtk::Align::Start)
            .build();
        ip_button.add_css_class("flat");
        ip_button.add_css_class("dishy-location-source");

        let device_button = gtk::Button::builder()
            .label(translator.text("satellite.useDeviceLocation"))
            .halign(gtk::Align::Start)
            .build();
        device_button.add_css_class("flat");
        device_button.add_css_class("dishy-location-source");

        let error_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        error_label.add_css_class("dishy-location-error");

        let entry_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        entry_row.append(&entry);
        entry_row.append(&save_button);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.add_css_class("dishy-location-setup");
        root.append(&help);
        root.append(&entry_row);
        let sources = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        sources.append(&device_button);
        sources.append(&ip_button);
        root.append(&sources);
        root.append(&error_label);

        let setup = Self {
            root,
            entry,
            ip_button,
            device_button,
            error_label,
            translator: translator.clone(),
            handlers: Rc::new(RefCell::new(Handlers::default())),
        };

        let this = setup.clone();
        setup.entry.connect_activate(move |_| this.submit_pasted());
        let this = setup.clone();
        save_button.connect_clicked(move |_| this.submit_pasted());
        let this = setup.clone();
        setup.ip_button.connect_clicked(move |_| {
            this.set_ip_busy(true);
            this.show_error(None);
            if let Some(handler) = this.handlers.borrow().ip_requested.as_ref() {
                handler();
            }
        });
        let this = setup.clone();
        setup.device_button.connect_clicked(move |_| {
            this.set_device_busy(true);
            this.show_error(None);
            if let Some(handler) = this.handlers.borrow().device_requested.as_ref() {
                handler();
            }
        });
        setup
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn connect_saved(&self, handler: impl Fn(ObserverLocation) + 'static) {
        self.handlers.borrow_mut().saved = Some(Box::new(handler));
    }

    pub fn connect_ip_requested(&self, handler: impl Fn() + 'static) {
        self.handlers.borrow_mut().ip_requested = Some(Box::new(handler));
    }

    pub fn connect_device_requested(&self, handler: impl Fn() + 'static) {
        self.handlers.borrow_mut().device_requested = Some(Box::new(handler));
    }

    /// The Location portal's answer to a request this form made (spec 0180).
    pub fn device_location_finished(&self, result: Result<ObserverLocation, ()>) {
        self.set_device_busy(false);
        match result {
            Ok(location) => self.save(location),
            Err(()) => self.show_error(Some(
                &self.translator.text("satellite.deviceLocationFailed"),
            )),
        }
    }

    /// The worker's answer to an IP lookup this form asked for.
    pub fn ip_lookup_finished(&self, result: Result<ObserverLocation, ()>) {
        self.set_ip_busy(false);
        match result {
            Ok(location) => self.save(location),
            Err(()) => self.show_error(Some(&self.translator.text("satellite.ipLocationFailed"))),
        }
    }

    /// Forget a half-typed entry and any error when the form is reopened.
    pub fn reset(&self) {
        self.entry.set_text("");
        self.show_error(None);
    }

    fn submit_pasted(&self) {
        match parse_coordinate_text(&self.entry.text()) {
            Some(location) => self.save(location),
            None => self.show_error(Some(
                &self.translator.text("satellite.coordinatesUnreadable"),
            )),
        }
    }

    fn save(&self, location: ObserverLocation) {
        self.show_error(None);
        if let Some(handler) = self.handlers.borrow().saved.as_ref() {
            handler(location);
        }
    }

    /// One source at a time: a lookup in flight disables both buttons.
    fn set_device_busy(&self, busy: bool) {
        self.device_button.set_sensitive(!busy);
        self.ip_button.set_sensitive(!busy);
        self.device_button.set_label(&self.translator.text(if busy {
            "satellite.locating"
        } else {
            "satellite.useDeviceLocation"
        }));
    }

    fn set_ip_busy(&self, busy: bool) {
        self.ip_button.set_sensitive(!busy);
        self.device_button.set_sensitive(!busy);
        self.ip_button.set_label(&self.translator.text(if busy {
            "satellite.lookingUp"
        } else {
            "satellite.approximateFromIp"
        }));
    }

    fn show_error(&self, message: Option<&str>) {
        self.error_label.set_label(message.unwrap_or(""));
        self.error_label.set_visible(message.is_some());
    }
}

/// The catalog marks the app name with `<strong>`. Escape everything else so
/// a translation cannot inject other Pango markup.
fn help_markup(text: &str) -> String {
    glib::markup_escape_text(text)
        .replace("&lt;strong&gt;", "<b>")
        .replace("&lt;/strong&gt;", "</b>")
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

    // SPEC-CONST-006: the help text keeps its emphasis and nothing else.
    #[test]
    fn help_markup_allows_only_the_catalog_emphasis() {
        assert_eq!(
            help_markup("open the <strong>Compass</strong> app & paste <i>it</i>"),
            "open the <b>Compass</b> app &amp; paste &lt;i&gt;it&lt;/i&gt;"
        );
    }
}
