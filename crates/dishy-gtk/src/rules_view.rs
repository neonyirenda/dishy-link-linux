//! The Network page's Rules tab: a card per device rule and per group, and
//! the dialog that writes them.
//!
//! Widgets only; every line a card shows and every command a form sends come
//! from `rules_model`. Changes go to the recorder thread; the cards redraw
//! from the snapshot it publishes back, so a failed write is never shown as
//! saved (SPEC-PAUSE-005).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use dishy_core::data_meter::MeterCycle;
use dishy_core::schedule::{ScheduleMode, ScheduleWindow};
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::i18n::Translator;
use crate::recorder::{RecorderCommand, RulesSnapshot};
use crate::rules_model::{
    FormError, RuleCard, RuleForm, RuleKind, cycle_label, form_command, form_for, new_form,
    rule_cards,
};

#[derive(Clone)]
pub struct RulesView {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Box,
    list: gtk::ListBox,
    empty: gtk::Label,
    account_note: gtk::Label,
    error: gtk::Label,
    translator: Translator,
    snapshot: RefCell<Arc<RulesSnapshot>>,
    names: RefCell<HashMap<String, String>>,
    commands: RefCell<Option<Sender<RecorderCommand>>>,
    cloud_connected: Cell<bool>,
}

impl RulesView {
    pub fn new(translator: &Translator) -> Self {
        let description = gtk::Label::builder()
            .label(translator.text("rules.description"))
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .build();
        description.add_css_class("dim-label");
        let new_button = gtk::Button::with_label(&translator.text("rules.newRule"));
        new_button.add_css_class("suggested-action");
        new_button.add_css_class("pill");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header.append(&description);
        header.append(&new_button);

        let account_note = gtk::Label::builder()
            .label(translator.text("rules.autoPauseOffRule"))
            .xalign(0.0)
            .wrap(true)
            .build();
        account_note.add_css_class("caption");
        account_note.add_css_class("dim-label");

        let empty = gtk::Label::builder()
            .label(translator.text("rules.noRules"))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .build();
        empty.add_css_class("dim-label");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        error.add_css_class("error");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.append(&header);
        root.append(&account_note);
        root.append(&error);
        root.append(&empty);
        root.append(&list);

        let view = Self {
            inner: Rc::new(Inner {
                root,
                list,
                empty,
                account_note,
                error,
                translator: translator.clone(),
                snapshot: RefCell::new(Arc::new(RulesSnapshot::default())),
                names: RefCell::new(HashMap::new()),
                commands: RefCell::new(None),
                cloud_connected: Cell::new(false),
            }),
        };
        let this = Rc::downgrade(&view.inner);
        new_button.connect_clicked(move |button| {
            if let Some(inner) = this.upgrade() {
                open_dialog(&inner, button, new_form(Vec::new()), false);
            }
        });
        view.inner.redraw();
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.inner.root
    }

    pub fn set_command_sender(&self, sender: Option<Sender<RecorderCommand>>) {
        *self.inner.commands.borrow_mut() = sender;
    }

    /// Device names, keyed as the odometer and rules key them.
    pub fn set_names(&self, names: HashMap<String, String>) {
        *self.inner.names.borrow_mut() = names;
        self.inner.redraw();
    }

    pub fn set_snapshot(&self, snapshot: Arc<RulesSnapshot>) {
        *self.inner.snapshot.borrow_mut() = snapshot;
        self.inner.error.set_visible(false);
        self.inner.redraw();
    }

    pub fn set_cloud_connected(&self, connected: bool) {
        self.inner.cloud_connected.set(connected);
        self.inner.redraw();
    }

    pub fn write_failed(&self, error: &str) {
        let message = format!(
            "{} {error}",
            self.inner.translator.text("rules.recorderRefused")
        );
        self.inner.error.set_label(&message);
        self.inner.error.set_visible(true);
    }

    pub fn card_count(&self) -> usize {
        rule_cards(
            &self.inner.snapshot.borrow(),
            &self.inner.names.borrow(),
            dishy_core::calendar::now_ms(),
            &self.inner.translator,
        )
        .len()
    }
}

impl Inner {
    fn send(&self, command: RecorderCommand) {
        if let Some(sender) = self.commands.borrow().as_ref()
            && sender.send(command).is_err()
        {
            self.error
                .set_label(&self.translator.text("errors.recorderNotAnsweringRules"));
            self.error.set_visible(true);
        }
    }

