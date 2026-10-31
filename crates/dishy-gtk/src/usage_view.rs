//! The local data-usage page.
//!
//! The figures and bars come from `dishy-core`'s shared summary; this file only
//! lays them out and asks the one recorder thread for a fresh fold. It never
//! opens a store or touches the router, so the data directory keeps its single
//! writer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Once;
use std::sync::mpsc::Sender;

use chrono::{Local, TimeZone};
use gtk::cairo::Context;
use gtk::prelude::*;

use dishy_core::calendar::now_ms;
use dishy_core::device_kind::classify_device;
use dishy_core::meter::{ClientMonthlyLimit, MonthlyLimitProgress, limit_progress};
use dishy_core::odometer::{ClientTotal, key_of, month_of};
use dishy_core::oui::OuiRegistry;
use dishy_core::usage::{UsageBucket, UsageRange, UsageSummary};

use crate::format::{fixed, format_bytes};
use crate::i18n::Translator;
use crate::recorder::RecorderCommand;

const CSS: &str = r#"
.dishy-usage-page {
  padding: 18px 24px 28px 24px;
}
.dishy-usage-figures {
  margin-bottom: 18px;
}
.dishy-usage-figure {
  padding: 13px 16px;
}
.dishy-usage-figure-label {
  font-size: 13px;
  font-weight: 600;
  opacity: 0.72;
}
.dishy-usage-figure-value {
  font-size: 30px;
  font-weight: 700;
}
.dishy-usage-figure-unit {
  font-size: 14px;
  font-weight: 600;
  opacity: 0.7;
}
.dishy-usage-range {
  margin-bottom: 10px;
}
.dishy-usage-coverage {
  font-size: 12px;
  font-weight: 500;
  opacity: 0.68;
  margin-top: 2px;
}
.dishy-usage-chart {
  margin: 4px 0 10px 0;
}
.dishy-usage-explainer {
  margin-top: 18px;
}
.dishy-usage-explainer-body {
  opacity: 0.72;
}
.dishy-device-section {
  margin-top: 24px;
}
.dishy-device-header {
  margin-bottom: 2px;
}
.dishy-device-title {
  font-size: 16px;
  font-weight: 700;
}
.dishy-device-month {
  font-size: 12px;
  font-weight: 500;
  opacity: 0.68;
  margin-bottom: 8px;
}
.dishy-device-empty {
  font-size: 12px;
  opacity: 0.65;
  padding: 12px 0;
}
.dishy-device-row {
  padding: 8px 0;
  border-top: 1px solid alpha(currentColor, 0.08);
}
.dishy-device-name {
  font-size: 14px;
  font-weight: 600;
}
.dishy-device-sub {
  font-size: 11px;
  opacity: 0.65;
}
.dishy-device-total {
  font-family: monospace;
  font-size: 14px;
  font-weight: 600;
}
.dishy-device-split {
  font-family: monospace;
  font-size: 11px;
  opacity: 0.65;
}
.dishy-device-limit {
  font-size: 11px;
  opacity: 0.72;
}
.dishy-device-limit-reached {
  color: @error_color;
  font-weight: 600;
}
.dishy-device-limit-bar trough {
  min-height: 4px;
}
.dishy-device-limit-bar progress {
  min-height: 4px;
}
"#;

#[derive(Clone)]
pub struct DataUsagePage {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::ScrolledWindow,
    translator: Translator,
    range: Cell<UsageRange>,
    summary: RefCell<Option<UsageSummary>>,
    commands: RefCell<Option<Sender<RecorderCommand>>>,
    down_value: gtk::Label,
    down_unit: gtk::Label,
    up_value: gtk::Label,
    up_unit: gtk::Label,
    total_value: gtk::Label,
    total_unit: gtk::Label,
    coverage: gtk::Label,
    chart: UsageBars,
    device_section: DeviceUsageSection,
    client_totals: RefCell<Vec<ClientTotal>>,
    monthly_limits: RefCell<Vec<ClientMonthlyLimit>>,
    merge_candidates: RefCell<Vec<dishy_core::odometer::MergeCandidate>>,
}

struct DeviceUsageSection {
    container: gtk::Box,
    /// The duplicate-device prompt (spec 0176).
    merge_prompt: gtk::Box,
    month_label: gtk::Label,
    device_list: gtk::Box,
    clear_button: gtk::Button,
    confirm_box: gtk::Box,
    empty_label: gtk::Label,
    error_label: gtk::Label,
}

impl DeviceUsageSection {
    fn new(translator: &Translator) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("dishy-device-section");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("dishy-device-header");

        let title = gtk::Label::builder()
            .label(translator.text("devices.usageTitle"))
            .halign(gtk::Align::Start)
            .build();
        title.add_css_class("dishy-device-title");
        header.append(&title);

