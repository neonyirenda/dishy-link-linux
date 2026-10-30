//! Live alert state, known checks, and the recorder's recent alert episodes.

use dishy_core::alert::{
    AlertEpisode, AlertSnapshot, AlertSource, AlertState, AlertTransitionKind, Severity,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use dishy_core::data_meter::DataLimitLabel;
use gtk::prelude::*;
use libadwaita::prelude::*;

use crate::events_model::{EventRow, event_duration, event_rows, event_title};
use crate::i18n::Translator;
use crate::recorder::{EventsSnapshot, RecorderCommand};

type ToggleHandler = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

#[derive(Clone)]
pub struct AlertsPage {
    root: gtk::Popover,
    header_status: gtk::Label,
    sound_toggle: gtk::ToggleButton,
    notifications_toggle: gtk::ToggleButton,
    on_sound: ToggleHandler,
    on_notifications: ToggleHandler,
    events_rows: gtk::ListBox,
    events_title: gtk::Label,
    events_empty: gtk::Label,
    events: Rc<RefCell<Vec<EventRow>>>,
    commands: Rc<RefCell<Option<Sender<RecorderCommand>>>>,
    translator: Translator,
    active_rows: gtk::Box,
    history_rows: gtk::Box,
    status_rows: gtk::Box,
    active_empty: gtk::Label,
    history_empty: gtk::Label,
    content: gtk::Stack,
}

impl AlertsPage {
    pub fn new(translator: &Translator) -> Self {
        let active_rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let history_rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let status_rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let active_empty = empty_label(translator.text("alerts.noActive"));
        let history_empty = empty_label(translator.text("alerts.noneCleared"));

        let active_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        active_content.append(&active_rows);
        active_content.append(&active_empty);

        let history_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        history_content.append(&history_rows);
        history_content.append(&history_empty);
        let status_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        status_content.append(&status_rows);

        let events_title = gtk::Label::builder().xalign(0.0).build();
        events_title.add_css_class("caption");
        events_title.add_css_class("dim-label");
        let events_rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        events_rows.add_css_class("boxed-list");
        let events_empty = empty_label(translator.text("events.empty"));
        let events_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        events_content.append(&events_title);
        events_content.append(&events_rows);
        events_content.append(&events_empty);

        let content = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .width_request(390)
            .height_request(420)
            .build();
        for (name, title, page) in [
            (
                "active",
                translator.text("alerts.tabActive"),
                Self::scrolled(&active_content),
            ),
            (
                "history",
                translator.text("alerts.tabHistory"),
                Self::scrolled(&history_content),
            ),
            (
                "status",
                translator.text("alerts.tabStatus"),
                Self::scrolled(&status_content),
            ),
            (
                "events",
                translator.text("events.title"),
                Self::scrolled(&events_content),
            ),
        ] {
            content.add_titled(&page, Some(name), &title);
        }
        let switcher = gtk::StackSwitcher::builder()
            .stack(&content)
            .halign(gtk::Align::Center)
            .build();
        switcher.set_widget_name("alerts-tabs");
        switcher.add_css_class("dishy-alert-tabs");
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_start(14)
            .margin_end(14)
            .build();
        let title = gtk::Label::builder()
            .label(translator.text("navigation.alerts"))
            .halign(gtk::Align::Start)
            .hexpand(true)
            .css_classes(["heading"])
            .build();
        let header_status = gtk::Label::builder().halign(gtk::Align::Start).build();
        header_status.add_css_class("caption");
        header_status.add_css_class("dim-label");
        let sound_toggle = gtk::ToggleButton::builder()
            .icon_name("audio-volume-high-symbolic")
            .active(true)
            .build();
        sound_toggle.add_css_class("flat");
        let notifications_toggle = gtk::ToggleButton::builder()
            .icon_name("preferences-system-notifications-symbolic")
            .tooltip_text(translator.text("notifications.on"))
            .build();
        notifications_toggle.add_css_class("flat");
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        title_row.append(&title);
        title_row.append(&sound_toggle);
        title_row.append(&notifications_toggle);
        header.append(&title_row);
        header.append(&header_status);
        header.append(&switcher);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.append(&header);
        panel.append(&content);
        let root = gtk::Popover::new();
        root.set_child(Some(&panel));
        let page = Self {
            root,
            header_status,
            sound_toggle,
            notifications_toggle,
            on_sound: Rc::new(RefCell::new(None)),
            on_notifications: Rc::new(RefCell::new(None)),
            events_rows,
            events_title,
            events_empty,
            events: Rc::new(RefCell::new(Vec::new())),
            commands: Rc::new(RefCell::new(None)),
            translator: translator.clone(),
            active_rows,
            history_rows,
            status_rows,
            active_empty,
            history_empty,
            content,
        };
        // The event log is a local file read; fetch it whenever the menu opens.
        let commands = page.commands.clone();
        page.root.connect_show(move |_| {
            if let Some(sender) = commands.borrow().as_ref() {
                let _ = sender.send(RecorderCommand::GetEvents);
            }
        });
        let events = page.events.clone();
        let translator = translator.clone();
        let root = page.root.clone();
        page.events_rows.connect_row_activated(move |_, row| {
            let rows = events.borrow();
            if let Some(selected) = rows.get(row.index() as usize) {
                let parent = root.root().and_downcast::<gtk::Window>();
                root.popdown();
                crate::events_view::open_event_detail(parent.as_ref(), selected, &translator);
            }
        });
        page.set_events(&EventsSnapshot::default());
        page.set_sound_on(true);
        let handler = page.on_sound.clone();
        let this = page.clone();
        page.sound_toggle.connect_toggled(move |toggle| {
            this.show_sound(toggle.is_active());
            if let Some(handler) = handler.borrow().as_ref() {
                handler(toggle.is_active());
            }
        });
        let handler = page.on_notifications.clone();
        page.notifications_toggle.connect_toggled(move |toggle| {
            if let Some(handler) = handler.borrow().as_ref() {
                handler(toggle.is_active());
            }
        });
        page.header_status
            .set_label(&header_line(0, &page.translator));
        page
    }

    /// Reflect the stored choice without reporting it back as a change.
    pub fn set_sound_on(&self, on: bool) {
        let handler = self.on_sound.borrow_mut().take();
        self.sound_toggle.set_active(on);
        self.show_sound(on);
        *self.on_sound.borrow_mut() = handler;
    }

    pub fn set_notifications_on(&self, on: bool) {
        let handler = self.on_notifications.borrow_mut().take();
        self.notifications_toggle.set_active(on);
        *self.on_notifications.borrow_mut() = handler;
    }

    pub fn connect_sound_toggled(&self, handler: impl Fn(bool) + 'static) {
        *self.on_sound.borrow_mut() = Some(Box::new(handler));
    }

    pub fn connect_notifications_toggled(&self, handler: impl Fn(bool) + 'static) {
        *self.on_notifications.borrow_mut() = Some(Box::new(handler));
    }

    pub fn toggle_sound(&self) {
        self.sound_toggle.set_active(!self.sound_toggle.is_active());
    }

    pub fn header_text(&self) -> String {
        self.header_status.label().to_string()
    }

    fn show_sound(&self, on: bool) {
        self.sound_toggle.set_icon_name(if on {
            "audio-volume-high-symbolic"
        } else {
            "audio-volume-muted-symbolic"
        });
        self.sound_toggle
            .set_tooltip_text(Some(&sound_title(on, &self.translator)));
    }

    pub fn set_command_sender(&self, sender: Option<Sender<RecorderCommand>>) {
        *self.commands.borrow_mut() = sender;
    }

    /// The event log and reports, matched into rows (spec 0174).
    pub fn set_events(&self, snapshot: &EventsSnapshot) {
        let rows = event_rows(&snapshot.events, &snapshot.reports, &self.translator);
        clear_list(&self.events_rows);
        for row in &rows {
            let kind = crate::events_model::event_kind_name(&row.event, row.report.as_ref());
            let subtitle = format!(
                "{} · {} · {}",
                self.translator.text(&format!("eventDetail.kind.{kind}")),
                crate::events_model::event_span(&row.event, &self.translator),
                event_duration(row.event.duration_ms, &self.translator)
            );
            let item = libadwaita::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(&event_title(
                    &row.event,
                    &self.translator,
                )))
                .subtitle(gtk::glib::markup_escape_text(
                    subtitle.trim_end_matches(" · "),
                ))
                .activatable(true)
                .build();
            item.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            self.events_rows.append(&item);
        }
        let count_key = if rows.len() == 1 {
            "events.count_one"
        } else {
            "events.count_other"
        };
        self.events_title.set_label(
            &self
                .translator
                .text_with(count_key, &[("count", &rows.len().to_string())]),
        );
        self.events_rows.set_visible(!rows.is_empty());
        self.events_empty.set_visible(rows.is_empty());
        *self.events.borrow_mut() = rows;
    }

    pub fn event_row_count(&self) -> usize {
        self.events.borrow().len()
    }

    pub fn popover(&self) -> &gtk::Popover {
        &self.root
    }

    pub fn widget(&self) -> &gtk::Popover {
        &self.root
    }

    pub fn is_visible(&self) -> bool {
        self.root.is_visible()
    }

    pub fn selected_view(&self) -> Option<String> {
        self.content
            .visible_child_name()
            .map(|name| name.to_string())
    }

    fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(child)
            .build()
    }

    pub fn set_snapshot(&self, snapshot: &AlertSnapshot, translator: &Translator, now_ms: i64) {
        self.header_status
            .set_label(&header_line(snapshot.active.len(), translator));
        clear_rows(&self.active_rows);
        clear_rows(&self.history_rows);
        clear_rows(&self.status_rows);

        self.active_empty
            .set_label(&translator.text("alerts.noActive"));
        self.active_empty.set_visible(snapshot.active.is_empty());
        for state in &snapshot.active {
            append_alert_row(&self.active_rows, state, translator);
        }

        self.history_empty
            .set_label(&translator.text("alerts.noneCleared"));
        self.history_empty.set_visible(snapshot.history.is_empty());
        for episode in &snapshot.history {
            append_history_row(&self.history_rows, episode, translator, now_ms);
        }

        append_status_rows(
            &self.status_rows,
            &snapshot.status_list,
            snapshot.dish_reachable,
            AlertSource::Dish,
            translator,
        );
        append_status_rows(
            &self.status_rows,
            &snapshot.status_list,
            snapshot.router_reachable,
            AlertSource::Router,
            translator,
        );
    }
}

