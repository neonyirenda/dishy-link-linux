//! The dashboard's Obstructions and Events & outages cards (spec 0187), ports
//! of Electron's upstream `ObstructionCard.tsx` and upstream `OutageLog.tsx`. Both draw from
//! data already in hand: the obstruction reply and the recorder's event log.

use std::cell::RefCell;
use std::rc::Rc;

use dishy_core::telemetry::{DishObstructionMap, DishStatus};
use dishy_sky::Survey;
use gtk::prelude::*;

use crate::events_model::{
    EventRow, event_clock, event_duration, event_kind_name, event_rows, event_title,
};
use crate::i18n::Translator;
use crate::recorder::EventsSnapshot;
use crate::sky_canvas::SkyCanvas;
use crate::sky_view::{format_sky_stats, live_survey, obstruction_legend, stat_block, stat_value};

/// Under half a percent obstructed reads as a clear view, as Electron's card.
pub fn obstruction_note_key(fraction_obstructed: Option<f64>) -> &'static str {
    if fraction_obstructed.unwrap_or(0.0) < 0.005 {
        "obstruction.clearViewNote"
    } else {
        "obstruction.obstructedNote"
    }
}

fn card(class: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("card");
    card.add_css_class("dishy-card");
    card.add_css_class(class);
    card
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .hexpand(true)
        .build();
    label.add_css_class("dishy-card-title");
    label
}

struct ObstructionInner {
    root: gtk::Box,
    stack: gtk::Stack,
    canvas: SkyCanvas,
    survey: RefCell<Option<Survey>>,
    sky_value: gtk::Label,
    observed_value: gtk::Label,
    note: gtk::Label,
    translator: Translator,
    on_open_sky: RefCell<Option<Box<dyn Fn()>>>,
}

/// The live obstruction dome, its controls, key, stats, and note.
#[derive(Clone)]
pub struct ObstructionCard {
    inner: Rc<ObstructionInner>,
}

impl ObstructionCard {
    pub fn new(translator: &Translator) -> Self {
        let root = card("dishy-obstruction-card");
        root.set_vexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&heading(&translator.text("obstruction.title")));
        let rotation = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text(translator.text("obstruction.pauseRotation"))
            .valign(gtk::Align::Center)
            .build();
        rotation.add_css_class("flat");
        rotation.add_css_class("circular");
        let trim = gtk::ToggleButton::builder()
            .icon_name("view-conceal-symbolic")
            .tooltip_text(translator.text("obstruction.hideUnmapped"))
            .valign(gtk::Align::Center)
            .build();
        trim.add_css_class("flat");
        trim.add_css_class("circular");
        let open_sky = gtk::Button::builder()
            .label(translator.text("obstruction.liveSatelliteView"))
            .valign(gtk::Align::Center)
            .build();
        open_sky.add_css_class("flat");
        open_sky.add_css_class("dishy-accent-link");
        open_sky.set_widget_name("dashboard-open-sky");
        header.append(&rotation);
        header.append(&trim);
        header.append(&open_sky);
        root.append(&header);

        let canvas = SkyCanvas::new();
        canvas.area().set_height_request(300);
        let waiting = gtk::Label::builder()
            .label(translator.text("obstruction.waiting"))
            .vexpand(true)
            .build();
        waiting.add_css_class("dim-label");
        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&waiting, Some("waiting"));
        stack.add_named(canvas.area(), Some("dome"));
        root.append(&stack);

        let legend = obstruction_legend(translator);
        legend.set_halign(gtk::Align::Center);
        root.append(&legend);
        let sky_value = stat_value();
        let observed_value = stat_value();
        let stats = gtk::Box::new(gtk::Orientation::Horizontal, 28);
        stats.set_halign(gtk::Align::Center);
        stats.set_homogeneous(true);
        stats.append(&stat_block(
            &translator.text("metrics.skyObstructed"),
            &sky_value,
        ));
        stats.append(&stat_block(
            &translator.text("obstruction.observedFor"),
            &observed_value,
        ));
        root.append(&stats);
        let note = gtk::Label::builder()
            .label(translator.text("obstruction.clearViewNote"))
            .xalign(0.0)
            .wrap(true)
            .build();
        note.add_css_class("dishy-callout");
        root.append(&note);

        let inner = Rc::new(ObstructionInner {
            root,
            stack,
            canvas,
            survey: RefCell::new(None),
            sky_value,
            observed_value,
            note,
            translator: translator.clone(),
            on_open_sky: RefCell::new(None),
        });
        let this = Rc::downgrade(&inner);
        inner.canvas.set_seed(move |renderer| {
            if let Some(survey) = this
                .upgrade()
                .and_then(|inner| inner.survey.borrow().clone())
            {
                renderer.set_survey(survey);
            }
        });
        let this = Rc::downgrade(&inner);
        rotation.connect_clicked(move |button| {
            let Some(inner) = this.upgrade() else { return };
            let rotating = inner
                .canvas
                .with_renderer(|renderer| renderer.toggle_rotation())
                .unwrap_or(true);
            button.set_icon_name(if rotating {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
            button.set_tooltip_text(Some(&inner.translator.text(if rotating {
                "obstruction.pauseRotation"
            } else {
                "obstruction.resumeRotation"
            })));
        });
        let this = Rc::downgrade(&inner);
        trim.connect_toggled(move |button| {
            let Some(inner) = this.upgrade() else { return };
            let trimming = button.is_active();
            inner
                .canvas
                .with_renderer(|renderer| renderer.set_trim_unmapped(trimming));
            button.set_tooltip_text(Some(&inner.translator.text(if trimming {
                "obstruction.showUnmapped"
            } else {
                "obstruction.hideUnmapped"
            })));
            inner.canvas.queue_render();
        });
        let this = Rc::downgrade(&inner);
        open_sky.connect_clicked(move |_| {
            if let Some(inner) = this.upgrade()
                && let Some(handler) = inner.on_open_sky.borrow().as_ref()
            {
                handler();
            }
        });
        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.inner.root
    }

    /// The latest obstruction reply and status, as the Sky page gets them.
    pub fn update(&self, status: Option<&DishStatus>, map: Option<&DishObstructionMap>) {
        let inner = &self.inner;
        let survey = map.and_then(|map| live_survey(status, map));
        inner
            .stack
            .set_visible_child_name(if survey.is_some() { "dome" } else { "waiting" });
        if let Some(survey) = survey.clone() {
            inner
                .canvas
                .with_renderer(|renderer| renderer.set_survey(survey));
        }
        *inner.survey.borrow_mut() = survey;
        let (obstructed, observed) = format_sky_stats(status, &inner.translator);
        inner.sky_value.set_label(&obstructed);
        inner.observed_value.set_label(&observed);
        let fraction = status
            .and_then(|status| status.obstruction_stats.as_ref())
            .map(|stats| stats.fraction_obstructed.0)
            .filter(|value| value.is_finite());
        inner
            .note
            .set_label(&inner.translator.text(obstruction_note_key(fraction)));
    }

    pub fn is_waiting(&self) -> bool {
        self.inner.stack.visible_child_name().as_deref() != Some("dome")
    }

    pub fn connect_open_sky(&self, handler: impl Fn() + 'static) {
        *self.inner.on_open_sky.borrow_mut() = Some(Box::new(handler));
    }

    pub fn activate_open_sky(&self) {
        if let Some(handler) = self.inner.on_open_sky.borrow().as_ref() {
            handler();
        }
    }
}