        let info_icon = gtk::Image::from_icon_name("help-about-symbolic");
        info_icon.set_tooltip_text(Some(&translator.text("devices.usageTip")));
        info_icon.set_opacity(0.6);
        header.append(&info_icon);

        let clear_button = gtk::Button::builder()
            .label(translator.text("devices.clearAll"))
            .halign(gtk::Align::End)
            .hexpand(true)
            .has_frame(false)
            .build();
        clear_button.add_css_class("flat");
        header.append(&clear_button);

        let confirm_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        confirm_box.set_halign(gtk::Align::End);
        confirm_box.set_hexpand(true);
        confirm_box.set_visible(false);

        let confirm_yes = gtk::Button::builder()
            .label(translator.text("devices.clearAllIncludingLimits"))
            .has_frame(false)
            .build();
        confirm_yes.add_css_class("destructive-action");
        confirm_box.append(&confirm_yes);

        let confirm_cancel = gtk::Button::builder()
            .label(translator.text("common.cancel"))
            .has_frame(false)
            .build();
        confirm_cancel.add_css_class("flat");
        confirm_box.append(&confirm_cancel);

        header.append(&confirm_box);
        container.append(&header);

        let month_label = gtk::Label::builder().halign(gtk::Align::Start).build();
        month_label.add_css_class("dishy-device-month");
        container.append(&month_label);

        let empty_label = gtk::Label::builder()
            .label(translator.text("devices.usageUnavailable"))
            .halign(gtk::Align::Start)
            .visible(false)
            .build();
        empty_label.add_css_class("dishy-device-empty");
        container.append(&empty_label);

        let error_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .wrap(true)
            .visible(false)
            .build();
        error_label.add_css_class("error");
        container.append(&error_label);

        let merge_prompt = gtk::Box::new(gtk::Orientation::Vertical, 8);
        merge_prompt.add_css_class("card");
        merge_prompt.set_visible(false);
        container.append(&merge_prompt);
        let device_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&device_list);

        Self {
            container,
            merge_prompt,
            month_label,
            device_list,
            clear_button,
            confirm_box,
            empty_label,
            error_label,
        }
    }
}