fn empty_label(text: String) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .css_classes(["dim-label"])
        .visible(false)
        .build()
}

fn clear_rows(rows: &gtk::Box) {
    while let Some(child) = rows.first_child() {
        rows.remove(&child);
    }
}

/// "all healthy" or the active count, for the menu header.
pub fn header_line(active: usize, translator: &Translator) -> String {
    match active {
        0 => translator.text("notifications.allHealthy"),
        1 => translator.text_with("notifications.activeCount_one", &[("count", "1")]),
        count => translator.text_with(
            "notifications.activeCount_other",
            &[("count", &count.to_string())],
        ),
    }
}

pub fn sound_title(on: bool, translator: &Translator) -> String {
    translator.text(if on {
        "notifications.soundsOnTitle"
    } else {
        "notifications.soundsMutedTitle"
    })
}

/// The words for one alert, wherever it is shown: the active list, history,
/// or a desktop notification. Rule alerts carry their subject and measure in
/// their label (`DataLimitLabel`); every other alert names its device.
pub fn alert_message(
    source: AlertSource,
    key: &str,
    label: Option<&str>,
    kind: AlertTransitionKind,
    translator: &Translator,
) -> String {
    let firing = kind == AlertTransitionKind::Fired;
    if source == AlertSource::System && key.starts_with("dataLimit:") {
        let rule = label
            .and_then(DataLimitLabel::parse)
            .unwrap_or_else(|| DataLimitLabel {
                subject: label.unwrap_or_default().to_owned(),
                detail: "—".into(),
                timing: false,
                as_group: false,
            });
        let measure = if rule.timing { "time" } else { "allowance" };
        let face = if firing { "Firing" } else { "Ok" };
        let group = if rule.as_group { "Group" } else { "" };
        let detail_name = if rule.timing { "duration" } else { "allowance" };
        return translator.text_with(
            &format!("alerts.dataLimit.{measure}{face}{group}"),
            &[("subject", &rule.subject), (detail_name, &rule.detail)],
        );
    }
    let face = if firing { "firing" } else { "ok" };
    let message_key = if source == AlertSource::System && key.starts_with("monthlyDataLimit:") {
        format!("alerts.system.monthlyDataLimit.{face}")
    } else {
        format!("alerts.{}.{key}.{face}", source.as_str())
    };
    label.map_or_else(
        || translator.text(&message_key),
        |device| translator.text_with(&message_key, &[("device", device)]),
    )
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn append_alert_row(rows: &gtk::Box, state: &AlertState, translator: &Translator) {
    let message = alert_message(
        state.source,
        &state.key,
        state.label.as_deref(),
        AlertTransitionKind::Fired,
        translator,
    );
    let severity = severity_label(state.severity, translator);
    let source = source_label(state.source, translator);
    append_row(rows, message, format!("{source} · {severity}"));
}

fn append_history_row(
    rows: &gtk::Box,
    episode: &AlertEpisode,
    translator: &Translator,
    now_ms: i64,
) {
    let message = alert_message(
        episode.source,
        &episode.key,
        episode.label.as_deref(),
        AlertTransitionKind::Fired,
        translator,
    );
    append_row(rows, message, history_subtitle(episode, translator, now_ms));
}

fn history_subtitle(episode: &AlertEpisode, translator: &Translator, now_ms: i64) -> String {
    let timing = match episode.end_ms {
        Some(end_ms) => translator.text_with(
            "alerts.lastedCleared",
            &[
                (
                    "span",
                    &format_duration(end_ms.saturating_sub(episode.start_ms)),
                ),
                ("when", &format_alert_age(end_ms, now_ms, translator)),
            ],
        ),
        None => translator.text_with(
            "alerts.startedAgo",
            &[(
                "when",
                &format_alert_age(episode.start_ms, now_ms, translator),
            )],
        ),
    };
    format!(
        "{} · {} · {timing}",
        source_label(episode.source, translator),
        severity_label(episode.severity, translator)
    )
}

fn append_status_rows(
    rows: &gtk::Box,
    states: &[AlertState],
    reachable: Option<bool>,
    source: AlertSource,
    translator: &Translator,
) {
    let source_states: Vec<_> = states
        .iter()
        .filter(|state| state.source == source)
        .collect();
    if source_states.is_empty() {
        return;
    }
    for state in source_states {
        let message = status_message(state, translator);
        let subtitle = status_row_subtitle(state, reachable, translator);
        append_row(rows, message, subtitle);
    }
}

fn status_row_subtitle(
    state: &AlertState,
    reachable: Option<bool>,
    translator: &Translator,
) -> String {
    format!(
        "{} · {}",
        source_label(state.source, translator),
        status_subtitle(reachable, state.severity, translator)
    )
}

fn status_message(state: &AlertState, translator: &Translator) -> String {
    let kind = if state.active {
        dishy_core::alert::AlertTransitionKind::Fired
    } else {
        dishy_core::alert::AlertTransitionKind::Cleared
    };
    translator.text(&state.message_key(kind))
}

fn status_subtitle(reachable: Option<bool>, severity: Severity, translator: &Translator) -> String {
    match reachable {
        Some(false) => translator.text("alerts.offlineLastKnown"),
        None => translator.text("alerts.awaitingStatus"),
        Some(true) => severity_label(severity, translator),
    }
}

fn append_row(rows: &gtk::Box, title: String, subtitle: String) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    let title = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    let subtitle = gtk::Label::builder()
        .label(subtitle)
        .halign(gtk::Align::Start)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    row.append(&title);
    row.append(&subtitle);
    rows.append(&row);
}