    fn redraw(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let cards = rule_cards(
            &self.snapshot.borrow(),
            &self.names.borrow(),
            dishy_core::calendar::now_ms(),
            &self.translator,
        );
        self.empty.set_visible(cards.is_empty());
        self.list.set_visible(!cards.is_empty());
        // Rules here never pause anything on their own; with an account a
        // reached card offers the explicit pause on the device list.
        self.account_note
            .set_label(&self.translator.text(if self.cloud_connected.get() {
                "rules.autoPauseOffRule"
            } else {
                "rules.connectToPauseRule"
            }));
        for card in cards {
            self.list.append(&self.card_row(card));
        }
    }

    fn card_row(self: &Rc<Self>, card: RuleCard) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&card.title))
            .subtitle(glib::markup_escape_text(&format!(
                "{} · {}",
                card.measure, card.members
            )))
            .build();
        let status = gtk::Box::new(gtk::Orientation::Vertical, 4);
        status.set_valign(gtk::Align::Center);
        let progress = gtk::Label::builder()
            .label(&card.progress)
            .xalign(1.0)
            .build();
        progress.add_css_class("caption");
        status.append(&progress);
        if let Some(fraction) = card.fraction {
            let bar = gtk::LevelBar::builder()
                .value(fraction)
                .width_request(120)
                .build();
            status.append(&bar);
        }
        if card.reached {
            // Watch-only: reached, announced, and not paused (spec 0169).
            let reached = gtk::Label::new(Some(&self.translator.text("network.dataLimit")));
            reached.add_css_class("warning");
            reached.add_css_class("caption");
            status.append(&reached);
        }
        row.add_suffix(&status);

        let edit = gtk::Button::from_icon_name("document-edit-symbolic");
        edit.set_valign(gtk::Align::Center);
        edit.add_css_class("flat");
        edit.set_tooltip_text(Some(&self.translator.text("rules.editRule")));
        let restart = gtk::Button::from_icon_name("view-refresh-symbolic");
        restart.set_valign(gtk::Align::Center);
        restart.add_css_class("flat");
        restart.set_tooltip_text(Some(&self.translator.text("rules.startOver")));
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_valign(gtk::Align::Center);
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some(&self.translator.text("rules.removeRule")));
        row.add_suffix(&edit);
        row.add_suffix(&restart);
        row.add_suffix(&remove);

        let this = Rc::downgrade(self);
        let edit_card = card.clone();
        edit.connect_clicked(move |button| {
            if let Some(inner) = this.upgrade()
                && let Some(form) = form_for(&edit_card, &inner.snapshot.borrow())
            {
                open_dialog(&inner, button, form, true);
            }
        });
        let this = Rc::downgrade(self);
        let restart_card = card.clone();
        restart.connect_clicked(move |_| {
            let Some(inner) = this.upgrade() else { return };
            let snapshot = inner.snapshot.borrow().clone();
            for (client_key, group_id) in rule_targets(&restart_card, &snapshot) {
                inner.send(RecorderCommand::RestartRule {
                    client_key,
                    group_id,
                });
            }
        });
        let this = Rc::downgrade(self);
        remove.connect_clicked(move |button| {
            let Some(inner) = this.upgrade() else { return };
            let command = match (&card.group_id, &card.client_key) {
                (Some(group_id), _) => RecorderCommand::RemoveGroup(group_id.clone()),
                (None, Some(key)) => RecorderCommand::RemoveDeviceRule(key.clone()),
                (None, None) => return,
            };
            confirm(
                button,
                &inner.translator.text("rules.deleteRule"),
                &card.title,
                &inner.translator.text("rules.deleteRule"),
                {
                    let inner = inner.clone();
                    move || inner.send(command.clone())
                },
            );
        });
        row
    }
}

/// The (device, group) pairs a card's restart applies to.
fn rule_targets(card: &RuleCard, snapshot: &RulesSnapshot) -> Vec<(String, Option<String>)> {
    match (&card.group_id, &card.client_key) {
        (Some(group_id), _) => snapshot
            .rules
            .iter()
            .filter(|rule| rule.group_id.as_deref() == Some(group_id.as_str()))
            .map(|rule| (rule.client_key.clone(), Some(group_id.clone())))
            .collect(),
        (None, Some(key)) => vec![(key.clone(), None)],
        (None, None) => Vec::new(),
    }
}

