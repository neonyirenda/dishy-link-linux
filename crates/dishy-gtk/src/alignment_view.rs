//! The alignment panel: SpaceX's own verdict on whether the dish points
//! inside its five-degree service cone, the two instruments that draw the dials,
//! and the facts that explain them.
//!
//! This ports upstream `components/alignment/AlignmentCard.tsx` from the Electron renderer
//! to native GTK4 widgets. The math is in `dishy-core::alignment`; this module
//! presents it, draws the instruments with Cairo, and keeps everything
//! translated. It starts no poll and reads only the recorder's cached status.

use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::Once;

use dishy_core::alignment::{AlignmentReading, compute_alignment};
use dishy_core::telemetry::DishStatus;
use gtk::cairo;
use gtk::prelude::*;
use libadwaita as adw;

/// Parse a hex color string into RGB components.
fn parse_hex_color(hex: &str) -> (f64, f64, f64) {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64 / 255.0;
    (r, g, b)
}

use crate::i18n::Translator;

/// Size of the instrument dials in pixels.
const INSTRUMENT_SIZE: f64 = 250.0;

/// Their needle orange, verbatim from the dish's own web app.
const NEEDLE_ORANGE: &str = "#ffac30";

/// Instrument health berry state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BerryState {
    Good,
    Bad,
    Unknown,
}

/// Instrument drawing widget using Cairo.
struct InstrumentDrawing {
    reading: AlignmentReading,
    instrument_type: InstrumentType,
}

/// Electron's dial tokens for the current theme: `--ink-secondary` for the
/// dots and letters, `--ink-muted` for the wedge, `--dish-edge` for the plate.
struct DialPalette {
    secondary: (f64, f64, f64),
    muted: (f64, f64, f64),
    dish_edge: (f64, f64, f64, f64),
}

impl DialPalette {
    /// Picked from the widget's ink: light ink means the dark theme.
    fn for_ink(ink: (f64, f64, f64)) -> Self {
        let rgb = |hex: u32| {
            (
                f64::from((hex >> 16) & 0xff) / 255.0,
                f64::from((hex >> 8) & 0xff) / 255.0,
                f64::from(hex & 0xff) / 255.0,
            )
        };
        if (ink.0 + ink.1 + ink.2) / 3.0 > 0.5 {
            Self {
                secondary: rgb(0xb9b9b9),
                muted: rgb(0x7c7c7c),
                dish_edge: (1.0, 1.0, 1.0, 0.559),
            }
        } else {
            let (r, g, b) = rgb(0xaba9a9);
            Self {
                secondary: rgb(0x55544f),
                muted: rgb(0x8a8880),
                dish_edge: (r, g, b, 1.0),
            }
        }
    }
}