fn source_label(source: AlertSource, translator: &Translator) -> String {
    let key = match source {
        AlertSource::Dish => "alerts.deviceDish",
        AlertSource::Router => "alerts.deviceRouter",
        AlertSource::System => "alerts.deviceSystem",
    };
    translator.text(key)
}

fn severity_label(severity: Severity, translator: &Translator) -> String {
    let key = match severity {
        Severity::Critical => "alerts.severityCritical",
        Severity::Warning => "alerts.severityWarning",
        Severity::Advisory => "alerts.severityAdvisory",
    };
    translator.text(key)
}

fn format_alert_age(timestamp_ms: i64, now_ms: i64, translator: &Translator) -> String {
    let minutes = now_ms.saturating_sub(timestamp_ms).max(0) / 60_000;
    if minutes == 0 {
        return translator.text("time.justNow");
    }
    if minutes < 60 {
        return translator.text_with("time.minutesAgo", &[("count", &minutes.to_string())]);
    }
    let hours = minutes / 60;
    if hours < 24 {
        let key = if hours == 1 {
            "time.hoursAgo_one"
        } else {
            "time.hoursAgo_other"
        };
        return translator.text_with(key, &[("count", &hours.to_string())]);
    }
    let days = hours / 24;
    let key = if days == 1 {
        "time.daysAgo_one"
    } else {
        "time.daysAgo_other"
    };
    translator.text_with(key, &[("count", &days.to_string())])
}