fn confirm(
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
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });
    dialog.present();
}

const CYCLES: [&str; 6] = ["daily", "weekly", "monthly", "billing", "once", "custom"];

fn cycle_index(cycle: &MeterCycle) -> u32 {
    match cycle {
        MeterCycle::Daily => 0,
        MeterCycle::Weekly { .. } => 1,
        MeterCycle::Monthly { .. } => 2,
        MeterCycle::Billing { .. } => 3,
        MeterCycle::Once => 4,
        MeterCycle::Custom { .. } => 5,
    }
}

/// The cycle the dropdown and its one number describe. Weekly reads the
/// number as a weekday (0 = Sunday), custom as days, the monthly kinds as the
/// day of the month.
fn cycle_from(index: u32, number: u32, now_ms: i64) -> MeterCycle {
    match index {
        0 => MeterCycle::Daily,
        1 => MeterCycle::Weekly {
            weekday: number.min(6) as u8,
        },
        3 => MeterCycle::Billing {
            day: number.clamp(1, 31) as u8,
        },
        4 => MeterCycle::Once,
        5 => MeterCycle::Custom {
            days: number.max(1),
            start_ms: now_ms,
        },
        _ => MeterCycle::Monthly {
            day: number.clamp(1, 31) as u8,
        },
    }
}

fn cycle_number(cycle: &MeterCycle) -> u32 {
    match cycle {
        MeterCycle::Weekly { weekday } => u32::from(*weekday),
        MeterCycle::Monthly { day } | MeterCycle::Billing { day } => u32::from(*day),
        MeterCycle::Custom { days, .. } => *days,
        MeterCycle::Daily | MeterCycle::Once => 1,
    }
}

/// One timetable window's controls.
struct WindowRow {
    root: gtk::Box,
    days: Vec<gtk::ToggleButton>,
    start: gtk::SpinButton,
    end: gtk::SpinButton,
    window: ScheduleWindow,
}

impl WindowRow {
    fn new(window: ScheduleWindow) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let mut days = Vec::new();
        // Monday first, as the Electron editor lays the week out.
        for (weekday, letter) in [
            (1, "M"),
            (2, "T"),
            (3, "W"),
            (4, "T"),
            (5, "F"),
            (6, "S"),
            (0, "S"),
        ] {
            let toggle = gtk::ToggleButton::with_label(letter);
            toggle.set_active(window.weekdays.contains(&weekday));
            toggle.add_css_class("circular");
            root.append(&toggle);
            days.push(toggle);
        }
        let hour_spin = |minute: u32| {
            let spin = gtk::SpinButton::with_range(0.0, 24.0, 0.25);
            spin.set_digits(2);
            spin.set_value(f64::from(minute) / 60.0);
            spin
        };
        let start = hour_spin(window.start_minute);
        let end = hour_spin(window.end_minute);
        root.append(&start);
        root.append(&gtk::Label::new(Some("–")));
        root.append(&end);
        Self {
            root,
            days,
            start,
            end,
            window,
        }
    }

    fn value(&self) -> ScheduleWindow {
        let weekdays = [1_u8, 2, 3, 4, 5, 6, 0]
            .into_iter()
            .zip(&self.days)
            .filter(|(_, toggle)| toggle.is_active())
            .map(|(weekday, _)| weekday)
            .collect();
        ScheduleWindow {
            weekdays,
            start_minute: (self.start.value() * 60.0).round() as u32,
            end_minute: (self.end.value() * 60.0).round() as u32,
            // Dated runs are kept as they were saved; this editor sets weekdays.
            ..self.window.clone()
        }
    }
}