impl DataUsagePage {
    pub fn new(translator: &Translator) -> Self {
        install_css();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("dishy-usage-page");

        let (figures, down_value, down_unit, up_value, up_unit, total_value, total_unit) =
            figure_row(translator);
        body.append(&figures);

        let range = Cell::new(UsageRange::Today);
        let range_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        range_row.add_css_class("dishy-usage-range");
        range_row.set_halign(gtk::Align::Center);
        let range_tooltip = translator.text("dataUsage.rangeLabel");
        range_row.set_tooltip_text(Some(&range_tooltip));
        let (control, range_buttons) = range_control(translator, range.get());
        range_row.append(&control);
        body.append(&range_row);

        let chart = UsageBars::new(translator);
        body.append(chart.widget());

        let coverage = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .wrap(true)
            .visible(false)
            .build();
        coverage.add_css_class("dishy-usage-coverage");
        body.append(&coverage);

        let explainer = gtk::Expander::new(Some(&translator.text("dataUsage.howMeasured")));
        explainer.add_css_class("dishy-usage-explainer");
        let explainer_body = gtk::Label::builder()
            .label(translator.text("dataUsage.howMeasuredBody"))
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .build();
        explainer_body.add_css_class("dishy-usage-explainer-body");
        explainer.set_child(Some(&explainer_body));
        body.append(&explainer);

        let device_section = DeviceUsageSection::new(translator);
        body.append(&device_section.container);

        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();

        root.add_css_class("dishy-page-scroller");

        let inner = Rc::new(Inner {
            root,
            translator: translator.clone(),
            range,
            summary: RefCell::new(None),
            commands: RefCell::new(None),
            merge_candidates: RefCell::new(Vec::new()),
            down_value,
            down_unit,
            up_value,
            up_unit,
            total_value,
            total_unit,
            coverage,
            chart,
            device_section,
            client_totals: RefCell::new(Vec::new()),
            monthly_limits: RefCell::new(Vec::new()),
        });

        let weak = Rc::downgrade(&inner);
        inner.device_section.clear_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.device_section.clear_button.set_visible(false);
                inner.device_section.confirm_box.set_visible(true);
            }
        });

        let weak = Rc::downgrade(&inner);
        if let Some(cancel_btn) = inner
            .device_section
            .confirm_box
            .last_child()
            .and_then(|w| w.downcast::<gtk::Button>().ok())
        {
            cancel_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.device_section.confirm_box.set_visible(false);
                    inner.device_section.clear_button.set_visible(true);
                }
            });
        }

        let weak = Rc::downgrade(&inner);
        if let Some(confirm_btn) = inner
            .device_section
            .confirm_box
            .first_child()
            .and_then(|w| w.downcast::<gtk::Button>().ok())
        {
            confirm_btn.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.device_section.confirm_box.set_visible(false);
                    inner.device_section.clear_button.set_visible(true);
                    if let Some(sender) = inner.commands.borrow().as_ref() {
                        let _ = sender.send(RecorderCommand::ClearClientTotals);
                    }
                }
            });
        }

        let weak = Rc::downgrade(&inner);
        for (index, button) in range_buttons.iter().enumerate() {
            let weak = weak.clone();
            button.connect_toggled(move |button| {
                if !button.is_active() {
                    return;
                }
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                inner.range.set(UsageRange::ALL[index]);
                inner.request();
            });
        }

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
        gtk::glib::timeout_add_seconds_local(30, move || {
            if let Some(inner) = weak.upgrade()
                && inner.root.is_mapped()
            {
                inner.request();
            }
            gtk::glib::ControlFlow::Continue
        });

        let actions = gtk::gio::SimpleActionGroup::new();
        let answer =
            gtk::gio::SimpleAction::new("answer-merge", Some(gtk::glib::VariantTy::BOOLEAN));
        let weak = Rc::downgrade(&inner);
        answer.connect_activate(move |_, value| {
            if let (Some(inner), Some(same)) =
                (weak.upgrade(), value.and_then(|value| value.get::<bool>()))
            {
                inner.answer_merge(same);
            }
        });
        actions.add_action(&answer);
        inner.root.insert_action_group("usage", Some(&actions));
        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.root.upcast_ref()
    }

    /// Give the page the recorder handle. Until then it shows dashes and does
    /// not queue summaries.
    pub fn set_command_sender(&self, sender: Option<Sender<RecorderCommand>>) {
        *self.inner.commands.borrow_mut() = sender;
        if self.inner.root.is_mapped() {
            self.inner.set_active(true);
        } else {
            self.inner.request();
        }
    }

    pub fn set_summary(&self, summary: UsageSummary) {
        if summary.range != self.inner.range.get() {
            return;
        }
        self.inner.apply(&summary);
        *self.inner.summary.borrow_mut() = Some(summary);
    }

    pub fn set_client_totals(&self, totals: Vec<ClientTotal>) {
        *self.inner.client_totals.borrow_mut() = totals;
        self.inner.rebuild_device_list();
        self.inner.rebuild_merge_prompt();
    }

    pub fn set_merge_candidates(&self, candidates: Vec<dishy_core::odometer::MergeCandidate>) {
        *self.inner.merge_candidates.borrow_mut() = candidates;
        self.inner.rebuild_merge_prompt();
    }

    pub fn merge_prompt_visible(&self) -> bool {
        self.inner.device_section.merge_prompt.is_visible()
    }

    /// Answer the shown proposal, as its buttons do.
    pub fn answer_merge(&self, same: bool) {
        self.inner.answer_merge(same);
    }

    pub fn set_client_monthly_limits(&self, limits: Vec<ClientMonthlyLimit>) {
        *self.inner.monthly_limits.borrow_mut() = limits;
        self.inner.rebuild_device_list();
    }

    pub fn set_monthly_limit_error(&self, error: Option<String>) {
        if let Some(error) = error {
            let message = self
                .inner
                .translator
                .text_with("common.failed", &[("message", &error)]);
            self.inner.device_section.error_label.set_label(&message);
            self.inner.device_section.error_label.set_visible(true);
        } else {
            self.inner.device_section.error_label.set_visible(false);
        }
    }

    pub fn device_count(&self) -> usize {
        self.inner.client_totals.borrow().len()
    }

    /// The range currently selected, exposed for the shell's tests.
    pub fn range(&self) -> UsageRange {
        self.inner.range.get()
    }

    /// Whether the chart is drawing buckets rather than the empty state.
    pub fn shows_data(&self) -> bool {
        self.inner.chart.stack.visible_child_name().as_deref() == Some("chart")
    }
}