/// The white dish plate, its corners rounded and edged as Electron draws it,
/// centred on the current origin.
fn dish_plate(cr: &cairo::Context, width: f64, height: f64, palette: &DialPalette) {
    let (x, y, r) = (-width / 2.0, -height / 2.0, 1.0);
    cr.new_sub_path();
    cr.arc(x + width - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + width - r, y + height - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + height - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    let (er, eg, eb, ea) = palette.dish_edge;
    cr.set_source_rgba(er, eg, eb, ea);
    cr.set_line_width(0.75);
    let _ = cr.stroke();
}

/// A pie slice from `(cx, cy)`. The explicit move to the centre matters:
/// Cairo's `arc` otherwise joins from wherever the last mark left the pen.
fn wedge(cr: &cairo::Context, cx: f64, cy: f64, radius: f64, start: f64, end: f64) {
    cr.new_path();
    cr.move_to(cx, cy);
    cr.arc(cx, cy, radius, start, end);
    cr.close_path();
    let _ = cr.fill();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstrumentType {
    Rotation,
    Tilt,
}

impl InstrumentDrawing {
    fn new(reading: AlignmentReading, instrument_type: InstrumentType) -> Self {
        Self {
            reading,
            instrument_type,
        }
    }

    /// Draw in Electron's 250-unit viewBox, scaled to fit and centred, as the
    /// SVG's `width='100%'` does.
    fn draw(&self, cr: &cairo::Context, width: f64, height: f64, ink: (f64, f64, f64)) {
        let palette = DialPalette::for_ink(ink);
        let scale = (width.min(height) / INSTRUMENT_SIZE).max(0.01);
        cr.translate(
            (width - INSTRUMENT_SIZE * scale) / 2.0,
            (height - INSTRUMENT_SIZE * scale) / 2.0,
        );
        cr.scale(scale, scale);
        let center = INSTRUMENT_SIZE / 2.0;
        match self.instrument_type {
            InstrumentType::Rotation => self.draw_rotation(cr, center, &palette),
            InstrumentType::Tilt => self.draw_tilt(cr, &palette),
        }
    }

    fn draw_rotation(&self, cr: &cairo::Context, center: f64, palette: &DialPalette) {
        // SPEC-ALIGN-005: dotted 5° ring with gaps around cardinals, compass letters north-up,
        // tolerance wedge centered on desired azimuth, dish pointer + orange needle at actual azimuth
        let ring_radius = 0.45 * INSTRUMENT_SIZE;
        let dish_width = INSTRUMENT_SIZE / 8.0;
        let dish_height = INSTRUMENT_SIZE / 6.0;

        // Draw the dotted ring: 72 dots at 5° intervals, skipping 3 around each cardinal
        // Their tick loop: dots at 5° intervals, but skip positions 0, 1, 17 in each 18-dot quadrant
        // which corresponds to angles that are multiples of 90° (cardinals)
        let (r, g, b) = palette.secondary;
        cr.set_source_rgb(r, g, b);

        for dot_index in 0..72 {
            let position_in_quadrant = dot_index % 18;
            // Skip dots around cardinals (positions 0, 1, 17 in each quadrant)
            if position_in_quadrant == 0 || position_in_quadrant == 1 || position_in_quadrant == 17
            {
                continue;
            }
            let angle_deg = dot_index as f64 * 5.0 - 90.0; // -90 offset for north-up
            let angle_rad = angle_deg * PI / 180.0;
            let x = center + ring_radius * angle_rad.cos();
            let y = center + ring_radius * angle_rad.sin();
            cr.arc(x, y, 1.2, 0.0, 2.0 * PI);
            cr.fill().unwrap();
        }

        // Draw compass labels: N, E, S, W (upright, not rotated)
        let font_size = INSTRUMENT_SIZE / 20.0;
        cr.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        cr.set_font_size(font_size);
        cr.set_source_rgb(r, g, b);

        let labels = [("N", -90.0), ("E", 0.0), ("S", 90.0), ("W", 180.0)];
        for (label, angle_deg) in labels.iter() {
            let angle_rad = angle_deg * PI / 180.0;
            let x = center + ring_radius * angle_rad.cos();
            let y = center + ring_radius * angle_rad.sin();

            // Upright and centred on the mark, like SVG's text-anchor: middle
            // with dy='0.35em'.
            let advance = cr
                .text_extents(label)
                .map(|extents| extents.x_advance())
                .unwrap_or(0.0);
            cr.move_to(x - advance / 2.0, y + font_size * 0.35);
            cr.show_text(label).unwrap();
        }

        // Draw tolerance wedge centered on desired azimuth
        if self.reading.is_valid {
            let wedge_radius = 0.98 * ring_radius;
            let opacity = if self.reading.is_aligned { 0.32 } else { 0.16 };
            let (r, g, b) = palette.muted;
            cr.set_source_rgba(r, g, b, opacity);
            let (start_rad, end_rad) = rotation_wedge_rad(
                self.reading.desired_azimuth_deg,
                self.reading.azimuth_tolerance_deg,
            );
            wedge(cr, center, center, wedge_radius, start_rad, end_rad);
        }

        // Dish pointer and needle at the actual azimuth. The needle is drawn
        // pointing up (north), so the azimuth is the rotation itself.
        let needle_angle_rad = rotation_pointer_rad(
            self.reading
                .is_valid
                .then_some(self.reading.boresight_azimuth_deg),
        );
        cr.save().unwrap();
        cr.translate(center, center);
        cr.rotate(needle_angle_rad);
        dish_plate(cr, dish_width, dish_height, palette);

        // Draw needle (orange line)
        if self.reading.is_valid {
            let needle_y2 = center - 0.94 * ring_radius;
            let (r, g, b) = parse_hex_color(NEEDLE_ORANGE);
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.5);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.move_to(0.0, 0.0);
            cr.line_to(0.0, needle_y2 - center);
            cr.stroke().unwrap();
        }
        cr.restore().unwrap();
    }

    fn draw_tilt(&self, cr: &cairo::Context, palette: &DialPalette) {
        // SPEC-ALIGN-005: y-flipped quarter arc from 0 to 90°, wedge spanning valid elevation band,
        // needle at actual elevation
        let pivot = INSTRUMENT_SIZE / 6.0;
        let arc_radius = 0.77 * INSTRUMENT_SIZE;
        let dish_length = INSTRUMENT_SIZE / 4.0;
        let dish_thickness = dish_length / 10.0;

        // Set up y-flipped coordinate system: translate(0, size) scale(1, -1)
        cr.save().unwrap();
        cr.translate(0.0, INSTRUMENT_SIZE);
        cr.scale(1.0, -1.0);

        // Draw the arc dots: 0° to 90° in 5° increments
        let (r, g, b) = palette.secondary;
        cr.set_source_rgb(r, g, b);
        for dot_index in 0..=18 {
            let angle_deg = dot_index as f64 * 5.0;
            let angle_rad = angle_deg * PI / 180.0;
            let x = pivot + arc_radius * angle_rad.cos();
            let y = pivot + arc_radius * angle_rad.sin();
            cr.arc(x, y, 1.2, 0.0, 2.0 * PI);
            cr.fill().unwrap();
        }

        // Draw tolerance wedge spanning the valid elevation band
        if self.reading.is_valid {
            let wedge_radius = 0.98 * arc_radius;
            let wedge_center_deg = (self.reading.upper_elevation_limit_deg
                + self.reading.lower_elevation_limit_deg)
                / 2.0;
            let wedge_span_deg =
                self.reading.upper_elevation_limit_deg - self.reading.lower_elevation_limit_deg;

            let opacity = if self.reading.is_elevation_valid {
                0.32
            } else {
                0.16
            };
            let (r, g, b) = palette.muted;
            cr.set_source_rgba(r, g, b, opacity);

            let start_rad = (wedge_center_deg - wedge_span_deg / 2.0) * PI / 180.0;
            let end_rad = (wedge_center_deg + wedge_span_deg / 2.0) * PI / 180.0;
            wedge(cr, pivot, pivot, wedge_radius, start_rad, end_rad);
        }

        // Draw dish plate and needle at actual elevation
        let needle_elevation = if self.reading.is_valid {
            self.reading.boresight_elevation_deg - 90.0
        } else {
            70.0 - 90.0 // Default to middle when not valid
        };
        let needle_angle_rad = needle_elevation * PI / 180.0;

        cr.save().unwrap();
        cr.translate(pivot, pivot);
        cr.rotate(needle_angle_rad);

        dish_plate(cr, dish_length, dish_thickness, palette);

        // Draw needle (orange line)
        if self.reading.is_valid {
            let needle_y2 = pivot + 0.96 * arc_radius;
            let (r, g, b) = parse_hex_color(NEEDLE_ORANGE);
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(1.5);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.move_to(0.0, 0.0);
            cr.line_to(0.0, needle_y2 - pivot);
            cr.stroke().unwrap();
        }
        cr.restore().unwrap();

        cr.restore().unwrap();
    }
}

/// The tolerance wedge's start and end, in Cairo radians from +x. Cairo, like
/// SVG, works in y-down device space, so a positive angle already turns
/// clockwise on screen and the Electron angles carry over unchanged. Compass
/// north is up, a quarter turn back from +x.
fn rotation_wedge_rad(desired_azimuth_deg: f64, tolerance_deg: f64) -> (f64, f64) {
    let center = desired_azimuth_deg - 90.0;
    (
        (center - tolerance_deg).to_radians(),
        (center + tolerance_deg).to_radians(),
    )
}

/// The pointer's rotation about the dial centre. The needle is drawn toward
/// −y (north), so this is the azimuth itself; it rests north with no reading.
fn rotation_pointer_rad(boresight_azimuth_deg: Option<f64>) -> f64 {
    boresight_azimuth_deg.unwrap_or(0.0).to_radians()
}

/// Health berry for a dial: green when in spec, red when out, grey until the
/// attitude filter has anything to say. (SPEC-ALIGN-005)
fn berry_for(in_spec: bool, is_valid: bool) -> BerryState {
    if in_spec {
        BerryState::Good
    } else if is_valid {
        BerryState::Bad
    } else {
        BerryState::Unknown
    }
}

/// Health berry widget.
fn create_berry(berry: BerryState) -> gtk::Widget {
    let color = match berry {
        // Electron's --status-good, --status-critical, --ink-muted.
        BerryState::Good => "#4db450",
        BerryState::Bad => "#f43f5e",
        BerryState::Unknown => "#7c7c7c",
    };

    // Create a drawing area for the berry
    let drawing = gtk::DrawingArea::new();
    drawing.set_size_request(7, 7);
    drawing.set_hexpand(false);
    drawing.set_vexpand(false);

    let color_clone = color.to_string();
    let (r, g, b) = parse_hex_color(&color_clone);
    drawing.set_draw_func(move |_, cr, _width, _height| {
        cr.set_source_rgb(r, g, b);
        cr.arc(3.5, 3.5, 3.5, 0.0, 2.0 * PI);
        cr.fill().unwrap();
    });

    drawing.upcast()
}

/// Instrument header with label and berry.
fn create_instrument_header(label: &str, berry: BerryState) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    header.set_halign(gtk::Align::Start);

    let label_widget = gtk::Label::new(Some(label));
    label_widget.set_xalign(0.0);
    label_widget.set_halign(gtk::Align::Start);
    label_widget.add_css_class("dishy-alignment-instrument-label");
    header.append(&label_widget);

    let berry_widget = create_berry(berry);
    header.append(&berry_widget);

    header
}