fn format_duration(duration_ms: i64) -> String {
    let minutes = duration_ms.max(0) / 60_000;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {}m", minutes % 60);
    }
    let days = hours / 24;
    format!("{days}d {}h", hours % 24)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Locale, SUPPORTED_LOCALES};
    use dishy_core::alert::{known_status_keys, severity_for};

    // SPEC-SOUND-004: the header says all healthy or counts active alerts, and
    // the mute toggle's title says what clicking does.
    #[test]
    fn the_menu_header_counts_and_titles_its_toggle() {
        let english = Translator::new(Locale::En);
        assert_eq!(header_line(0, &english), "Alerts — all healthy");
        assert_eq!(header_line(1, &english), "1 active alert");
        assert_eq!(header_line(3, &english), "3 active alerts");
        assert_eq!(
            sound_title(true, &english),
            "Alert sounds on — click to mute"
        );
        assert_eq!(
            sound_title(false, &english),
            "Alert sounds muted — click to unmute"
        );
    }

    // SPEC-PAUSE-006 / SPEC-RULE-009: one function words every alert; a rule
    // alert names its subject and measure, for a device or a group, and a
    // monthly limit keeps its spec 0161 wording.
    #[test]
    fn alert_messages_word_rules_limits_and_device_alerts() {
        use dishy_core::data_meter::DataLimitLabel;
        let english = Translator::new(Locale::En);
        let label = |subject: &str, detail: &str, timing: bool, as_group: bool| {
            DataLimitLabel {
                subject: subject.into(),
                detail: detail.into(),
                timing,
                as_group,
            }
            .encode()
        };
        let fired = AlertTransitionKind::Fired;
        assert_eq!(
            alert_message(
                AlertSource::System,
                "dataLimit:42",
                Some(&label("Phone", "20.0 GB", false, false)),
                fired,
                &english
            ),
            "Phone reached its 20.0 GB data allowance"
        );
        assert_eq!(
            alert_message(
                AlertSource::System,
                "dataLimit:group:kids",
                Some(&label("Kids", "2h", true, true)),
                fired,
                &english
            ),
            "Kids reached the end of their 2h timer"
        );
        assert_eq!(
            alert_message(
                AlertSource::System,
                "dataLimit:42",
                Some("Phone"),
                fired,
                &english
            ),
            alert_message(
                AlertSource::System,
                "dataLimit:42",
                Some(&label("Phone", "—", false, false)),
                fired,
                &english
            ),
            "an unstructured label still reads"
        );
        assert!(
            alert_message(
                AlertSource::System,
                "monthlyDataLimit:42",
                Some("Phone"),
                fired,
                &english
            )
            .contains("Phone")
        );
    }

    #[test]
    fn every_alert_status_and_section_label_is_translated() {
        for locale in SUPPORTED_LOCALES {
            let translator = Translator::new(locale);
            for key in [
                "alerts.deviceDish",
                "alerts.deviceRouter",
                "alerts.deviceSystem",
                "alerts.tabActive",
                "alerts.tabHistory",
                "alerts.tabStatus",
                "alerts.noActive",
                "alerts.noneCleared",
                "alerts.offlineLastKnown",
                "alerts.awaitingStatus",
                "alerts.firingTitle",
                "alerts.clearedTitle",
                "alerts.startedAgo",
                "alerts.lastedCleared",
                "alerts.severityCritical",
                "alerts.severityWarning",
                "alerts.severityAdvisory",
            ] {
                assert_ne!(translator.text(key), key, "{} missing {key}", locale.code());
            }
            for source in [AlertSource::Dish, AlertSource::Router, AlertSource::System] {
                for key in known_status_keys(source) {
                    for state in ["ok", "firing"] {
                        let message = translator.text(&format!(
                            "alerts.{}.{}.{}",
                            source.as_str(),
                            key,
                            state
                        ));
                        assert_ne!(
                            message,
                            format!("alerts.{}.{}.{}", source.as_str(), key, state)
                        );
                    }
                    assert!(severity_for(source, key).is_some());
                }
            }
        }
    }

    // SPEC-ALERT-005: known checks remain visible as stale, not all-clear, while
    // their device is offline; an unobserved device is distinct from offline.
    #[test]
    fn offline_status_is_marked_as_last_known() {
        let translator = Translator::new(Locale::En);
        assert_eq!(
            status_subtitle(Some(false), Severity::Critical, &translator),
            "Starlink offline · last known status"
        );
        assert_eq!(
            status_subtitle(None, Severity::Critical, &translator),
            "Waiting for the first status reply."
        );
        let known = AlertState {
            source: AlertSource::Dish,
            key: "thermalShutdown".into(),
            severity: Severity::Critical,
            active: false,
            label: None,
        };
        assert_eq!(status_message(&known, &translator), "Not overheated");
        assert!(
            status_row_subtitle(&known, Some(false), &translator)
                .contains("Dish · Starlink offline")
        );
    }

    // SPEC-ALERT-005: closed episodes remain identifiable by their localized
    // alert description and a relative cleared time.
    #[test]
    fn history_rows_use_the_alert_description_and_cleared_timestamp() {
        let translator = Translator::new(Locale::En);
        let episode = AlertEpisode {
            source: AlertSource::Dish,
            key: "thermalShutdown".into(),
            severity: Severity::Critical,
            start_ms: 1_000,
            end_ms: Some(121_000),
            label: None,
        };
        let subtitle = history_subtitle(&episode, &translator, 181_000);
        assert!(subtitle.contains("lasted 2m"));
        assert!(subtitle.contains("cleared 1 min ago"));
    }

    #[test]
    fn ages_and_durations_keep_recent_and_cleared_history_readable() {
        let translator = Translator::new(Locale::En);
        assert_eq!(format_alert_age(100_000, 100_000, &translator), "just now");
        assert_eq!(format_duration(3_600_000), "1h 0m");
    }
}