impl Inner {
    // SPEC-USAGE-004: summaries come from the recorder thread, never a direct
    // store read from this thread.
    fn request(&self) {
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(RecorderCommand::Usage(self.range.get()));
            let _ = sender.send(RecorderCommand::GetClientTotals);
        }
    }

    fn set_active(&self, active: bool) {
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(RecorderCommand::DeviceUsageDemand(active));
            if active {
                self.request();
            }
        }
    }

    fn apply(&self, summary: &UsageSummary) {
        let (down, down_unit) = format_gigabytes(summary.total_down_gb);
        let (up, up_unit) = format_gigabytes(summary.total_up_gb);
        let (total, total_unit) = format_gigabytes(summary.total_down_gb + summary.total_up_gb);
        self.down_value.set_label(&down);
        self.down_unit.set_label(down_unit);
        self.up_value.set_label(&up);
        self.up_unit.set_label(up_unit);
        self.total_value.set_label(&total);
        self.total_unit.set_label(total_unit);

        let percent = (summary.coverage.fraction * 100.0).round().max(0.0) as i64;
        let key = if percent < 95 {
            "dataUsage.collectedPartial"
        } else {
            "dataUsage.collected"
        };
        self.coverage.set_label(
            &self
                .translator
                .text_with(key, &[("percent", &percent.to_string())]),
        );
        self.coverage.set_visible(true);
        self.chart.set_buckets(summary.buckets.clone());
    }

    // SPEC-ODO-006: device list rendering, sorting, and row actions.
    fn answer_merge(&self, same: bool) {
        let Some(candidate) = self.merge_candidates.borrow().first().cloned() else {
            return;
        };
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(if same {
                RecorderCommand::MergeClients {
                    from: candidate.from_key.clone(),
                    to: candidate.to_key.clone(),
                }
            } else {
                RecorderCommand::RejectMerge {
                    a: candidate.from_key.clone(),
                    b: candidate.to_key.clone(),
                }
            });
        }
        self.merge_candidates.borrow_mut().remove(0);
        self.rebuild_merge_prompt();
    }

    fn rebuild_merge_prompt(&self) {
        let host = &self.device_section.merge_prompt;
        while let Some(child) = host.first_child() {
            host.remove(&child);
        }
        let prompt = crate::merge_model::merge_prompt(
            &self.merge_candidates.borrow(),
            &self.client_totals.borrow(),
            dishy_core::calendar::now_ms(),
            &self.translator,
        );
        let Some(prompt) = prompt else {
            host.set_visible(false);
            return;
        };
        host.set_margin_top(8);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::builder()
            .label(&prompt.title)
            .xalign(0.0)
            .hexpand(true)
            .build();
        title.add_css_class("heading");
        header.append(&title);
        if let Some(more) = &prompt.more {
            let label = gtk::Label::new(Some(more));
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            header.append(&label);
        }
        let explanation = gtk::Label::builder().xalign(0.0).wrap(true).build();
        explanation.set_markup(&prompt.explanation);
        let cards = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for (index, card) in [&prompt.older, &prompt.newer].into_iter().enumerate() {
            if index == 1 {
                cards.append(&gtk::Label::new(Some("→")));
            }
            let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
            column.set_hexpand(true);
            let name = gtk::Label::builder().label(&card.name).xalign(0.0).build();
            name.add_css_class("heading");
            column.append(&name);
            for line in [&card.detail, &card.router_id] {
                let label = gtk::Label::builder().label(line).xalign(0.0).build();
                label.add_css_class("caption");
                label.add_css_class("dim-label");
                column.append(&label);
            }
            cards.append(&column);
        }
        let outcome = gtk::Label::builder()
            .label(&prompt.outcome)
            .xalign(0.0)
            .wrap(true)
            .build();
        let same = gtk::Button::with_label(&prompt.same);
        same.add_css_class("suggested-action");
        let different = gtk::Button::with_label(&prompt.different);
        different.add_css_class("flat");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.append(&same);
        buttons.append(&different);
        for child in [
            header.upcast_ref::<gtk::Widget>(),
            explanation.upcast_ref(),
            cards.upcast_ref(),
            outcome.upcast_ref(),
            buttons.upcast_ref(),
        ] {
            child.set_margin_start(12);
            child.set_margin_end(12);
            host.append(child);
        }
        buttons.set_margin_bottom(12);
        header.set_margin_top(12);
        let root = self.root.clone();
        let answer = move |same: bool| {
            // Route through the page so the next proposal shows.
            root.activate_action("usage.answer-merge", Some(&same.to_variant()))
                .ok();
        };
        let answer_same = answer.clone();
        same.connect_clicked(move |_| answer_same(true));
        different.connect_clicked(move |_| answer(false));
        host.set_visible(true);
    }

    fn rebuild_device_list(&self) {
        while let Some(child) = self.device_section.device_list.first_child() {
            self.device_section.device_list.remove(&child);
        }

        let now = now_ms();
        let dt = Local::now();
        let month_text = dt.format("%B %Y").to_string();
        self.device_section.month_label.set_label(&month_text);

        let totals = self.client_totals.borrow();
        let mut sorted: Vec<ClientTotal> = totals.clone();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.rx_bytes.saturating_add(a.tx_bytes)));

        if sorted.is_empty() {
            self.device_section.empty_label.set_visible(true);
            self.device_section.clear_button.set_sensitive(false);
            return;
        }

        self.device_section.empty_label.set_visible(false);
        self.device_section.clear_button.set_sensitive(true);

        let registry = OuiRegistry::bundled();

        for total in sorted {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("dishy-device-row");

            let vendor_opt = registry.vendor_for_mac(&total.mac_address);
            let vendor_name = vendor_opt.map(|v| {
                if v == "Private" {
                    self.translator.text("devices.privateVendor")
                } else {
                    v.to_string()
                }
            });

            let display_name = total
                .name
                .clone()
                .or_else(|| vendor_name.clone())
                .unwrap_or_else(|| total.mac_address.clone());

            let kind = classify_device(Some(&display_name));
            let icon = gtk::Image::from_icon_name(kind.icon_name());
            icon.set_icon_size(gtk::IconSize::Large);
            icon.set_opacity(0.7);
            row.append(&icon);

            let text_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
            text_box.set_hexpand(true);

            let name_label = gtk::Label::builder()
                .label(&display_name)
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            name_label.add_css_class("dishy-device-name");
            text_box.append(&name_label);

            let mut subparts = Vec::new();
            if let Some(ref v) = vendor_name
                && v != &display_name
            {
                subparts.push(v.clone());
            }
            if month_of(total.since_ms, false) != month_of(now, false)
                && let Some(date) = Local.timestamp_millis_opt(total.since_ms).single()
            {
                subparts.push(date.format("%b").to_string());
            }
            subparts.push(format_seen_time(total.last_seen_ms, now, &self.translator));

            let sub_label = gtk::Label::builder()
                .label(subparts.join(" \u{00b7} "))
                .halign(gtk::Align::Start)
                .build();
            sub_label.add_css_class("dishy-device-sub");
            text_box.append(&sub_label);

            let key = key_of(total.client_id, &total.mac_address);
            let limit = self
                .monthly_limits
                .borrow()
                .iter()
                .find(|limit| limit.client_key == key)
                .cloned();
            if let Some(limit) = &limit {
                let progress = limit_progress(limit, total.rx_bytes, total.tx_bytes);
                let progress_label = limit_progress_label(&self.translator, progress);
                let meter_label = gtk::Label::builder()
                    .label(&progress_label)
                    .halign(gtk::Align::Start)
                    .build();
                meter_label.add_css_class("dishy-device-limit");
                if progress.reached {
                    meter_label.add_css_class("dishy-device-limit-reached");
                }
                meter_label.set_widget_name("monthly-limit-state");
                text_box.append(&meter_label);

                let bar = gtk::ProgressBar::builder()
                    .fraction(
                        (progress.used_bytes as f64 / progress.limit_bytes as f64).clamp(0.0, 1.0),
                    )
                    .hexpand(true)
                    .build();
                bar.add_css_class("dishy-device-limit-bar");
                bar.set_widget_name("monthly-limit-progress");
                text_box.append(&bar);
            }

            row.append(&text_box);

            let usage_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
            usage_box.set_halign(gtk::Align::End);

            let total_bytes = total.rx_bytes.saturating_add(total.tx_bytes);
            let total_label = gtk::Label::builder()
                .label(format_bytes(total_bytes))
                .halign(gtk::Align::End)
                .build();
            total_label.add_css_class("dishy-device-total");
            usage_box.append(&total_label);

            let split_str = format!(
                "{} \u{2193} \u{00b7} {} \u{2191}",
                format_bytes(total.rx_bytes),
                format_bytes(total.tx_bytes)
            );
            let split_label = gtk::Label::builder()
                .label(&split_str)
                .halign(gtk::Align::End)
                .build();
            split_label.add_css_class("dishy-device-split");
            usage_box.append(&split_label);

            row.append(&usage_box);

            let actions_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            let limit_button = gtk::Button::from_icon_name("speedometer-symbolic");
            limit_button.add_css_class("flat");
            limit_button.set_widget_name("monthly-limit-edit");
            let limit_action_key = if limit.is_some() {
                "devices.editMonthlyLimit"
            } else {
                "devices.setMonthlyLimit"
            };
            limit_button.set_tooltip_text(Some(
                &self
                    .translator
                    .text_with(limit_action_key, &[("name", &display_name)]),
            ));
            let dialog = monthly_limit_dialog(
                &self.root,
                &self.translator,
                &display_name,
                &key,
                limit.as_ref(),
                self.commands.borrow().clone(),
                self.device_section.error_label.clone(),
            );
            let dialog_clone = dialog.clone();
            limit_button.connect_clicked(move |_| dialog_clone.present());
            actions_box.append(&limit_button);

            let reset_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
            reset_btn.add_css_class("flat");
            reset_btn.set_tooltip_text(Some(
                &self
                    .translator
                    .text_with("devices.resetUsageFor", &[("name", &display_name)]),
            ));
            let commands_sender = self.commands.borrow().clone();
            let reset_key = key.clone();
            reset_btn.connect_clicked(move |_| {
                if let Some(ref sender) = commands_sender {
                    let _ = sender.send(RecorderCommand::ResetClientTotal(reset_key.clone()));
                }
            });
            actions_box.append(&reset_btn);

            let delete_btn = gtk::Button::from_icon_name("edit-delete-symbolic");
            delete_btn.add_css_class("flat");
            delete_btn.add_css_class("destructive-action");
            delete_btn.set_tooltip_text(Some(&self.translator.text_with(
                if limit.is_some() {
                    "devices.deleteUsageAndLimitFor"
                } else {
                    "devices.deleteUsageFor"
                },
                &[("name", &display_name)],
            )));
            let commands_sender = self.commands.borrow().clone();
            let delete_key = key.clone();
            delete_btn.connect_clicked(move |_| {
                if let Some(ref sender) = commands_sender {
                    let _ = sender.send(RecorderCommand::RemoveClientTotal(delete_key.clone()));
                }
            });
            actions_box.append(&delete_btn);

            row.append(&actions_box);
            self.device_section.device_list.append(&row);
        }
    }
}

