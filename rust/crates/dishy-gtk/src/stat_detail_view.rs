//! The window a dashboard tile opens: its figures, window chart, latency
//! distribution, router chart, window energy, and the latency Quality view.
//!
//! Widgets only; every figure comes from `stat_detail_model` (spec 0172). The
//! dashboard pushes fresh samples on its own refresh, and summaries arrive
//! from the recorder thread. Opening a detail adds no device request.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use dishy_core::latency_summary::LatencySummary;
use dishy_core::telemetry::TelemetrySample;
use dishy_core::usage::{UsageRange, UsageSummary};
use gtk::cairo;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::chart::ChartCanvas;
use crate::i18n::Translator;
use crate::recorder::RecorderCommand;
use crate::stat_detail_model::{
    DETAIL_WINDOWS, DetailKind, DetailView, HistogramPanel, detail_view, quality_view,
};

/// What the dashboard knows that a detail needs on each refresh.
#[derive(Debug, Clone)]
pub struct DetailInputs {
    pub samples: Arc<Vec<TelemetrySample>>,
    pub current: f64,
    pub now_ms: i64,
    pub power_end_ms: i64,
}

/// The recorder range whose summary matches a detail window, if any.
fn summary_range(window_minutes: u32) -> Option<UsageRange> {
    match window_minutes {
        60 => Some(UsageRange::OneHour),
        360 => Some(UsageRange::SixHours),
        _ => None,
    }
}

#[derive(Clone)]
pub struct StatDetailWindow {
    inner: Rc<Inner>,
}

struct Inner {
    window: adw::Window,
    kind: DetailKind,
    translator: Translator,
    commands: Option<Sender<RecorderCommand>>,
    window_minutes: Cell<u32>,
    inputs: RefCell<Option<DetailInputs>>,
    energy: RefCell<Option<Arc<UsageSummary>>>,
    quality_range: Cell<UsageRange>,
    average: gtk::Label,
    current: gtk::Label,
    chart: ChartCanvas,
    legend: gtk::Box,
    histogram_box: gtk::Box,
    secondary_box: gtk::Box,
    secondary_figures: gtk::Label,
    secondary_chart: ChartCanvas,
    secondary_empty: gtk::Label,
    energy_label: gtk::Label,
    quality_score: gtk::Label,
    quality_figures: gtk::Label,
    quality_collected: gtk::Label,
    quality_bars: gtk::DrawingArea,
    bars: RefCell<Vec<(i64, Option<f64>)>>,
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .build();
    label.add_css_class("caption");
    label.add_css_class("dim-label");
    label
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class("heading");
    label
}

fn figure_text(figures: &[(String, (String, String))]) -> String {
    figures
        .iter()
        .map(|(label, (value, unit))| format!("{label} {value} {unit}"))
        .collect::<Vec<_>>()
        .join("   ·   ")
}

