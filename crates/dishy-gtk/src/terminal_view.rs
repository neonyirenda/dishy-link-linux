//! The dish terminal card and its expanded window (spec 0173). Widgets only;
//! the facts come from `terminal_model`.

use dishy_core::telemetry::DishStatus;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::i18n::Translator;
use crate::terminal_model::{Tone, stale_header, terminal_facts, update_banner};

/// One view of the facts. The dashboard card and the expanded window are two
/// of these, so they cannot drift apart.
#[derive(Clone)]
pub struct TerminalFacts {
    root: gtk::Box,
    stale: gtk::Label,
    device_id: gtk::Label,
    banner: gtk::Label,
    grid: gtk::FlowBox,
    translator: Translator,
}

impl TerminalFacts {
    pub fn new(translator: &Translator, expand: Option<&gtk::Button>, title: Option<&str>) -> Self {
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(title) = title {
            let heading = gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .build();
            heading.add_css_class("heading");
            header.append(&heading);
        } else {
            header.append(&gtk::Box::builder().hexpand(true).build());
        }
        let stale = gtk::Label::builder().visible(false).build();
        stale.add_css_class("error");
        stale.add_css_class("caption");
        let device_id = gtk::Label::new(None);
        device_id.add_css_class("monospace");
        device_id.add_css_class("dim-label");
        device_id.add_css_class("caption");
        header.append(&stale);
        header.append(&device_id);
        if let Some(expand) = expand {
            header.append(expand);
        }
        let banner = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        banner.add_css_class("warning");
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(3)
            .column_spacing(18)
            .row_spacing(6)
            .homogeneous(true)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.append(&header);
        root.append(&banner);
        root.append(&grid);
        Self {
            root,
            stale,
            device_id,
            banner,
            grid,
            translator: translator.clone(),
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// `stale_age` is set while the dish is not answering: `Some(None)` when
    /// the age of the facts is unknown.
    pub fn set(&self, status: &DishStatus, at_ms: i64, stale_age: Option<Option<String>>) {
        let translator = &self.translator;
        match stale_age {
            Some(age) => {
                self.stale.set_label(&stale_header(age, translator));
                self.stale.set_visible(true);
            }
            None => self.stale.set_visible(false),
        }
        self.device_id.set_label(
            status
                .device_info
                .as_ref()
                .and_then(|info| info.id.as_deref())
                .unwrap_or_default(),
        );
        match update_banner(status, translator) {
            Some(text) => {
                self.banner.set_label(&format!("↻ {text}"));
                self.banner.set_visible(true);
            }
            None => self.banner.set_visible(false),
        }
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for fact in terminal_facts(status, at_ms, translator) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::builder()
                .label(&fact.label)
                .xalign(0.0)
                .hexpand(true)
                .build();
            label.add_css_class("dim-label");
            let value = gtk::Label::builder()
                .label(&fact.value)
                .xalign(1.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .tooltip_text(&fact.value)
                .build();
            value.add_css_class("monospace");
            match fact.tone {
                Some(Tone::Good) => value.add_css_class("success"),
                Some(Tone::Warn) => value.add_css_class("warning"),
                Some(Tone::Bad) => value.add_css_class("error"),
                None => {}
            }
            row.append(&label);
            row.append(&value);
            self.grid.insert(&row, -1);
        }
    }
}

/// The expanded window: the same facts, the full width of their own window.
pub fn open_terminal_window(
    parent: Option<&gtk::Window>,
    translator: &Translator,
) -> (adw::Window, TerminalFacts) {
    let facts = TerminalFacts::new(translator, None, None);
    facts.widget().set_margin_top(16);
    facts.widget().set_margin_bottom(16);
    facts.widget().set_margin_start(18);
    facts.widget().set_margin_end(18);
    let window = adw::Window::builder()
        .modal(true)
        .default_width(760)
        .default_height(520)
        .title(translator.text("navigation.dishTerminal"))
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(facts.widget())
            .build(),
    ));
    window.set_content(Some(&toolbar));
    window.present();
    (window, facts)
}