fn monthly_limit_bytes(raw_gigabytes: &str) -> Option<u64> {
    raw_gigabytes
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|gigabytes| *gigabytes > 0)?
        .checked_mul(1_000_000_000)
}

fn limit_progress_label(translator: &Translator, progress: MonthlyLimitProgress) -> String {
    let key = if progress.reached {
        "devices.monthlyLimitReached"
    } else {
        "devices.monthlyLimitProgress"
    };
    translator.text_with(
        key,
        &[
            ("used", &format_bytes(progress.used_bytes)),
            ("limit", &format_bytes(progress.limit_bytes)),
        ],
    )
}

fn monthly_limit_dialog(
    root: &gtk::ScrolledWindow,
    translator: &Translator,
    device_name: &str,
    client_key: &str,
    existing: Option<&ClientMonthlyLimit>,
    commands: Option<Sender<RecorderCommand>>,
    error_label: gtk::Label,
) -> gtk::Window {
    let dialog = gtk::Window::builder()
        .title(translator.text_with("devices.monthlyLimitTitle", &[("name", device_name)]))
        .modal(true)
        .default_width(360)
        .build();
    dialog.set_widget_name("monthly-limit-dialog");
    if let Some(parent) = root
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok())
    {
        dialog.set_transient_for(Some(&parent));
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let prompt = gtk::Label::builder()
        .label(translator.text("devices.monthlyLimitPrompt"))
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();
    content.append(&prompt);

    let entry = gtk::Entry::builder()
        .placeholder_text(translator.text("devices.monthlyLimitPlaceholder"))
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    entry.set_widget_name("monthly-limit-gb");
    if let Some(limit) = existing {
        entry.set_text(&(limit.limit_bytes / 1_000_000_000).to_string());
    }
    content.append(&entry);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(&translator.text("common.cancel"));
    let save = gtk::Button::with_label(&translator.text("common.save"));
    save.set_widget_name("monthly-limit-save");
    save.set_sensitive(monthly_limit_bytes(&entry.text()).is_some());
    buttons.append(&cancel);
    buttons.append(&save);
    if let Some(existing) = existing {
        let remove = gtk::Button::with_label(&translator.text("devices.removeMonthlyLimit"));
        remove.add_css_class("destructive-action");
        remove.set_widget_name("monthly-limit-remove");
        buttons.prepend(&remove);
        let sender = commands.clone();
        let client_key = existing.client_key.clone();
        let error_label = error_label.clone();
        let translator = translator.clone();
        let dialog = dialog.clone();
        remove.connect_clicked(move |_| match sender.as_ref() {
            Some(sender)
                if sender
                    .send(RecorderCommand::RemoveClientMonthlyLimit(
                        client_key.clone(),
                    ))
                    .is_ok() =>
            {
                error_label.set_visible(false);
                dialog.close();
            }
            Some(_) | None => {
                error_label.set_label(&translator.text("devices.monthlyLimitUnavailable"));
                error_label.set_visible(true);
            }
        });
    }
    content.append(&buttons);
    dialog.set_child(Some(&content));

    let weak_save = save.downgrade();
    entry.connect_changed(move |entry| {
        if let Some(save) = weak_save.upgrade() {
            save.set_sensitive(monthly_limit_bytes(&entry.text()).is_some());
        }
    });
    let dialog_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_cancel.close());

    let sender = commands;
    let client_key = client_key.to_owned();
    let error_label = error_label.clone();
    let translator = translator.clone();
    let dialog_save = dialog.clone();
    save.connect_clicked(move |_| {
        let Some(limit_bytes) = monthly_limit_bytes(&entry.text()) else {
            error_label.set_label(&translator.text("devices.monthlyLimitRefused"));
            error_label.set_visible(true);
            return;
        };
        let Some(sender) = sender.as_ref() else {
            error_label.set_label(&translator.text("devices.monthlyLimitUnavailable"));
            error_label.set_visible(true);
            return;
        };
        if sender
            .send(RecorderCommand::SetClientMonthlyLimit {
                client_key: client_key.clone(),
                limit_bytes,
            })
            .is_ok()
        {
            error_label.set_visible(false);
            dialog_save.close();
        } else {
            error_label.set_label(&translator.text("devices.monthlyLimitUnavailable"));
            error_label.set_visible(true);
        }
    });

    dialog
}