/// The strings this page paints, resolved once per locale.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentCopy {
    pub alignment: String,
    pub stale: String,
    pub stale_from: String,
    pub not_ready: String,
    pub aligned: String,
    pub not_aligned: String,
    pub no_reading: String,
    pub current_rotation: String,
    pub rotate_recommendation: String,
    pub target_azimuth: String,
    pub boresight_error: String,
    pub boresight_error_value: String,
    pub attitude_uncertainty: String,
    pub attitude_state: String,
    pub gps_satellites: String,
    pub current_tilt: String,
    pub tilt_recommendation: String,
    pub boresight_elevation: String,
    pub target_elevation: String,
    pub elevation_range: String,
    pub rotation: String,
    pub tilt: String,
    pub how_to_read: String,
    pub how_to_read_body: String,
    pub satellite_view: String,
    pub state_converged: String,
    pub state_unconverged: String,
    pub state_reset: String,
    pub state_faulted: String,
    pub state_invalid: String,
}

pub fn alignment_copy(translator: &Translator) -> AlignmentCopy {
    let text = |key: &str| translator.text(key);
    let text_with = |key: &str, params: &[(&str, &str)]| translator.text_with(key, params);

    let limit = 5.0;
    let boresight_error_value = text_with(
        "alignment.boresightErrorValue",
        &[("error", "{error}"), ("limit", &format!("{limit}"))],
    );

    AlignmentCopy {
        alignment: text("navigation.alignment"),
        stale: text("alignment.stale"),
        stale_from: text("alignment.staleFrom"),
        not_ready: text("alignment.notReady"),
        aligned: text("alignment.aligned"),
        not_aligned: text("alignment.notAligned"),
        no_reading: text("alignment.noReading"),
        current_rotation: text("alignment.currentRotation"),
        rotate_recommendation: text("alignment.rotateRecommendation"),
        target_azimuth: text("alignment.targetAzimuth"),
        boresight_error: text("alignment.boresightError"),
        boresight_error_value,
        attitude_uncertainty: text("alignment.attitudeUncertainty"),
        attitude_state: text("alignment.attitudeState"),
        gps_satellites: text("terminal.gpsSatellites"),
        current_tilt: text("alignment.currentTilt"),
        tilt_recommendation: text("alignment.tiltRecommendation"),
        boresight_elevation: text("alignment.boresightElevation"),
        target_elevation: text("alignment.targetElevation"),
        elevation_range: text("alignment.elevationRange"),
        rotation: text("alignment.rotation"),
        tilt: text("alignment.tilt"),
        how_to_read: text("alignment.howToRead"),
        how_to_read_body: text("alignment.howToReadBody"),
        satellite_view: text("navigation.satelliteView"),
        state_converged: text("alignment.state.converged"),
        state_unconverged: text("alignment.state.unconverged"),
        state_reset: text("alignment.state.reset"),
        state_faulted: text("alignment.state.faulted"),
        state_invalid: text("alignment.state.invalid"),
    }
}

