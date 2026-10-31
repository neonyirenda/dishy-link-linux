//! The speed-test page: three headline figures, jitter and loss, a Cairo
//! gauge, and the Go button.
//!
//! Port of upstream `components/speed-test/SpeedTestCard.tsx` (gauge view). The run
//! itself is `dishy_core::speed_test`, driven on a worker thread so the GTK
//! main thread never blocks. Latency, jitter, and loss are read from the
//! recorder's dish samples for the run's window, so the page adds no dish or
//! router request.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use dishy_core::speed_test::{
    CancelToken, GAUGE_TICKS, SpeedTestClient, SpeedTestPhase, SpeedTestProgress, gauge_fraction,
    link_quality,
};
use dishy_core::telemetry::TelemetrySample;
use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;

use crate::i18n::Translator;

/// The gauge's sweep, as in upstream `GaugeDial.tsx`: 0 at the bottom-left, then 270°
/// clockwise to the bottom-right.
const GAUGE_START_DEG: f64 = 135.0;
const GAUGE_SWEEP_DEG: f64 = 270.0;
/// The dashboard's upload colour, which the gauge fill uses while uploading.
const WARM: (f64, f64, f64) = (1.0, 0.69, 0.25);
/// How quickly the needle closes the gap to its reading, per second.
const EASE_RATE: f64 = 8.0;

const CSS: &str = r#"
.dishy-speed-figure-label {
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
}
.dishy-speed-figure-value {
  font-size: 28px;
  font-weight: 700;
  font-feature-settings: "tnum";
}
.dishy-speed-unit {
  font-size: 13px;
  font-weight: 500;
}
.dishy-speed-muted { opacity: 0.5; }
.dishy-speed-pills {
  border-top: 1px solid alpha(currentColor, 0.12);
  border-bottom: 1px solid alpha(currentColor, 0.12);
  padding: 8px 0;
}
.dishy-speed-pill-label { font-size: 12px; opacity: 0.66; }
.dishy-speed-pill-value { font-size: 13px; font-weight: 600; font-feature-settings: "tnum"; }
.dishy-speed-phase { font-size: 11.5px; font-weight: 500; opacity: 0.66; }
.dishy-speed-phase.error { color: #ff6b6b; opacity: 1; }
.dishy-speed-note { font-size: 10.5px; font-weight: 500; opacity: 0.46; }
.dishy-speed-go { min-height: 42px; border-radius: 12px; font-size: 14px; font-weight: 600; }
"#;

/// The strings this page paints, resolved once per locale.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedCopy {
    pub download: String,
    pub upload: String,
    pub latency: String,
    pub jitter: String,
    pub loss: String,
    pub ready: String,
    pub failed: String,
    pub go: String,
    pub run_again: String,
    pub try_again: String,
    pub running: String,
    pub phase_idle: String,
    pub phase_download: String,
    pub phase_upload: String,
    pub phase_done: String,
    pub phase_error: String,
    pub measured_against: String,
}