fn format_seen_time(last_seen_ms: i64, now_ms: i64, translator: &Translator) -> String {
    let diff_ms = now_ms - last_seen_ms;
    if diff_ms < 120_000 {
        return translator.text("devices.activeNow");
    }
    let seconds = (diff_ms / 1000).max(0);
    if seconds < 60 {
        return translator.text("time.justNow");
    }
    let minutes = (seconds + 30) / 60;
    if minutes < 60 {
        return translator.text_with("time.minutesAgo", &[("count", &minutes.to_string())]);
    }
    let hours = (minutes + 30) / 60;
    if hours < 24 {
        let key = if hours == 1 {
            "time.hoursAgo_one"
        } else {
            "time.hoursAgo_other"
        };
        return translator.text_with(key, &[("count", &hours.to_string())]);
    }
    let days = (hours + 12) / 24;
    let key = if days == 1 {
        "time.daysAgo_one"
    } else {
        "time.daysAgo_other"
    };
    translator.text_with(key, &[("count", &days.to_string())])
}

fn figure_row(
    translator: &Translator,
) -> (
    gtk::Box,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
    gtk::Label,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_homogeneous(true);
    row.add_css_class("dishy-usage-figures");
    let down = figure(translator, "dataUsage.downloadFigure");
    let up = figure(translator, "dataUsage.uploadFigure");
    let total = figure(translator, "dataUsage.total");
    row.append(&down.root);
    row.append(&up.root);
    row.append(&total.root);
    (
        row,
        down.value,
        down.unit,
        up.value,
        up.unit,
        total.value,
        total.unit,
    )
}

