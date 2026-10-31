//! The window an Events row opens: the event, a band switch's bands, and the
//! outage report with copy buttons (spec 0174). Widgets only; the words come
//! from `events_model`.

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::events_model::{
    EventRow, band_label, event_duration, event_span, event_title, report_sections, report_text,
};
use crate::i18n::Translator;

pub fn open_event_detail(
    parent: Option<&gtk::Window>,
    row: &EventRow,
    translator: &Translator,
) -> adw::Window {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(18);
    body.set_margin_end(18);

    let kind = crate::events_model::event_kind_name(&row.event, row.report.as_ref());
    let tag = gtk::Label::builder()
        .label(translator.text(&format!("eventDetail.kind.{kind}")))
        .xalign(0.0)
        .build();
    tag.add_css_class("caption-heading");
    tag.add_css_class(if kind == "outage" {
        "error"
    } else if kind == "degraded" {
        "warning"
    } else {
        "dim-label"
    });
    let title = gtk::Label::builder()
        .label(event_title(&row.event, translator))
        .xalign(0.0)
        .wrap(true)
        .build();
    title.add_css_class("title-3");
    let span = gtk::Label::builder()
        .label(format!(
            "{} · {}",
            event_span(&row.event, translator),
            event_duration(row.event.duration_ms, translator)
        ))
        .xalign(0.0)
        .build();
    span.add_css_class("dim-label");
    body.append(&tag);
    body.append(&title);
    body.append(&span);

    if let Some(switch) = &row.event.band_switch {
        let bands = gtk::Label::builder()
            .label(format!(
                "{} → {}",
                band_label(&switch.from_band, translator),
                band_label(&switch.to_band, translator)
            ))
            .xalign(0.0)
            .build();
        bands.update_property(&[gtk::accessible::Property::Description(
            &translator.text("eventDetail.bandSwitchTo"),
        )]);
        body.append(&bands);
    }

    match &row.report {
        Some(report) => {
            for section in report_sections(report, translator) {
                let group = adw::PreferencesGroup::builder()
                    .title(&section.title)
                    .build();
                for (label, value) in &section.rows {
                    let item = adw::ActionRow::builder()
                        .title(gtk::glib::markup_escape_text(label))
                        .build();
                    let value = gtk::Label::new(Some(value));
                    value.add_css_class("monospace");
                    item.add_suffix(&value);
                    group.add(&item);
                }
                if let Some(note) = &section.note {
                    group.set_description(Some(note));
                }
                body.append(&group);
            }
            let text = report_text(&row.event, report, translator);
            let json = serde_json::to_string_pretty(report).unwrap_or_default();
            let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            buttons.set_halign(gtk::Align::End);
            for (label_key, content) in [
                ("eventDetail.copyText", text),
                ("eventDetail.copyJson", json),
            ] {
                let button = gtk::Button::with_label(&translator.text(label_key));
                let idle = translator.text(label_key);
                let copied = translator.text("eventDetail.copied");
                button.connect_clicked(move |button| {
                    button.clipboard().set_text(&content);
                    button.set_label(&copied);
                    let button = button.clone();
                    let idle = idle.clone();
                    gtk::glib::timeout_add_seconds_local_once(2, move || button.set_label(&idle));
                });
                buttons.append(&button);
            }
            body.append(&buttons);
        }
        None => {
            let note = gtk::Label::builder()
                .label(translator.text("eventDetail.noReport"))
                .xalign(0.0)
                .wrap(true)
                .build();
            note.add_css_class("dim-label");
            body.append(&note);
        }
    }

    let window = adw::Window::builder()
        .modal(true)
        .default_width(480)
        .title(translator.text("eventDetail.title"))
        .build();
    if let Some(parent) = parent {
        window.set_transient_for(Some(parent));
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&body)
            .build(),
    ));
    window.set_content(Some(&toolbar));
    window.present();
    window
}
