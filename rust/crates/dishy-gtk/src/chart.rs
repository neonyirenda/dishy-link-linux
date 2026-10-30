//! Native chart canvases for the dashboard.
//!
//! GTK4's `DrawingArea` plus Cairo, not a charting dependency and certainly not
//! a web view. The view model has already cut and bucketed the samples; this
//! file only maps them to pixels and breaks the line where the data has a gap.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo::{Context, LineCap, LineJoin};
use gtk::prelude::*;

use crate::dashboard::ChartSeries;
use crate::i18n::Translator;

const GRID_LINES: usize = 4;
const SERIES_WIDTH: f64 = 2.0;
const SPARKLINE_WIDTH: f64 = 1.5;

#[derive(Clone)]
pub struct ChartCanvas {
    inner: Rc<ChartInner>,
}

struct ChartInner {
    stack: gtk::Stack,
    area: gtk::DrawingArea,
    series: RefCell<Vec<ChartSeries>>,
    window_minutes: Cell<u32>,
    end_ms: Cell<i64>,
}

impl ChartCanvas {
    pub fn new(height: i32, window_minutes: u32, translator: &Translator) -> Self {
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(160)
            .vexpand(true)
            .hexpand(true)
            .build();
        let area = gtk::DrawingArea::builder()
            .content_height(height)
            .hexpand(true)
            .vexpand(true)
            .build();
        let empty = gtk::Label::builder()
            .label(translator.text("metrics.waitingForDish"))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .wrap(true)
            .build();
        empty.add_css_class("dishy-chart-empty");
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&area, Some("chart"));
        stack.set_visible_child_name("empty");

        let inner = Rc::new(ChartInner {
            stack,
            area,
            series: RefCell::new(Vec::new()),
            window_minutes: Cell::new(window_minutes),
            end_ms: Cell::new(0),
        });

        let weak = Rc::downgrade(&inner);
        inner.area.set_draw_func(move |_, cr, width, height| {
            if let Some(inner) = weak.upgrade() {
                draw_chart(&inner, cr, width, height);
            }
        });

        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.inner.stack
    }

    pub fn set_data(&self, series: Vec<ChartSeries>, window_minutes: u32, end_ms: i64) {
        let has_points = series.iter().any(|series| !series.is_empty());
        *self.inner.series.borrow_mut() = series;
        self.inner.window_minutes.set(window_minutes);
        self.inner.end_ms.set(end_ms);
        self.inner
            .stack
            .set_visible_child_name(if has_points { "chart" } else { "empty" });
        self.inner.area.queue_draw();
    }
}

#[derive(Clone)]
pub struct Sparkline {
    area: gtk::DrawingArea,
    values: Rc<RefCell<Vec<Option<f64>>>>,
}

impl Sparkline {
    pub fn new(color: (f64, f64, f64)) -> Self {
        let area = gtk::DrawingArea::builder()
            .content_height(34)
            .width_request(78)
            .hexpand(true)
            .valign(gtk::Align::End)
            .build();
        let values = Rc::new(RefCell::new(Vec::new()));
        let weak_values = Rc::downgrade(&values);
        area.set_draw_func(move |_, cr, width, height| {
            if let Some(values) = weak_values.upgrade() {
                draw_sparkline(&values.borrow(), color, cr, width, height);
            }
        });
        Self { area, values }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_values(&self, values: Vec<Option<f64>>) {
        *self.values.borrow_mut() = values;
        self.area.queue_draw();
    }
}

fn draw_chart(inner: &ChartInner, cr: &Context, width: i32, height: i32) {
    let width = width as f64;
    let height = height as f64;
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let series = inner.series.borrow();
    if series.iter().all(ChartSeries::is_empty) {
        return;
    }

    let natural_max = series
        .iter()
        .map(ChartSeries::max_value)
        .fold(0.0_f64, f64::max);
    let max = natural_max.max(1.0);

    cr.set_line_width(1.0);
    cr.set_source_rgba(0.52, 0.52, 0.56, 0.16);
    for line in 0..GRID_LINES {
        let y = height * line as f64 / (GRID_LINES - 1) as f64;
        cr.move_to(0.0, y.round() + 0.5);
        cr.line_to(width, y.round() + 0.5);
        let _ = cr.stroke();
    }

    let minutes = inner.window_minutes.get().max(1);
    let window_ms = i64::from(minutes) * 60_000;
    let start_ms = inner.end_ms.get() - window_ms;

    for series in series.iter() {
        if series.points.is_empty() {
            continue;
        }
        cr.set_line_width(SERIES_WIDTH);
        cr.set_line_cap(LineCap::Round);
        cr.set_line_join(LineJoin::Round);
        cr.set_source_rgba(series.color.0, series.color.1, series.color.2, 0.95);

        let mut started = false;
        for (index, point) in series.points.iter().enumerate() {
            let x = ((point.at_ms - start_ms) as f64 / window_ms as f64) * width;
            let y = height - (point.value.max(0.0) / max) * height;
            if !started || (index > 0 && series.gap_after(index - 1)) {
                cr.move_to(x, y);
                started = true;
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }
}

fn draw_sparkline(
    values: &[Option<f64>],
    color: (f64, f64, f64),
    cr: &Context,
    width: i32,
    height: i32,
) {
    let width = width as f64;
    let height = height as f64;
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let max = values
        .iter()
        .flatten()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0_f64, f64::max)
        .max(1.0);
    if values.iter().all(Option::is_none) {
        return;
    }

    cr.set_line_width(SPARKLINE_WIDTH);
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
    cr.set_source_rgba(color.0, color.1, color.2, 0.95);

    let denominator = values.len().saturating_sub(1).max(1) as f64;
    let mut started = false;
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            started = false;
            continue;
        };
        let x = index as f64 / denominator * width;
        let y = height - (value.max(0.0) / max) * height;
        if !started {
            cr.move_to(x, y);
            started = true;
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}