struct Figure {
    root: gtk::Box,
    value: gtk::Label,
    unit: gtk::Label,
}

fn figure(translator: &Translator, key: &str) -> Figure {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.add_css_class("dishy-tile");
    root.add_css_class("dishy-usage-figure");
    let label = gtk::Label::builder()
        .label(translator.text(key))
        .halign(gtk::Align::Start)
        .build();
    label.add_css_class("dishy-usage-figure-label");
    let value_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let value = gtk::Label::builder()
        .label("—")
        .halign(gtk::Align::Start)
        .build();
    value.add_css_class("dishy-usage-figure-value");
    let unit = gtk::Label::builder()
        .label("GB")
        .valign(gtk::Align::End)
        .build();
    unit.add_css_class("dishy-usage-figure-unit");
    value_row.append(&value);
    value_row.append(&unit);
    root.append(&label);
    root.append(&value_row);
    Figure { root, value, unit }
}

fn range_control(
    translator: &Translator,
    selected: UsageRange,
) -> (gtk::Box, Vec<gtk::ToggleButton>) {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("linked");
    root.set_widget_name("usage-range-control");
    root.set_halign(gtk::Align::Center);
    let mut first: Option<gtk::ToggleButton> = None;
    let mut buttons = Vec::with_capacity(UsageRange::ALL.len());
    for range in UsageRange::ALL {
        let button = gtk::ToggleButton::with_label(&translator.text(range.label_key()));
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.set_active(range == selected);
        root.append(&button);
        buttons.push(button);
    }
    (root, buttons)
}

#[derive(Clone)]
struct UsageBars {
    stack: gtk::Stack,
    area: gtk::DrawingArea,
    buckets: Rc<RefCell<Vec<UsageBucket>>>,
}

impl UsageBars {
    fn new(translator: &Translator) -> Self {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(160)
            .hexpand(true)
            .build();
        let area = gtk::DrawingArea::builder()
            .content_height(190)
            .hexpand(true)
            .build();
        area.add_css_class("dishy-usage-chart");
        let empty = gtk::Label::builder()
            .label(translator.text("metrics.noData"))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        empty.add_css_class("dishy-chart-empty");
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&area, Some("chart"));
        stack.set_visible_child_name("empty");

        let buckets = Rc::new(RefCell::new(Vec::new()));
        let weak = Rc::downgrade(&buckets);
        area.set_draw_func(move |_, cr, width, height| {
            if let Some(buckets) = weak.upgrade() {
                draw_bars(&buckets.borrow(), cr, width, height);
            }
        });
        Self {
            stack,
            area,
            buckets,
        }
    }

    fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    fn set_buckets(&self, buckets: Vec<UsageBucket>) {
        let has_data = buckets
            .iter()
            .any(|bucket| bucket.down_gb.is_some() || bucket.up_gb.is_some());
        *self.buckets.borrow_mut() = buckets;
        self.stack
            .set_visible_child_name(if has_data { "chart" } else { "empty" });
        self.area.queue_draw();
    }
}