impl StatDetailWindow {
    pub fn open(
        parent: Option<&gtk::Window>,
        kind: DetailKind,
        translator: &Translator,
        commands: Option<Sender<RecorderCommand>>,
        inputs: DetailInputs,
    ) -> Self {
        let preview = detail_view(
            kind,
            &inputs.samples,
            inputs.current,
            DETAIL_WINDOWS[0],
            inputs.now_ms,
            inputs.power_end_ms,
            None,
            translator,
        );
        let window = adw::Window::builder()
            .modal(true)
            .default_width(620)
            .default_height(720)
            .title(&preview.title)
            .build();
        if let Some(parent) = parent {
            window.set_transient_for(Some(parent));
        }

        let average = gtk::Label::new(None);
        average.add_css_class("title-2");
        let current = gtk::Label::new(None);
        current.add_css_class("title-2");
        let figure = |label: &str, value: &gtk::Label| {
            let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
            column.set_hexpand(true);
            column.append(&caption(label));
            column.append(value);
            column
        };
        let figures = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        figures.append(&figure(&translator.text("metrics.average"), &average));
        figures.append(&figure(&translator.text("metrics.current"), &current));

        let windows = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        windows.add_css_class("linked");
        windows.set_halign(gtk::Align::Start);
        windows.update_property(&[gtk::accessible::Property::Label(
            &translator.text("metrics.timeWindow"),
        )]);

        let chart = ChartCanvas::new(220, DETAIL_WINDOWS[0], translator);
        let legend = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        legend.set_halign(gtk::Align::Center);

        let histogram_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let secondary_figures = gtk::Label::builder().xalign(0.0).build();
        let secondary_chart = ChartCanvas::new(180, DETAIL_WINDOWS[0], translator);
        let secondary_empty = caption("");
        let secondary_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let energy_label = gtk::Label::builder().xalign(0.0).wrap(true).build();

        let explainer = gtk::Expander::builder()
            .label(&preview.explainer_title)
            .child(&caption(&preview.explainer))
            .build();

        let live = gtk::Box::new(gtk::Orientation::Vertical, 12);
        live.append(&figures);
        live.append(&windows);
        live.append(chart.widget());
        live.append(&legend);
        live.append(&histogram_box);
        live.append(&secondary_box);
        live.append(&energy_label);
        live.append(&explainer);

        let quality_score = gtk::Label::builder().xalign(0.0).build();
        quality_score.add_css_class("title-1");
        let quality_figures = gtk::Label::builder().xalign(0.0).wrap(true).build();
        let quality_collected = caption("");
        let quality_bars = gtk::DrawingArea::builder()
            .content_height(140)
            .hexpand(true)
            .build();
        let ranges = gtk::StringList::new(
            &UsageRange::ALL
                .iter()
                .map(|range| translator.text(range.label_key()))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let range_dropdown = gtk::DropDown::builder()
            .model(&ranges)
            .halign(gtk::Align::Start)
            .build();
        range_dropdown.update_property(&[gtk::accessible::Property::Label(
            &translator.text("latencyQuality.rangeLabel"),
        )]);
        let quality = gtk::Box::new(gtk::Orientation::Vertical, 10);
        quality.append(&quality_score);
        quality.append(&caption(&translator.text("latencyQuality.scoreLabel")));
        quality.append(&range_dropdown);
        quality.append(&quality_figures);
        quality.append(&quality_collected);
        quality.append(&heading(&translator.text("latencyQuality.p95Heading")));
        quality.append(&quality_bars);
        quality.append(
            &gtk::Expander::builder()
                .label(translator.text("latencyQuality.whatIs"))
                .child(&caption(&translator.text("latencyQuality.explainer")))
                .build(),
        );

        let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        body.set_margin_top(16);
        body.set_margin_bottom(16);
        body.set_margin_start(18);
        body.set_margin_end(18);
        let views = adw::ViewStack::new();
        if kind == DetailKind::Latency {
            views.add_titled(&live, Some("live"), &translator.text("metrics.live"));
            views.add_titled(
                &quality,
                Some("quality"),
                &translator.text("metrics.quality"),
            );
            let switcher = adw::ViewSwitcher::builder()
                .stack(&views)
                .policy(adw::ViewSwitcherPolicy::Wide)
                .halign(gtk::Align::Center)
                .build();
            body.append(&switcher);
            body.append(&views);
        } else {
            body.append(&live);
        }

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));
        window.set_content(Some(&toolbar));

        let inner = Rc::new(Inner {
            window,
            kind,
            translator: translator.clone(),
            commands,
            window_minutes: Cell::new(DETAIL_WINDOWS[0]),
            inputs: RefCell::new(Some(inputs)),
            energy: RefCell::new(None),
            quality_range: Cell::new(UsageRange::OneHour),
            average,
            current,
            chart,
            legend,
            histogram_box,
            secondary_box,
            secondary_figures,
            secondary_chart,
            secondary_empty,
            energy_label,
            quality_score,
            quality_figures,
            quality_collected,
            quality_bars,
            bars: RefCell::new(Vec::new()),
        });

        let mut first: Option<gtk::ToggleButton> = None;
        for minutes in DETAIL_WINDOWS {
            let label = if minutes < 60 {
                format!("{minutes}M")
            } else {
                format!("{}H", minutes / 60)
            };
            let button = gtk::ToggleButton::with_label(&label);
            if let Some(first) = &first {
                button.set_group(Some(first));
            } else {
                button.set_active(true);
                first = Some(button.clone());
            }
            let weak = Rc::downgrade(&inner);
            button.connect_toggled(move |button| {
                if button.is_active()
                    && let Some(inner) = weak.upgrade()
                {
                    inner.window_minutes.set(minutes);
                    inner.energy.borrow_mut().take();
                    inner.request_energy();
                    inner.redraw();
                }
            });
            windows.append(&button);
        }