/// Format a numeric value with one decimal place.
fn format_f64_one(value: f64) -> String {
    if value.is_finite() {
        format!("{:.1}", value)
    } else {
        "—".to_string()
    }
}

/// Format a numeric value with two decimal places.
fn format_f64_two(value: f64) -> String {
    if value.is_finite() {
        format!("{:.2}", value)
    } else {
        "—".to_string()
    }
}

/// The arrow symbol for azimuth adjustment.
fn azimuth_arrow(azimuth_error_deg: f64) -> &'static str {
    if azimuth_error_deg > 0.0 {
        "↺"
    } else if azimuth_error_deg < 0.0 {
        "↻"
    } else {
        ""
    }
}

/// Format the tilt recommendation direction.
fn tilt_direction(elevation_error_deg: f64) -> &'static str {
    if elevation_error_deg > 0.0 {
        "down"
    } else if elevation_error_deg < 0.0 {
        "up"
    } else {
        ""
    }
}

/// Attitude state label from the status.
fn attitude_state_label(status: &DishStatus, copy: &AlignmentCopy) -> String {
    status
        .alignment_stats
        .as_ref()
        .and_then(|stats| stats.attitude_estimation_state.as_deref())
        .map(|state| match state {
            "FILTER_CONVERGED" => copy.state_converged.clone(),
            "FILTER_UNCONVERGED" => copy.state_unconverged.clone(),
            "FILTER_RESET" => copy.state_reset.clone(),
            "FILTER_FAULTED" => copy.state_faulted.clone(),
            "FILTER_INVALID" => copy.state_invalid.clone(),
            _ => state.to_string(),
        })
        .unwrap_or("—".to_string())
}