fn draw_bars(buckets: &[UsageBucket], cr: &Context, width: i32, height: i32) {
    let width = width as f64;
    let height = height as f64;
    if width <= 0.0 || height <= 0.0 || buckets.is_empty() {
        return;
    }
    const LEFT: f64 = 48.0;
    const TOP: f64 = 8.0;
    const BOTTOM: f64 = 8.0;
    let plot_width = (width - LEFT).max(1.0);
    let plot_height = (height - TOP - BOTTOM).max(1.0);
    let max = buckets
        .iter()
        .filter_map(|bucket| match (bucket.down_gb, bucket.up_gb) {
            (Some(down), Some(up)) => Some(down + up),
            _ => None,
        })
        .fold(0.0_f64, f64::max)
        .max(1e-9);

    cr.set_line_width(1.0);
    cr.set_source_rgba(0.52, 0.52, 0.56, 0.16);
    for line in 0..=4 {
        let y = TOP + plot_height * line as f64 / 4.0;
        cr.move_to(LEFT, y.round() + 0.5);
        cr.line_to(width, y.round() + 0.5);
        let _ = cr.stroke();
    }

    cr.set_source_rgba(0.55, 0.55, 0.6, 0.75);
    cr.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    cr.set_font_size(10.0);
    let (label, _) = format_gigabytes(max);
    cr.move_to(0.0, TOP + 10.0);
    let _ = cr.show_text(&label);

    let slot = plot_width / buckets.len() as f64;
    let bar_width = (slot * 0.64).max(1.0);
    for (index, bucket) in buckets.iter().enumerate() {
        let x = LEFT + slot * index as f64 + (slot - bar_width) / 2.0;
        let (Some(down), Some(up)) = (bucket.down_gb, bucket.up_gb) else {
            // An empty slot is marked as a hole, never drawn as zero traffic.
            cr.set_source_rgba(0.52, 0.52, 0.56, 0.06);
            cr.rectangle(x, TOP, bar_width, plot_height);
            let _ = cr.fill();
            continue;
        };
        let total = down + up;
        if total <= 0.0 {
            continue;
        }
        let bar_height = (total / max) * plot_height;
        let down_height = (down / total) * bar_height;
        let up_height = bar_height - down_height;
        let base = TOP + plot_height;
        cr.set_source_rgba(0.48, 0.74, 0.78, 0.9);
        cr.rectangle(x, base - down_height, bar_width, down_height);
        let _ = cr.fill();
        cr.set_source_rgba(0.94, 0.62, 0.32, 0.9);
        cr.rectangle(x, base - bar_height, bar_width, up_height);
        let _ = cr.fill();
    }
}

/// Gigabytes to the value/unit pair the TypeScript formatter uses.
pub fn format_gigabytes(gigabytes: f64) -> (String, &'static str) {
    let gigabytes = if gigabytes.is_finite() {
        gigabytes.max(0.0)
    } else {
        0.0
    };
    let value = if gigabytes >= 100.0 {
        fixed(gigabytes, 0)
    } else if gigabytes >= 1.0 {
        fixed(gigabytes, 1)
    } else {
        fixed(gigabytes, 2)
    };
    if value.parse::<f64>().unwrap_or(0.0) < 1000.0 {
        (value, "GB")
    } else {
        (fixed(gigabytes / 1000.0, 2), "TB")
    }
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

    // SPEC-USAGE-001: the figure formatter matches `formatGigabytes`.
    #[test]
    fn gigabytes_choose_a_readable_value_and_unit() {
        assert_eq!(format_gigabytes(0.0), ("0".into(), "GB"));
        assert_eq!(format_gigabytes(0.004), ("0".into(), "GB"));
        assert_eq!(format_gigabytes(0.126), ("0.13".into(), "GB"));
        assert_eq!(format_gigabytes(1.25), ("1.3".into(), "GB"));
        assert_eq!(format_gigabytes(99.94), ("99.9".into(), "GB"));
        assert_eq!(format_gigabytes(123.4), ("123".into(), "GB"));
        assert_eq!(format_gigabytes(1_234.0), ("1.23".into(), "TB"));
        assert_eq!(format_gigabytes(f64::NAN), ("0".into(), "GB"));
    }

    // SPEC-ODO-006: relative time formatting for device last seen.
    #[test]
    fn relative_time_formatting_matches_ranges() {
        let translator = Translator::new(crate::i18n::Locale::En);
        let now = 1_000_000;
        assert_eq!(
            format_seen_time(now - 30_000, now, &translator),
            "Active now"
        );
        assert_eq!(
            format_seen_time(now - 150_000, now, &translator),
            "3 min ago"
        );
        assert_eq!(
            format_seen_time(now - 7_200_000, now, &translator),
            "2 hours ago"
        );
    }
}