        if kind == DetailKind::PingSuccess {
            inner
                .secondary_box
                .append(&heading(&translator.text("metrics.routerPingSuccess")));
            inner
                .secondary_box
                .append(&caption(&translator.text("metrics.routerPingNote")));
            inner.secondary_box.append(&inner.secondary_figures);
            inner.secondary_box.append(inner.secondary_chart.widget());
            inner.secondary_box.append(&inner.secondary_empty);
        }

        let weak = Rc::downgrade(&inner);
        inner
            .quality_bars
            .set_draw_func(move |area, cr, width, height| {
                if let Some(inner) = weak.upgrade() {
                    draw_bars(area, cr, width, height, &inner.bars.borrow());
                }
            });
        let weak = Rc::downgrade(&inner);
        range_dropdown.connect_selected_notify(move |dropdown| {
            if let Some(inner) = weak.upgrade()
                && let Some(range) = UsageRange::ALL.get(dropdown.selected() as usize)
            {
                inner.quality_range.set(*range);
                inner.request_quality();
            }
        });

        inner.request_energy();
        inner.request_quality();
        inner.redraw();
        inner.window.present();
        Self { inner }
    }

    pub fn kind(&self) -> DetailKind {
        self.inner.kind
    }

    pub fn is_open(&self) -> bool {
        self.inner.window.is_visible()
    }

    /// Fresh samples from the dashboard's own refresh.
    pub fn update(&self, inputs: DetailInputs) {
        *self.inner.inputs.borrow_mut() = Some(inputs);
        self.inner.redraw();
    }

    pub fn set_energy(&self, summary: Arc<UsageSummary>) {
        if summary_range(self.inner.window_minutes.get()) == Some(summary.range) {
            *self.inner.energy.borrow_mut() = Some(summary);
            self.inner.redraw();
        }
    }

    pub fn set_latency_summary(&self, summary: Arc<LatencySummary>) {
        if summary.range != self.inner.quality_range.get() {
            return;
        }
        let view = quality_view(&summary, &self.inner.translator);
        let score = match view.grade {
            Some(grade) => format!("{}  {grade}", view.score),
            None => view.score.clone(),
        };
        self.inner.quality_score.set_label(&score);
        self.inner
            .quality_figures
            .set_label(&figure_text(&view.figures));
        self.inner.quality_collected.set_label(&view.collected);
        *self.inner.bars.borrow_mut() = view.bars;
        self.inner.quality_bars.queue_draw();
    }
}

impl Inner {
    fn send(&self, command: RecorderCommand) {
        if let Some(sender) = &self.commands {
            let _ = sender.send(command);
        }
    }

    fn request_energy(&self) {
        if self.kind == DetailKind::Power
            && let Some(range) = summary_range(self.window_minutes.get())
        {
            self.send(RecorderCommand::DetailEnergy(range));
        }
    }

    fn request_quality(&self) {
        if self.kind == DetailKind::Latency {
            self.send(RecorderCommand::LatencySummary(self.quality_range.get()));
        }
    }

    fn redraw(&self) {
        let Some(inputs) = self.inputs.borrow().clone() else {
            return;
        };
        let energy = self.energy.borrow().clone();
        let view = detail_view(
            self.kind,
            &inputs.samples,
            inputs.current,
            self.window_minutes.get(),
            inputs.now_ms,
            inputs.power_end_ms,
            energy.as_deref(),
            &self.translator,
        );
        self.show(&view);
    }