/// GPS satellites count.
fn gps_satellites_label(status: &DishStatus) -> String {
    if let Some(gps_stats) = status.gps_stats.as_ref() {
        if gps_stats.gps_valid {
            format!("{} satellites", gps_stats.gps_sats)
        } else {
            "No fix".to_string()
        }
    } else {
        "—".to_string()
    }
}

/// Attitude uncertainty.
fn attitude_uncertainty_label(status: &DishStatus) -> String {
    if let Some(alignment_stats) = status.alignment_stats.as_ref() {
        format!(
            "±{}°",
            format_f64_two(alignment_stats.attitude_uncertainty_deg.0)
        )
    } else {
        "—".to_string()
    }
}

/// The main alignment page.
#[derive(Clone)]
pub struct AlignmentPage {
    inner: Rc<Inner>,
}

#[allow(dead_code)]
struct Inner {
    root: adw::StatusPage,
    waiting: adw::StatusPage,
    content: gtk::Box,
    translator: Translator,
    copy: AlignmentCopy,
    status: RefCell<Option<DishStatus>>,
    reading: RefCell<Option<AlignmentReading>>,
}

impl AlignmentPage {
    pub fn new(translator: &Translator) -> Self {
        install_css();
        let copy = alignment_copy(translator);
        let translator_clone = translator.clone();

        // Waiting state
        let waiting = adw::StatusPage::builder()
            .title(copy.no_reading.clone())
            .icon_name("find-location-symbolic")
            .build();

        // Main content box
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        // Root status page
        let root = adw::StatusPage::builder()
            .icon_name("find-location-symbolic")
            .build();

        let inner = Rc::new(Inner {
            root: root.clone(),
            waiting,
            content,
            translator: translator_clone,
            copy,
            status: RefCell::new(None),
            reading: RefCell::new(None),
        });

        inner.update_display();

        Self { inner }
    }

    pub fn widget(&self) -> &adw::StatusPage {
        &self.inner.root
    }

    pub fn set_status(&self, status: Option<DishStatus>) {
        *self.inner.status.borrow_mut() = status;
        self.inner.update_display();
    }
}

impl Inner {
    fn update_display(&self) {
        let status = self.status.borrow().clone();
        let reading = status.as_ref().map(compute_alignment);
        *self.reading.borrow_mut() = reading;

        if let Some(ref r) = reading {
            // Build the full content with the reading
            let content = self.build_content(r, status.as_ref().unwrap());
            self.root.set_child(Some(&content));
        } else {
            self.root.set_child(Some(&self.waiting));
        }
    }

    fn build_content(&self, reading: &AlignmentReading, status: &DishStatus) -> gtk::Box {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        // Verdict line
        let verdict = self.build_verdict(reading);
        content.append(&verdict);

        // Instruments row (placeholders for now)
        let instruments = self.build_instruments(reading);
        content.append(&instruments);

        // Facts section
        let facts = self.build_facts(reading, status);
        content.append(&facts);

        // How to read explainer
        let explainer = self.build_explainer();
        content.append(&explainer);

        // Satellite view link
        let satellite_link = gtk::LinkButton::with_label("", &self.copy.satellite_view);
        satellite_link.set_halign(gtk::Align::Start);
        satellite_link.set_margin_top(12);
        content.append(&satellite_link);

        content
    }