pub fn speed_copy(translator: &Translator) -> SpeedCopy {
    let text = |key: &str| translator.text(key);
    SpeedCopy {
        download: text("metrics.download"),
        upload: text("metrics.upload"),
        latency: text("metrics.latency"),
        jitter: text("latencyQuality.jitter"),
        loss: text("speedTest.loss"),
        ready: text("speedTest.ready"),
        failed: text("speedTest.failed"),
        go: text("speedTest.go"),
        run_again: text("speedTest.runAgain"),
        try_again: text("common.tryAgain"),
        running: text("speedTest.running"),
        phase_idle: text("speedTest.phaseIdle"),
        phase_download: text("speedTest.phaseDownload"),
        phase_upload: text("speedTest.phaseUpload"),
        phase_done: text("speedTest.phaseDone"),
        phase_error: text("speedTest.phaseError"),
        measured_against: text("speedTest.measuredAgainst"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeMode {
    Download,
    Upload,
    Idle,
}

/// What the gauge shows for a progress report.
#[derive(Debug, Clone, PartialEq)]
pub struct GaugeView {
    pub value: Option<f64>,
    pub mode: GaugeMode,
    pub caption: String,
}

pub fn gauge_view(progress: &SpeedTestProgress, copy: &SpeedCopy) -> GaugeView {
    let (value, mode, caption) = match progress.phase {
        SpeedTestPhase::Upload => (progress.upload_mbps, GaugeMode::Upload, &copy.upload),
        SpeedTestPhase::Download | SpeedTestPhase::Done => {
            (progress.download_mbps, GaugeMode::Download, &copy.download)
        }
        // The needle drains to zero under the caption the download reading
        // returns in, so it settles up from zero.
        SpeedTestPhase::Resting => (None, GaugeMode::Download, &copy.download),
        // A failure says so rather than resting on "Ready".
        SpeedTestPhase::Error => (None, GaugeMode::Idle, &copy.failed),
        SpeedTestPhase::Idle => (None, GaugeMode::Idle, &copy.ready),
    };
    GaugeView {
        value,
        mode,
        caption: caption.clone(),
    }
}

/// The line under the button.
pub fn phase_line(phase: SpeedTestPhase, copy: &SpeedCopy) -> String {
    match phase {
        SpeedTestPhase::Idle => &copy.phase_idle,
        SpeedTestPhase::Download => &copy.phase_download,
        SpeedTestPhase::Upload => &copy.phase_upload,
        SpeedTestPhase::Resting | SpeedTestPhase::Done => &copy.phase_done,
        SpeedTestPhase::Error => &copy.phase_error,
    }
    .clone()
}

/// The button's label, or `None` while a run is in progress (a spinner shows).
pub fn button_label(phase: SpeedTestPhase, copy: &SpeedCopy) -> Option<String> {
    match phase {
        SpeedTestPhase::Idle => Some(copy.go.clone()),
        SpeedTestPhase::Done => Some(copy.run_again.clone()),
        SpeedTestPhase::Error => Some(copy.try_again.clone()),
        SpeedTestPhase::Download | SpeedTestPhase::Upload | SpeedTestPhase::Resting => None,
    }
}

/// A headline figure: unmeasured reads as a muted 0, as in the Starlink app.
pub fn headline(value: Option<f64>, digits: usize) -> (String, bool) {
    match value {
        Some(value) => (format!("{value:.digits$}"), false),
        None => ("0".into(), true),
    }
}

/// A secondary figure: unmeasured reads as a dash.
pub fn pill(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.digits$}"))
}

/// Move the drawn value toward its target, frame by frame, so the needle
/// glides like a speedometer instead of snapping.
pub fn ease_toward(current: f64, target: f64, dt_seconds: f64) -> f64 {
    let step = 1.0 - (-EASE_RATE * dt_seconds.max(0.0)).exp();
    let next = current + (target - current) * step;
    if (target - next).abs() < 0.05 {
        target
    } else {
        next
    }
}

/// The big reading under either view, and whether it is the muted pending 0.
pub fn reading_text(value: Option<f64>, drawn: f64) -> (String, bool) {
    if value.is_none() && drawn < 0.1 {
        ("0".into(), true)
    } else {
        (format!("{drawn:.1}"), false)
    }
}

pub fn is_running(phase: SpeedTestPhase) -> bool {
    matches!(
        phase,
        SpeedTestPhase::Download | SpeedTestPhase::Upload | SpeedTestPhase::Resting
    )
}

/// A run on its worker thread, with the token that stops it.
struct ActiveRun {
    cancel: CancelToken,
    progress: Receiver<SpeedTestProgress>,
}

impl Drop for ActiveRun {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Figure {
    root: gtk::Box,
    value: gtk::Label,
}

impl Figure {
    fn new(icon: &str, label: &str, unit: &str) -> Self {
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        heading.set_halign(gtk::Align::Center);
        heading.append(&gtk::Image::from_icon_name(icon));
        let text = gtk::Label::new(Some(&label.to_uppercase()));
        text.add_css_class("dishy-speed-figure-label");
        heading.append(&text);
        let value = gtk::Label::new(Some("0"));
        value.add_css_class("dishy-speed-figure-value");
        let unit = gtk::Label::new(Some(unit));
        unit.add_css_class("dishy-speed-unit");
        unit.add_css_class("dim-label");
        unit.set_valign(gtk::Align::Baseline);
        let reading = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        reading.set_halign(gtk::Align::Center);
        reading.append(&value);
        reading.append(&unit);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.set_hexpand(true);
        root.append(&heading);
        root.append(&reading);
        Self { root, value }
    }

    fn show(&self, value: Option<f64>, digits: usize, active: bool) {
        let (text, pending) = headline(value, digits);
        self.value.set_label(&text);
        set_class(&self.value, "dim-label", pending);
        set_class(&self.root, "dishy-speed-muted", !active);
    }
}

#[derive(Clone)]
pub struct SpeedTestPage {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Box,
    copy: SpeedCopy,
    download: Figure,
    upload: Figure,
    latency: Figure,
    jitter: gtk::Label,
    loss: gtk::Label,
    gauge: gtk::DrawingArea,
    caption: gtk::Label,
    button: gtk::Button,
    spinner: gtk::Spinner,
    phase_label: gtk::Label,
    progress: RefCell<SpeedTestProgress>,
    samples: RefCell<Arc<Vec<TelemetrySample>>>,
    run: RefCell<Option<ActiveRun>>,
    /// The value the needle is drawn at, easing toward the reading.
    drawn: Cell<f64>,
    last_frame_us: Cell<i64>,
    /// The Starlink view (spec 0181) rather than the gauge.
    beam_view: Cell<bool>,
    view_switch: gtk::Box,
    /// The kit the beam view draws, and its decoded art.
    dish_model: RefCell<String>,
    art_surface: RefCell<Option<gtk::cairo::ImageSurface>>,
    beam_epoch_us: Cell<i64>,
}

impl SpeedTestPage {
    pub fn new(translator: &Translator) -> Self {
        install_css();
        let copy = speed_copy(translator);

        let download = Figure::new("go-down-symbolic", &copy.download, "Mbps");
        let upload = Figure::new("go-up-symbolic", &copy.upload, "Mbps");
        let latency = Figure::new("alarm-symbolic", &copy.latency, "ms");
        let figures = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        figures.set_homogeneous(true);
        figures.append(&download.root);
        figures.append(&upload.root);
        figures.append(&latency.root);

        let jitter = pill_value();
        let loss = pill_value();
        let pills = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        pills.set_halign(gtk::Align::Fill);
        pills.add_css_class("dishy-speed-pills");
        let pill_row = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        pill_row.set_halign(gtk::Align::Center);
        pill_row.set_hexpand(true);
        pill_row.append(&pill_block(&copy.jitter, &jitter, "ms"));
        pill_row.append(&pill_block(&copy.loss, &loss, "%"));
        pills.append(&pill_row);

        // The view switch, Starlink first and selected, as in Electron.
        let starlink_view = gtk::ToggleButton::builder()
            .label(translator.text("common.starlink"))
            .active(true)
            .build();
        let gauge_view_button = gtk::ToggleButton::builder()
            .label(translator.text("speedTest.gauge"))
            .group(&starlink_view)
            .build();
        let view_switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        view_switch.add_css_class("linked");
        view_switch.set_halign(gtk::Align::Center);
        view_switch.set_margin_bottom(4);
        view_switch.update_property(&[gtk::accessible::Property::Label(
            &translator.text("speedTest.viewLabel"),
        )]);
        view_switch.append(&starlink_view);
        view_switch.append(&gauge_view_button);

        let gauge = gtk::DrawingArea::builder()
            .content_width(300)
            .content_height(276)
            .halign(gtk::Align::Center)
            .build();
        let caption = gtk::Label::new(Some(&copy.ready));
        caption.add_css_class("dim-label");
        caption.add_css_class("heading");

        let button = gtk::Button::builder()
            .label(copy.go.clone())
            .hexpand(true)
            .build();
        button.add_css_class("suggested-action");
        button.add_css_class("dishy-speed-go");
        let spinner = gtk::Spinner::builder().spinning(true).build();
        spinner.update_property(&[gtk::accessible::Property::Label(&copy.running)]);

        let phase_label = gtk::Label::builder()
            .label(copy.phase_idle.clone())
            .justify(gtk::Justification::Center)
            .wrap(true)
            .build();
        phase_label.add_css_class("dishy-speed-phase");
        let note = gtk::Label::builder()
            .label(copy.measured_against.clone())
            .justify(gtk::Justification::Center)
            .wrap(true)
            .build();
        note.add_css_class("dishy-speed-note");

        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_width_request(360);
        column.set_halign(gtk::Align::Center);
        column.set_valign(gtk::Align::Center);
        column.set_margin_top(24);
        column.set_margin_bottom(24);
        column.set_margin_start(16);
        column.set_margin_end(16);
        column.append(&view_switch);
        column.append(&figures);
        column.append(&pills);
        column.append(&gauge);
        column.append(&caption);
        column.append(&button);
        column.append(&phase_label);
        column.append(&note);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&column)
            .build();

        scroller.add_css_class("dishy-page-scroller");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&scroller);

        let inner = Rc::new(Inner {
            root,
            copy,
            download,
            upload,
            latency,
            jitter,
            loss,
            gauge,
            caption,
            button,
            spinner,
            phase_label,
            progress: RefCell::new(SpeedTestProgress::IDLE),
            samples: RefCell::new(Arc::new(Vec::new())),
            run: RefCell::new(None),
            drawn: Cell::new(0.0),
            last_frame_us: Cell::new(0),
            beam_view: Cell::new(true),
            view_switch,
            dish_model: RefCell::new(String::new()),
            art_surface: RefCell::new(None),
            beam_epoch_us: Cell::new(glib::monotonic_time()),
        });
        inner.set_dish_model("unknown");
        let this = Rc::downgrade(&inner);
        starlink_view.connect_toggled(move |button| {
            if let Some(inner) = this.upgrade() {
                inner.beam_view.set(button.is_active());
                inner.gauge.queue_draw();
            }
        });
        connect(&inner);
        inner.show_progress();
        Self { inner }
    }

    pub fn is_beam_view(&self) -> bool {
        self.inner.beam_view.get()
    }

    /// The kit the Starlink view draws, from `dish_model_for`.
    pub fn set_dish_model(&self, model: &str) {
        self.inner.set_dish_model(model);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.inner.root
    }

    /// The recorder's rolling dish samples, for latency, jitter, and loss.
    pub fn set_samples(&self, samples: Arc<Vec<TelemetrySample>>) {
        *self.inner.samples.borrow_mut() = samples;
        self.inner.show_quality();
    }

    pub fn progress(&self) -> SpeedTestProgress {
        self.inner.progress.borrow().clone()
    }
}

impl Inner {
    fn start(&self) {
        // Whatever was in flight loses the link first: two runs measure each
        // other's load, and both report low.
        self.run.borrow_mut().take();
        let cancel = CancelToken::new();
        let (sender, receiver) = mpsc::channel();
        let token = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("dishy-speed-test".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let sender = Mutex::new(sender);
                runtime.block_on(SpeedTestClient::default().run(
                    &token,
                    Arc::new(move |progress| {
                        if let Ok(sender) = sender.lock() {
                            let _ = sender.send(progress);
                        }
                    }),
                ));
            });
        if let Err(error) = spawned {
            tracing::warn!("could not start the speed test: {error}");
            return;
        }
        *self.run.borrow_mut() = Some(ActiveRun {
            cancel,
            progress: receiver,
        });
    }

    /// Leaving the page ends the run, so it cannot saturate the link unseen.
    fn cancel(&self) {
        if self.run.borrow_mut().take().is_some() && is_running(self.progress.borrow().phase) {
            *self.progress.borrow_mut() = SpeedTestProgress::IDLE;
            self.show_progress();
        }
    }

    fn set_dish_model(&self, model: &str) {
        if *self.dish_model.borrow() == model {
            return;
        }
        *self.dish_model.borrow_mut() = model.to_owned();
        *self.art_surface.borrow_mut() =
            crate::speed_beam::art_surface(&crate::speed_beam::dish_art(model));
        self.gauge.queue_draw();
    }

    fn tick(&self) {
        let mut changed = false;
        loop {
            let next = self
                .run
                .borrow()
                .as_ref()
                .and_then(|run| run.progress.try_recv().ok());
            let Some(progress) = next else {
                break;
            };
            *self.progress.borrow_mut() = progress;
            changed = true;
        }
        if changed {
            self.show_progress();
        }
        let now_us = glib::monotonic_time();
        let last = self.last_frame_us.replace(now_us);
        let dt = if last == 0 {
            0.0
        } else {
            (now_us - last) as f64 / 1e6
        };
        let target = gauge_view(&self.progress.borrow(), &self.copy)
            .value
            .unwrap_or(0.0);
        let drawn = ease_toward(self.drawn.get(), target, dt.min(0.1));
        // The beam scene is always in motion; the gauge moves only when the
        // needle does.
        if drawn != self.drawn.get() || self.beam_view.get() {
            self.drawn.set(drawn);
            self.gauge.queue_draw();
        }
    }

    fn show_progress(&self) {
        let progress = self.progress.borrow().clone();
        let running = is_running(progress.phase);
        let measuring = matches!(
            progress.phase,
            SpeedTestPhase::Download | SpeedTestPhase::Upload
        );
        self.download.show(
            progress.download_mbps,
            1,
            !measuring || progress.phase == SpeedTestPhase::Download,
        );
        self.upload.show(
            progress.upload_mbps,
            1,
            !measuring || progress.phase == SpeedTestPhase::Upload,
        );
        let view = gauge_view(&progress, &self.copy);
        self.caption.set_label(&view.caption);
        match button_label(progress.phase, &self.copy) {
            Some(label) => {
                self.button.set_child(None::<&gtk::Widget>);
                self.button.set_label(&label);
            }
            None => self.button.set_child(Some(&self.spinner)),
        }
        self.button.set_sensitive(!running);
        self.view_switch.set_sensitive(!running);
        // The filled button belongs to the untouched page only.
        set_class(
            &self.button,
            "suggested-action",
            progress.phase == SpeedTestPhase::Idle,
        );
        self.phase_label
            .set_label(&phase_line(progress.phase, &self.copy));
        set_class(
            &self.phase_label,
            "error",
            progress.phase == SpeedTestPhase::Error,
        );
        self.show_quality();
        self.gauge.queue_draw();
    }

    fn show_quality(&self) {
        let progress = self.progress.borrow();
        let quality = link_quality(
            &self.samples.borrow(),
            progress.started_at_ms,
            progress.ended_at_ms,
            dishy_core::calendar::now_ms(),
        );
        self.latency
            .show(quality.latency_ms, 0, !is_running(progress.phase));
        self.jitter.set_label(&pill(quality.jitter_ms, 1));
        self.loss.set_label(&pill(quality.loss_pct, 1));
    }

    /// The Starlink view: the scene scaled to the width, the reading under it.
    fn draw_beam(&self, area: &gtk::DrawingArea, cr: &cairo::Context, width: i32, height: i32) {
        use crate::speed_beam::{VIEW_HEIGHT, VIEW_WIDTH};
        let progress = self.progress.borrow().clone();
        let view = gauge_view(&progress, &self.copy);
        let color = area.color();
        let ink = (
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
        );
        let active = is_running(progress.phase);
        let linked = active || view.mode != GaugeMode::Idle;
        let art = crate::speed_beam::dish_art(&self.dish_model.borrow());
        let elapsed_ms = (glib::monotonic_time() - self.beam_epoch_us.get()) as f64 / 1_000.0;
        let frame = crate::speed_beam::beam_frame(
            elapsed_ms,
            linked,
            active,
            view.mode == GaugeMode::Upload,
            crate::speed_beam::beam_origin(&art),
        );

        let reading_band = 46.0;
        let scale = (f64::from(width) / VIEW_WIDTH)
            .min((f64::from(height) - reading_band) / VIEW_HEIGHT)
            .max(0.1);
        let scene_x = (f64::from(width) - VIEW_WIDTH * scale) / 2.0;
        let _ = cr.save();
        cr.translate(scene_x, 0.0);
        cr.scale(scale, scale);
        crate::speed_beam::draw(
            cr,
            &frame,
            &art,
            self.art_surface.borrow().as_ref(),
            &crate::speed_beam::Palette::for_ink(ink),
        );
        let _ = cr.restore();

        let drawn = self.drawn.get();
        let (reading, pending) = reading_text(view.value, drawn);
        let baseline = VIEW_HEIGHT * scale + 38.0;
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        cr.set_font_size(38.0);
        let reading_width = cr
            .text_extents(&reading)
            .map_or(0.0, |extents| extents.x_advance());
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
        cr.set_font_size(13.0);
        let unit_width = cr
            .text_extents("Mbps")
            .map_or(0.0, |extents| extents.x_advance());
        let start = (f64::from(width) - reading_width - 6.0 - unit_width) / 2.0;
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        cr.set_font_size(38.0);
        cr.set_source_rgba(ink.0, ink.1, ink.2, if pending { 0.45 } else { 1.0 });
        cr.move_to(start, baseline);
        let _ = cr.show_text(&reading);
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
        cr.set_font_size(13.0);
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.55);
        cr.move_to(start + reading_width + 6.0, baseline);
        let _ = cr.show_text("Mbps");
    }

    fn draw_gauge(&self, area: &gtk::DrawingArea, cr: &cairo::Context, width: i32, height: i32) {
        if self.beam_view.get() {
            self.draw_beam(area, cr, width, height);
            return;
        }
        let view = gauge_view(&self.progress.borrow(), &self.copy);
        let ink = area.color();
        let ink = (
            f64::from(ink.red()),
            f64::from(ink.green()),
            f64::from(ink.blue()),
        );
        let fill = if view.mode == GaugeMode::Upload {
            WARM
        } else {
            ink
        };
        let drawn = self.drawn.get();
        let fraction = gauge_fraction(drawn);

        let size = f64::from(width.min(height));
        let center_x = f64::from(width) / 2.0;
        let center_y = f64::from(height) / 2.0 + size * 0.04;
        let radius = size * 0.4;
        let angle = |fraction: f64| (GAUGE_START_DEG + fraction * GAUGE_SWEEP_DEG).to_radians();
        // Cairo, like SVG, works in y-down space: angles turn clockwise.

        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_width(size * 0.055);
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.12);
        cr.arc(center_x, center_y, radius, angle(0.0), angle(1.0));
        let _ = cr.stroke();
        if fraction > 0.0 {
            cr.set_source_rgb(fill.0, fill.1, fill.2);
            cr.arc(center_x, center_y, radius, angle(0.0), angle(fraction));
            let _ = cr.stroke();
        }

        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
        cr.set_font_size(size * 0.042);
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.55);
        for tick in GAUGE_TICKS {
            let at = angle(gauge_fraction(f64::from(tick)));
            let label = tick.to_string();
            let (x, y) = (
                center_x + (radius - size * 0.1) * at.cos(),
                center_y + (radius - size * 0.1) * at.sin(),
            );
            centered_text(cr, &label, x, y);
        }

        let needle = angle(fraction);
        cr.set_line_width(size * 0.012);
        cr.set_source_rgb(fill.0, fill.1, fill.2);
        cr.move_to(center_x, center_y);
        cr.line_to(
            center_x + (radius - size * 0.16) * needle.cos(),
            center_y + (radius - size * 0.16) * needle.sin(),
        );
        let _ = cr.stroke();
        cr.arc(center_x, center_y, size * 0.022, 0.0, 2.0 * PI);
        let _ = cr.fill();

        let (reading, pending) = reading_text(view.value, drawn);
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        cr.set_font_size(size * 0.13);
        cr.set_source_rgba(ink.0, ink.1, ink.2, if pending { 0.45 } else { 1.0 });
        centered_text(cr, &reading, center_x, center_y + radius * 0.62);
        cr.set_font_size(size * 0.045);
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.55);
        centered_text(cr, "Mbps", center_x, center_y + radius * 0.86);
    }
}