    fn show(&self, view: &DetailView) {
        self.average
            .set_label(&format!("{} {}", view.average.0, view.average.1));
        self.current
            .set_label(&format!("{} {}", view.current.0, view.current.1));
        self.chart
            .set_data(view.series.clone(), self.window_minutes.get(), view.end_ms);

        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        self.legend.set_visible(view.series.len() > 1);
        for series in &view.series {
            self.legend.append(&legend_dot(&series.label, series.color));
        }

        while let Some(child) = self.histogram_box.first_child() {
            self.histogram_box.remove(&child);
        }
        self.histogram_box.set_visible(!view.histogram.is_empty());
        if !view.histogram.is_empty() {
            self.histogram_box.append(&heading(
                &self.translator.text("metrics.latencyDistribution"),
            ));
            self.histogram_box
                .append(&caption(&self.translator.text("metrics.selectedWindow")));
            let max = view
                .histogram
                .iter()
                .flat_map(|panel| panel.bins.iter().copied())
                .fold(0.0_f64, f64::max);
            for panel in &view.histogram {
                self.histogram_box
                    .append(&caption(&self.translator.text_with(
                        "metrics.histogramMax",
                        &[("percent", &crate::format::to_fixed(max, 1))],
                    )));
                self.histogram_box.append(&histogram(panel.clone(), max));
            }
        }

        if let Some(secondary) = &view.secondary {
            self.secondary_box.set_visible(true);
            match &secondary.figures {
                Some(figures) => {
                    self.secondary_figures.set_label(&figure_text(figures));
                    self.secondary_figures.set_visible(true);
                    self.secondary_chart.widget().set_visible(true);
                    self.secondary_empty.set_visible(false);
                    self.secondary_chart.set_data(
                        secondary.series.clone(),
                        self.window_minutes.get(),
                        view.end_ms,
                    );
                }
                None => {
                    self.secondary_figures.set_visible(false);
                    self.secondary_chart.widget().set_visible(false);
                    self.secondary_empty.set_label(&secondary.empty_note);
                    self.secondary_empty.set_visible(true);
                }
            }
        } else {
            self.secondary_box.set_visible(false);
        }

        match &view.energy {
            Some((kwh, note)) => {
                self.energy_label.set_label(&format!("{kwh} — {note}"));
                self.energy_label.set_visible(true);
            }
            None => self.energy_label.set_visible(false),
        }
    }
}

fn legend_dot(label: &str, color: (f64, f64, f64)) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dot = gtk::DrawingArea::builder()
        .content_width(9)
        .content_height(9)
        .valign(gtk::Align::Center)
        .build();
    dot.set_draw_func(move |_, cr, width, height| {
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            f64::from(width.min(height)) / 2.0,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill();
    });
    let text = gtk::Label::new(Some(label));
    text.add_css_class("caption-heading");
    row.append(&dot);
    row.append(&text);
    row
}

fn histogram(panel: HistogramPanel, max: f64) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_height(85)
        .hexpand(true)
        .build();
    area.update_property(&[gtk::accessible::Property::Label(&panel.label)]);
    area.set_draw_func(move |_, cr, width, height| {
        let count = panel.bins.len().max(1) as f64;
        let bar_width = f64::from(width) / count;
        cr.set_source_rgba(panel.color.0, panel.color.1, panel.color.2, 0.85);
        for (index, pct) in panel.bins.iter().enumerate() {
            if *pct <= 0.0 || max <= 0.0 {
                continue;
            }
            let bar_height = (pct / max).max(0.02) * f64::from(height);
            cr.rectangle(
                index as f64 * bar_width,
                f64::from(height) - bar_height,
                (bar_width - 1.0).max(1.0),
                bar_height,
            );
        }
        let _ = cr.fill();
    });
    area
}

fn draw_bars(
    area: &gtk::DrawingArea,
    cr: &cairo::Context,
    width: i32,
    height: i32,
    bars: &[(i64, Option<f64>)],
) {
    let max = bars
        .iter()
        .filter_map(|(_, value)| *value)
        .fold(0.0_f64, f64::max);
    if bars.is_empty() || max <= 0.0 {
        return;
    }
    let ink = area.color();
    cr.set_source_rgba(
        f64::from(ink.red()),
        f64::from(ink.green()),
        f64::from(ink.blue()),
        0.8,
    );
    let bar_width = f64::from(width) / bars.len() as f64;
    for (index, (_, value)) in bars.iter().enumerate() {
        let Some(value) = value else { continue };
        let bar_height = (value / max) * f64::from(height);
        cr.rectangle(
            index as f64 * bar_width + 1.0,
            f64::from(height) - bar_height,
            (bar_width - 2.0).max(1.0),
            bar_height,
        );
    }
    let _ = cr.fill();
}