    fn build_verdict(&self, reading: &AlignmentReading) -> gtk::Label {
        let (text, class) = if !reading.is_valid {
            (
                self.copy.not_ready.clone(),
                "dishy-alignment-verdict-not-ready",
            )
        } else if reading.is_aligned {
            (self.copy.aligned.clone(), "dishy-alignment-verdict-aligned")
        } else {
            (
                self.copy.not_aligned.clone(),
                "dishy-alignment-verdict-not-aligned",
            )
        };

        let label = gtk::Label::new(Some(&text));
        label.set_xalign(0.0);
        label.add_css_class("dishy-alignment-verdict");
        label.add_css_class(class);
        label
    }

    fn build_instruments(&self, reading: &AlignmentReading) -> gtk::Box {
        // SPEC-ALIGN-005: The instruments draw the dials: rotation dial with dotted 5° ring
        // (gaps around cardinals), compass letters north-up, tolerance wedge centered on desired
        // azimuth, dish pointer plus orange needle at actual azimuth; tilt dial with y-flipped
        // quarter arc from 0 to 90°, wedge spanning valid elevation band, needle at actual
        // elevation. Each dial carries a health berry.
        // Two equal cards sharing the row, as Electron's flex-1 pair does.
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_homogeneous(true);

        // Rotation instrument
        let rotation_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        rotation_card.add_css_class("dishy-alignment-instrument-card");
        rotation_card.set_hexpand(true);

        let rotation_berry = berry_for(reading.is_aligned, reading.is_valid);
        let rotation_head = create_instrument_header(&self.copy.rotation, rotation_berry);
        rotation_card.append(&rotation_head);

        let rotation_drawing = gtk::DrawingArea::new();
        rotation_drawing.set_size_request(INSTRUMENT_SIZE as i32, INSTRUMENT_SIZE as i32);
        rotation_drawing.set_hexpand(true);
        rotation_drawing.set_vexpand(true);

        let rotation_reading = *reading;
        rotation_drawing.set_draw_func(move |area, cr, width, height| {
            let ink = area.color();
            let ink = (
                f64::from(ink.red()),
                f64::from(ink.green()),
                f64::from(ink.blue()),
            );
            let instrument = InstrumentDrawing::new(rotation_reading, InstrumentType::Rotation);
            instrument.draw(cr, f64::from(width), f64::from(height), ink);
        });
        // Square, growing with the card, as the SVG's viewBox keeps it.
        let rotation_frame = gtk::AspectFrame::new(0.5, 0.5, 1.0, false);
        rotation_frame.set_child(Some(&rotation_drawing));
        rotation_frame.set_hexpand(true);
        rotation_card.append(&rotation_frame);

        // Tilt instrument
        let tilt_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        tilt_card.add_css_class("dishy-alignment-instrument-card");
        tilt_card.set_hexpand(true);

        let tilt_berry = berry_for(reading.is_elevation_valid, reading.is_valid);
        let tilt_head = create_instrument_header(&self.copy.tilt, tilt_berry);
        tilt_card.append(&tilt_head);

        let tilt_drawing = gtk::DrawingArea::new();
        tilt_drawing.set_size_request(INSTRUMENT_SIZE as i32, INSTRUMENT_SIZE as i32);
        tilt_drawing.set_hexpand(true);
        tilt_drawing.set_vexpand(true);

        let tilt_reading = *reading;
        tilt_drawing.set_draw_func(move |area, cr, width, height| {
            let ink = area.color();
            let ink = (
                f64::from(ink.red()),
                f64::from(ink.green()),
                f64::from(ink.blue()),
            );
            let instrument = InstrumentDrawing::new(tilt_reading, InstrumentType::Tilt);
            instrument.draw(cr, f64::from(width), f64::from(height), ink);
        });
        // Square, growing with the card, as the SVG's viewBox keeps it.
        let tilt_frame = gtk::AspectFrame::new(0.5, 0.5, 1.0, false);
        tilt_frame.set_child(Some(&tilt_drawing));
        tilt_frame.set_hexpand(true);
        tilt_card.append(&tilt_frame);

        row.append(&rotation_card);
        row.append(&tilt_card);
        row
    }