fn open_dialog(inner: &Rc<Inner>, anchor: &gtk::Button, form: RuleForm, editing: bool) {
    let translator = &inner.translator;
    let window = adw::Window::builder()
        .modal(true)
        .default_width(520)
        .title(translator.text(if editing {
            "rules.editRule"
        } else {
            "rules.createRule"
        }))
        .build();
    if let Some(parent) = anchor.root().and_downcast::<gtk::Window>() {
        window.set_transient_for(Some(&parent));
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let kinds = gtk::StringList::new(&[
        &translator.text("rules.modeLimit"),
        &translator.text("rules.modeSchedule"),
        &translator.text("rules.modeTimer"),
    ]);
    let kind = adw::ComboRow::builder()
        .title(translator.text("rules.whatEachKind"))
        .model(&kinds)
        .selected(match form.kind {
            RuleKind::Limit => 0,
            RuleKind::Schedule => 1,
            RuleKind::Timer => 2,
        })
        .build();

    let devices_group = adw::PreferencesGroup::builder()
        .title(translator.text("rules.appliesTo"))
        .build();
    let mut device_checks: Vec<(String, gtk::CheckButton)> = Vec::new();
    let mut names: Vec<(String, String)> = inner
        .names
        .borrow()
        .iter()
        .map(|(key, name)| (key.clone(), name.clone()))
        .collect();
    for key in &form.devices {
        if !names.iter().any(|(known, _)| known == key) {
            names.push((key.clone(), key.clone()));
        }
    }
    names.sort_by(|a, b| a.1.cmp(&b.1));
    for (key, name) in names {
        let check = gtk::CheckButton::builder()
            .active(form.devices.contains(&key))
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&name))
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        devices_group.add(&row);
        device_checks.push((key, check));
    }
    let name = adw::EntryRow::builder()
        .title(translator.text("rules.name"))
        .text(&form.name)
        .build();
    devices_group.add(&name);

    let limit_group = adw::PreferencesGroup::builder()
        .title(translator.text("rules.dataAllowance"))
        .build();
    let allowance = adw::SpinRow::with_range(0.0, 100_000.0, 1.0);
    allowance.set_title(&translator.text("rules.allowanceGb"));
    allowance.set_digits(1);
    allowance.set_value(form.allowance_gb);
    let cycle_names: Vec<String> = CYCLES
        .iter()
        .map(|cycle| match *cycle {
            "daily" => cycle_label(&MeterCycle::Daily, translator),
            "weekly" => cycle_label(&MeterCycle::Weekly { weekday: 1 }, translator),
            "monthly" => cycle_label(&MeterCycle::Monthly { day: 1 }, translator),
            "billing" => translator.text("rules.billingNeedsAccount"),
            "once" => cycle_label(&MeterCycle::Once, translator),
            _ => translator.text("rules.cycle"),
        })
        .collect();
    let cycle_model =
        gtk::StringList::new(&cycle_names.iter().map(String::as_str).collect::<Vec<_>>());
    let cycle = adw::ComboRow::builder()
        .title(translator.text("rules.resets"))
        .model(&cycle_model)
        .selected(cycle_index(&form.cycle))
        .build();
    let cycle_day = adw::SpinRow::with_range(0.0, 365.0, 1.0);
    cycle_day.set_title(&translator.text("rules.resetsOnDay"));
    cycle_day.set_value(f64::from(cycle_number(&form.cycle)));
    let shared = adw::SwitchRow::builder()
        .title(translator.text("rules.shared"))
        .subtitle(translator.text("rules.sharedDetail"))
        .active(form.shared)
        .build();
    limit_group.add(&allowance);
    limit_group.add(&cycle);
    limit_group.add(&cycle_day);
    limit_group.add(&shared);

    let timer_group = adw::PreferencesGroup::builder()
        .title(translator.text("rules.timer"))
        .description(translator.text("rules.timerMax"))
        .build();
    let hours = adw::SpinRow::with_range(0.0, 24.0, 1.0);
    hours.set_title(&translator.text("rules.hours"));
    hours.set_value(f64::from(form.countdown_minutes / 60));
    let minutes = adw::SpinRow::with_range(0.0, 59.0, 5.0);
    minutes.set_title(&translator.text("rules.minutes"));
    minutes.set_value(f64::from(form.countdown_minutes % 60));
    timer_group.add(&hours);
    timer_group.add(&minutes);

    let schedule_group = adw::PreferencesGroup::builder()
        .title(translator.text("rules.hoursHeading"))
        .build();
    let schedule_mode = adw::ComboRow::builder()
        .title(translator.text("rules.whatScheduleDoes"))
        .model(&gtk::StringList::new(&[
            &translator.text("rules.allowDetail"),
            &translator.text("rules.blockDetail"),
        ]))
        .selected(u32::from(form.schedule.mode == ScheduleMode::Block))
        .build();
    schedule_group.add(&schedule_mode);
    let windows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let window_rows: Rc<RefCell<Vec<WindowRow>>> = Rc::new(RefCell::new(Vec::new()));
    let add_window = {
        let windows_box = windows_box.clone();
        let window_rows = window_rows.clone();
        move |window: ScheduleWindow| {
            let row = WindowRow::new(window);
            windows_box.append(&row.root);
            window_rows.borrow_mut().push(row);
        }
    };
    for window in form.schedule.windows.clone() {
        add_window(window);
    }
    let add_button = gtk::Button::with_label(&translator.text("rules.addAnotherTime"));
    add_button.add_css_class("flat");
    add_button.set_halign(gtk::Align::Start);
    add_button.connect_clicked(move |_| {
        add_window(ScheduleWindow {
            weekdays: vec![1, 2, 3, 4, 5],
            start_minute: 16 * 60,
            end_minute: 20 * 60,
            from_date: None,
            to_date: None,
        });
    });
    let schedule_body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    schedule_body.append(&windows_box);
    schedule_body.append(&add_button);

    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    error.add_css_class("error");
    let save = gtk::Button::with_label(&translator.text("rules.saveRule"));
    save.add_css_class("suggested-action");
    save.set_halign(gtk::Align::End);

    let show_kind = {
        let (limit_group, timer_group, schedule_group, schedule_body) = (
            limit_group.clone(),
            timer_group.clone(),
            schedule_group.clone(),
            schedule_body.clone(),
        );
        move |selected: u32| {
            limit_group.set_visible(selected == 0);
            schedule_group.set_visible(selected == 1 || selected == 0);
            schedule_body.set_visible(selected == 1 || selected == 0);
            timer_group.set_visible(selected == 2);
        }
    };
    show_kind(kind.selected());
    kind.connect_selected_notify(move |row| show_kind(row.selected()));

    let kind_group = adw::PreferencesGroup::new();
    kind_group.add(&kind);
    content.append(&kind_group);
    content.append(&devices_group);
    content.append(&limit_group);
    content.append(&timer_group);
    content.append(&schedule_group);
    content.append(&schedule_body);
    content.append(&error);
    content.append(&save);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(640)
        .child(&content)
        .build();

    scroller.add_css_class("dishy-page-scroller");
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroller));
    window.set_content(Some(&toolbar));

    let this = Rc::downgrade(inner);
    let dialog = window.clone();
    save.connect_clicked(move |_| {
        let Some(inner) = this.upgrade() else { return };
        let now_ms = dishy_core::calendar::now_ms();
        let selected_kind = match kind.selected() {
            1 => RuleKind::Schedule,
            2 => RuleKind::Timer,
            _ => RuleKind::Limit,
        };
        let next = RuleForm {
            kind: selected_kind,
            devices: device_checks
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(key, _)| key.clone())
                .collect(),
            name: name.text().to_string(),
            allowance_gb: allowance.value(),
            cycle: if cycle.selected() == cycle_index(&form.cycle)
                && cycle_day.value() as u32 == cycle_number(&form.cycle)
            {
                form.cycle.clone()
            } else {
                cycle_from(cycle.selected(), cycle_day.value() as u32, now_ms)
            },
            shared: shared.is_active(),
            countdown_minutes: (hours.value() as u32) * 60 + minutes.value() as u32,
            schedule: dishy_core::schedule::Schedule {
                mode: if schedule_mode.selected() == 1 {
                    ScheduleMode::Block
                } else {
                    ScheduleMode::Allow
                },
                windows: window_rows
                    .borrow()
                    .iter()
                    .map(WindowRow::value)
                    .filter(|window| !window.weekdays.is_empty() || window.from_date.is_some())
                    .collect(),
            },
            group_id: form.group_id.clone(),
        };
        let snapshot = inner.snapshot.borrow().clone();
        match form_command(
            &next,
            &snapshot,
            &inner.names.borrow(),
            now_ms,
            &inner.translator,
        ) {
            Ok(command) => {
                inner.send(command);
                dialog.close();
            }
            Err(problem) => {
                let key = match problem {
                    FormError::NoDevices => "rules.pickDevices",
                    FormError::NoAllowance => "rules.noLimitYet",
                    FormError::NoCountdown => "rules.timerMax",
                    FormError::NoWindows => "rules.setSchedule",
                };
                error.set_label(&inner.translator.text(key));
                error.set_visible(true);
            }
        }
    });
    window.present();
}