fn connect(inner: &Rc<Inner>) {
    let this = Rc::downgrade(inner);
    inner.button.connect_clicked(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.start();
        }
    });

    let this = Rc::downgrade(inner);
    inner.gauge.set_draw_func(move |area, cr, width, height| {
        if let Some(inner) = this.upgrade() {
            inner.draw_gauge(area, cr, width, height);
        }
    });

    let this = Rc::downgrade(inner);
    inner.root.connect_unmap(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.cancel();
        }
    });

    let this = Rc::downgrade(inner);
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let Some(inner) = this.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if inner.root.is_mapped() {
            inner.tick();
        }
        glib::ControlFlow::Continue
    });
}

fn centered_text(cr: &cairo::Context, text: &str, x: f64, y: f64) {
    let Ok(extents) = cr.text_extents(text) else {
        return;
    };
    cr.move_to(
        x - extents.x_advance() / 2.0,
        y - (extents.y_bearing() + extents.height() / 2.0),
    );
    let _ = cr.show_text(text);
}

fn pill_value() -> gtk::Label {
    let label = gtk::Label::new(Some("—"));
    label.add_css_class("dishy-speed-pill-value");
    label
}

fn pill_block(label: &str, value: &gtk::Label, unit: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    let heading = gtk::Label::new(Some(label));
    heading.add_css_class("dishy-speed-pill-label");
    let unit = gtk::Label::new(Some(unit));
    unit.add_css_class("dim-label");
    row.append(&heading);
    row.append(value);
    row.append(&unit);
    row
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
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
    use crate::i18n::Locale;

    fn progress(phase: SpeedTestPhase, down: Option<f64>, up: Option<f64>) -> SpeedTestProgress {
        SpeedTestProgress {
            phase,
            download_mbps: down,
            upload_mbps: up,
            ..SpeedTestProgress::IDLE
        }
    }

    // SPEC-SPEED-007: every string comes from the shared catalog.
    #[test]
    fn the_copy_is_the_electron_wording() {
        let copy = speed_copy(&Translator::new(Locale::En));
        assert_eq!(copy.go, "Go");
        assert_eq!(copy.run_again, "Run again");
        assert_eq!(copy.try_again, "Try again");
        assert_eq!(copy.ready, "Ready");
        assert_eq!(copy.jitter, "Jitter");
        assert_eq!(copy.phase_download, "Measuring download…");
        assert!(copy.measured_against.contains("Cloudflare"));
        assert_ne!(speed_copy(&Translator::new(Locale::Fr)).go, "");
    }

    // SPEC-SPEED-003, SPEC-SPEED-007: the gauge follows the phase being measured, drains
    // to zero under the download caption while resting, and a failure says so.
    #[test]
    fn the_gauge_follows_the_phase() {
        let copy = speed_copy(&Translator::new(Locale::En));
        let view = |phase, down, up| gauge_view(&progress(phase, down, up), &copy);
        assert_eq!(
            view(SpeedTestPhase::Idle, None, None),
            GaugeView {
                value: None,
                mode: GaugeMode::Idle,
                caption: "Ready".into()
            }
        );
        assert_eq!(
            view(SpeedTestPhase::Download, Some(80.0), None).value,
            Some(80.0)
        );
        let upload = view(SpeedTestPhase::Upload, Some(80.0), Some(12.0));
        assert_eq!((upload.value, upload.mode), (Some(12.0), GaugeMode::Upload));
        assert_eq!(upload.caption, "Upload");
        let resting = view(SpeedTestPhase::Resting, Some(80.0), Some(12.0));
        assert_eq!((resting.value, resting.mode), (None, GaugeMode::Download));
        let done = view(SpeedTestPhase::Done, Some(80.0), Some(12.0));
        assert_eq!(
            (done.value, done.caption.as_str()),
            (Some(80.0), "Download")
        );
        let failed = view(SpeedTestPhase::Error, None, None);
        assert_eq!(
            (failed.mode, failed.caption.as_str()),
            (GaugeMode::Idle, "Failed")
        );
    }

    // SPEC-SPEED-007: button, phase line, and figure formats.
    #[test]
    fn controls_and_figures_use_the_electron_formats() {
        let copy = speed_copy(&Translator::new(Locale::En));
        assert_eq!(
            button_label(SpeedTestPhase::Idle, &copy).as_deref(),
            Some("Go")
        );
        assert_eq!(
            button_label(SpeedTestPhase::Done, &copy).as_deref(),
            Some("Run again")
        );
        assert_eq!(
            button_label(SpeedTestPhase::Error, &copy).as_deref(),
            Some("Try again")
        );
        assert_eq!(button_label(SpeedTestPhase::Upload, &copy), None);
        assert_eq!(button_label(SpeedTestPhase::Resting, &copy), None);
        assert_eq!(phase_line(SpeedTestPhase::Resting, &copy), copy.phase_done);
        assert_eq!(phase_line(SpeedTestPhase::Error, &copy), copy.phase_error);

        assert_eq!(headline(None, 1), ("0".to_string(), true));
        assert_eq!(headline(Some(87.26), 1), ("87.3".to_string(), false));
        assert_eq!(headline(Some(24.6), 0), ("25".to_string(), false));
        assert_eq!(pill(None, 1), "—");
        assert_eq!(pill(Some(0.42), 1), "0.4");
    }

    // SPEC-SPEED-006: the needle eases toward the reading without overshoot.
    // SPEC-BEAM-005: both views read one decimal of the eased value, and a
    // reading not yet measured shows a muted 0 until the value drains.
    #[test]
    fn the_reading_is_shared_by_both_views() {
        assert_eq!(
            reading_text(Some(118.5), 118.46),
            ("118.5".to_string(), false)
        );
        assert_eq!(reading_text(None, 0.05), ("0".to_string(), true));
        assert_eq!(
            reading_text(None, 42.0),
            ("42.0".to_string(), false),
            "still draining"
        );
    }

    #[test]
    fn the_needle_eases_without_overshooting() {
        let first = ease_toward(0.0, 100.0, 1.0 / 60.0);
        assert!(first > 0.0 && first < 100.0);
        let mut value = 0.0;
        for _ in 0..240 {
            value = ease_toward(value, 100.0, 1.0 / 60.0);
            assert!(value <= 100.0);
        }
        assert!((value - 100.0).abs() < 0.1);
        assert_eq!(ease_toward(50.0, 50.0, 0.016), 50.0);
        assert!(ease_toward(100.0, 0.0, 0.016) < 100.0);
    }
}