    fn build_facts(&self, reading: &AlignmentReading, status: &DishStatus) -> gtk::Box {
        let facts = gtk::Box::new(gtk::Orientation::Horizontal, 24);

        // Rotation facts column
        let rotation_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        rotation_col.add_css_class("dishy-alignment-fact-column");

        rotation_col.append(&self.fact_row(
            &self.copy.current_rotation,
            &format!("{}°", format_f64_one(reading.boresight_azimuth_deg)),
        ));

        let rotate_value = format!(
            "{}° {}",
            format_f64_two(reading.azimuth_error_deg.abs()),
            azimuth_arrow(reading.azimuth_error_deg)
        );
        let rotate_class = if reading.boresight_error_deg.abs() < 5.0 {
            "dishy-alignment-in-spec"
        } else {
            "dishy-alignment-out-of-spec"
        };
        rotation_col.append(&self.colored_fact_row(
            &self.copy.rotate_recommendation,
            &rotate_value,
            rotate_class,
        ));

        rotation_col.append(&self.fact_row(
            &self.copy.target_azimuth,
            &format!(
                "{}° ±{}°",
                format_f64_one(reading.desired_azimuth_deg),
                format_f64_one(reading.azimuth_tolerance_deg)
            ),
        ));

        let boresight_value = self
            .copy
            .boresight_error_value
            .replace("{error}", &format_f64_two(reading.boresight_error_deg));
        let boresight_class = if reading.boresight_error_deg < 5.0 {
            "dishy-alignment-in-spec"
        } else {
            "dishy-alignment-out-of-spec"
        };
        rotation_col.append(&self.colored_fact_row(
            &self.copy.boresight_error,
            &boresight_value,
            boresight_class,
        ));

        rotation_col.append(&self.fact_row(
            &self.copy.attitude_uncertainty,
            &attitude_uncertainty_label(status),
        ));

        rotation_col.append(&self.fact_row(
            &self.copy.attitude_state,
            &attitude_state_label(status, &self.copy),
        ));

        rotation_col
            .append(&self.fact_row(&self.copy.gps_satellites, &gps_satellites_label(status)));

        // Tilt facts column
        let tilt_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tilt_col.add_css_class("dishy-alignment-fact-column");

        tilt_col.append(&self.fact_row(
            &self.copy.current_tilt,
            &format!("{}°", format_f64_one(reading.tilt_angle_deg)),
        ));

        let tilt_value = if reading.elevation_error_deg.abs() > 0.01 {
            format!(
                "{:.2}° {}",
                reading.elevation_error_deg.abs(),
                tilt_direction(reading.elevation_error_deg)
            )
        } else {
            "0°".to_string()
        };
        let tilt_class = if reading.elevation_error_deg.abs() < 5.0 {
            "dishy-alignment-in-spec"
        } else {
            "dishy-alignment-out-of-spec"
        };
        tilt_col.append(&self.colored_fact_row(
            &self.copy.tilt_recommendation,
            &tilt_value,
            tilt_class,
        ));

        tilt_col.append(&self.fact_row(
            &self.copy.boresight_elevation,
            &format!("{}°", format_f64_one(reading.boresight_elevation_deg)),
        ));

        tilt_col.append(&self.fact_row(
            &self.copy.target_elevation,
            &format!("{}°", format_f64_one(reading.desired_elevation_deg)),
        ));

        tilt_col.append(&self.fact_row(
            &self.copy.elevation_range,
            &format!(
                "{}° – {}°",
                format_f64_one(reading.lower_elevation_limit_deg),
                format_f64_one(reading.upper_elevation_limit_deg)
            ),
        ));

        facts.append(&rotation_col);
        facts.append(&tilt_col);
        facts
    }

    fn build_explainer(&self) -> gtk::Box {
        let explainer = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let how_to_read_label = gtk::Label::new(Some(&self.copy.how_to_read));
        how_to_read_label.set_xalign(0.0);
        how_to_read_label.add_css_class("dishy-alignment-section-label");
        explainer.append(&how_to_read_label);

        let how_to_read_body = gtk::Label::new(Some(&self.copy.how_to_read_body));
        how_to_read_body.set_xalign(0.0);
        how_to_read_body.set_wrap(true);
        how_to_read_body.add_css_class("dishy-alignment-explainer");
        explainer.append(&how_to_read_body);

        explainer
    }

    fn fact_row(&self, label: &str, value: &str) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_spacing(12);

        let label_widget = gtk::Label::new(Some(label));
        label_widget.set_xalign(0.0);
        label_widget.add_css_class("dishy-alignment-fact-label");
        row.append(&label_widget);

        let value_widget = gtk::Label::new(Some(value));
        value_widget.set_xalign(1.0);
        value_widget.set_hexpand(true);
        value_widget.add_css_class("dishy-alignment-fact-value");
        row.append(&value_widget);