/// The recorder's event log, newest first, one compact row each.
#[derive(Clone)]
pub struct EventsCard {
    root: gtk::Box,
    count: gtk::Label,
    list: gtk::ListBox,
    empty: gtk::Label,
    rows: Rc<RefCell<Vec<EventRow>>>,
    translator: Translator,
}

impl EventsCard {
    pub fn new(translator: &Translator) -> Self {
        let root = card("dishy-events-card");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&heading(&translator.text("events.title")));
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        header.append(&count);
        root.append(&header);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("dishy-event-list");
        let empty = gtk::Label::builder()
            .label(translator.text("events.empty"))
            .xalign(0.0)
            .wrap(true)
            .build();
        empty.add_css_class("dim-label");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&list)
            .build();
        root.append(&scroller);
        root.append(&empty);
        let rows: Rc<RefCell<Vec<EventRow>>> = Rc::new(RefCell::new(Vec::new()));
        let opened = rows.clone();
        let detail_translator = translator.clone();
        list.connect_row_activated(move |list, row| {
            if let Some(selected) = opened.borrow().get(row.index() as usize) {
                let parent = list.root().and_downcast::<gtk::Window>();
                crate::events_view::open_event_detail(
                    parent.as_ref(),
                    selected,
                    &detail_translator,
                );
            }
        });
        let card = Self {
            root,
            count,
            list,
            empty,
            rows,
            translator: translator.clone(),
        };
        card.set_events(&EventsSnapshot::default());
        card
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_events(&self, snapshot: &EventsSnapshot) {
        let rows = event_rows(&snapshot.events, &snapshot.reports, &self.translator);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for row in &rows {
            self.list.append(&self.row_widget(row));
        }
        let count_key = if rows.len() == 1 {
            "events.count_one"
        } else {
            "events.count_other"
        };
        self.count.set_label(
            &self
                .translator
                .text_with(count_key, &[("count", &rows.len().to_string())]),
        );
        self.list.set_visible(!rows.is_empty());
        self.empty.set_visible(rows.is_empty());
        *self.rows.borrow_mut() = rows;
    }

    pub fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    /// Time, kind mark, title, duration, chevron: Electron's outage log row.
    fn row_widget(&self, row: &EventRow) -> gtk::ListBoxRow {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let time = gtk::Label::new(Some(&event_clock(&row.event)));
        time.add_css_class("dim-label");
        time.add_css_class("monospace");
        let mark = gtk::Label::new(Some("●"));
        mark.add_css_class("dishy-event-mark");
        mark.add_css_class(&format!(
            "dishy-event-{}",
            event_kind_name(&row.event, row.report.as_ref())
        ));
        let title = gtk::Label::builder()
            .label(event_title(&row.event, &self.translator))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let duration = gtk::Label::new(Some(&event_duration(
            row.event.duration_ms,
            &self.translator,
        )));
        duration.add_css_class("dim-label");
        duration.add_css_class("monospace");
        line.append(&time);
        line.append(&mark);
        line.append(&title);
        line.append(&duration);
        line.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        let item = gtk::ListBoxRow::builder()
            .child(&line)
            .activatable(true)
            .build();
        item.add_css_class("dishy-event-row");
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-DASHP-003: under half a percent obstructed is a clear view.
    #[test]
    fn the_note_switches_at_half_a_percent() {
        assert_eq!(obstruction_note_key(None), "obstruction.clearViewNote");
        assert_eq!(
            obstruction_note_key(Some(0.0049)),
            "obstruction.clearViewNote"
        );
        assert_eq!(
            obstruction_note_key(Some(0.005)),
            "obstruction.obstructedNote"
        );
    }
}