        row
    }

    fn colored_fact_row(&self, label: &str, value: &str, color_class: &str) -> gtk::Box {
        let row = self.fact_row(label, value);
        if let Some(value_widget) = row
            .last_child()
            .and_then(|w| w.downcast::<gtk::Label>().ok())
        {
            value_widget.add_css_class(color_class);
        }
        row
    }
}

// CSS for the alignment page
const CSS: &str = r#"
.dishy-alignment-verdict {
  font-size: 13.5px;
  font-weight: 600;
}

.dishy-alignment-verdict-aligned {
  color: #22c55e;
}

.dishy-alignment-verdict-not-aligned {
  color: #fbbf24;
}

.dishy-alignment-verdict-not-ready {
  color: #94a3b8;
}

.dishy-alignment-instrument-card {
  background-color: alpha(currentColor, 0.03);
  border-radius: 8px;
  padding: 8px 14px 14px 14px;
}

.dishy-alignment-instrument-label {
  font-family: monospace;
  font-size: 10.5px;
  font-weight: 500;
  letter-spacing: 0.09em;
  text-transform: uppercase;
  color: alpha(currentColor, 0.66);
}

.dishy-alignment-section-label {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.09em;
  color: alpha(currentColor, 0.55);
}

.dishy-alignment-explainer {
  font-size: 13px;
  color: alpha(currentColor, 0.55);
}

.dishy-alignment-fact-label {
  font-size: 12px;
  color: alpha(currentColor, 0.66);
}

.dishy-alignment-fact-value {
  font-family: monospace;
  font-size: 13px;
  color: alpha(currentColor, 0.86);
}

.dishy-alignment-in-spec {
  color: #4ade80;
}

.dishy-alignment-out-of-spec {
  color: #fbbf24;
}
"#;

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

    /// Where a Cairo angle lands on screen, in y-down device space.
    fn screen_direction(angle_rad: f64) -> (f64, f64) {
        (angle_rad.cos(), angle_rad.sin())
    }

    // SPEC-ALIGN-005: the rotation wedge is only the pie slice from the dial
    // centre; it never reaches back to the compass letters drawn before it.
    #[test]
    fn the_rotation_wedge_is_a_slice_from_the_centre() {
        let reading = AlignmentReading {
            is_valid: true,
            is_aligned: true,
            boresight_azimuth_deg: 180.0,
            desired_azimuth_deg: 180.0,
            azimuth_tolerance_deg: 10.0,
            ..AlignmentReading::default()
        };
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 250, 250).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            InstrumentDrawing::new(reading, InstrumentType::Rotation).draw(
                &cr,
                250.0,
                250.0,
                (1.0, 1.0, 1.0),
            );
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.take_data().unwrap();
        let alpha = |x: usize, y: usize| data[y * stride + x * 4 + 3];
        // Between W and the centre, slightly below the horizontal: empty.
        assert_eq!(alpha(60, 135), 0, "no stray fill toward W");
        // Inside the slice, beside the needle toward S: filled.
        assert!(alpha(132, 200) > 0, "the wedge sits around south");
    }

    // SPEC-ALIGN-005: the pointer turns clockwise from north by the azimuth, and
    // the wedge is centred on the desired azimuth, both in the same y-down
    // convention the Electron SVG uses.
    #[test]
    fn rotation_dial_angles_follow_the_compass() {
        // The needle is drawn toward −y. Rotating (0, −1) by θ in y-down space
        // gives (sin θ, −cos θ).
        let needle = |azimuth: f64| {
            let angle = rotation_pointer_rad(Some(azimuth));
            (angle.sin(), -angle.cos())
        };
        let (x, y) = needle(0.0);
        assert!(x.abs() < 1e-9 && y < 0.0, "north is up");
        let (x, y) = needle(90.0);
        assert!(x > 0.0 && y.abs() < 1e-9, "east is right");
        let (x, y) = needle(180.0);
        assert!(x.abs() < 1e-9 && y > 0.0, "south is down");
        assert_eq!(rotation_pointer_rad(None), 0.0);

        let (start, end) = rotation_wedge_rad(90.0, 10.0);
        assert!(start < end, "the arc runs clockwise from start to end");
        let (x, y) = screen_direction((start + end) / 2.0);
        assert!(
            x > 0.0 && y.abs() < 1e-9,
            "a wedge for east sits on the right"
        );
        assert!(((end - start).to_degrees() - 20.0).abs() < 1e-9);
    }
}
