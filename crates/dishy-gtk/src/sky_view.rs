//! The sky page: a native GL canvas, its legend, and the controls over it.
//!
//! `dishy-sky` decides every vertex; `sky_render` owns the GPU objects. This
//! module places the canvas in the app, feeds it the recorder's status and
//! obstruction replies, and keeps the chrome translated. It starts no device
//! poll. The live constellation comes from the ephemeris worker, which reads
//! only the public element catalogue and runs only while this page is shown.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;
use std::time::Duration;

use dishy_core::telemetry::{DishObstructionMap, DishStatus};
use dishy_sky::tle_feed::{TleFeedError, TleFeedState};
use dishy_sky::{ObserverLocation, ObstructionLookup, SatelliteLook, Survey};
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;

use crate::ephemeris::{EphemerisRequest, EphemerisUpdate, EphemerisWorker, SkySnapshot};
use crate::i18n::Translator;
use crate::location_setup::LocationSetup;
use crate::sky_render::SatelliteLayer;

const CSS: &str = r#"
.dishy-sky-panel {
  background-color: rgba(0, 0, 0, 0.72);
  border-radius: 14px;
  padding: 11px 14px;
  color: #f4f6f9;
}
.dishy-sky-controls {
  background-color: rgba(0, 0, 0, 0.55);
  border-radius: 999px;
  padding: 3px;
}
.dishy-sky-caption {
  font-size: 12px;
  color: alpha(currentColor, 0.66);
}
.dishy-sky-stat-label {
  font-size: 11px;
  color: alpha(currentColor, 0.62);
}
.dishy-sky-stat-value {
  font-family: monospace;
  font-size: 14px;
  font-weight: 600;
}
.dishy-sky-legend {
  font-size: 12px;
  color: alpha(currentColor, 0.86);
}
.dishy-sky-clear { color: #ffffff; }
.dishy-sky-partial { color: #6e0f0f; }
.dishy-sky-obstructed { color: #f51e1e; }
.dishy-sky-unmapped { color: #7c7c7c; }
.dishy-sky-serving { color: #ffb040; }
.dishy-sky-error { color: #ff8b8b; }
.dishy-sky-site {
  font-size: 12px;
  font-weight: 500;
  color: alpha(currentColor, 0.66);
}
.dishy-sky-site-action {
  font-size: 11.5px;
  font-weight: 600;
  padding: 0 2px;
  min-height: 0;
  color: alpha(currentColor, 0.66);
}
.dishy-sky-callout {
  background-color: rgba(0, 0, 0, 0.58);
  color: #f4f6f9;
  border: 1px solid rgba(40, 40, 40, 0.59);
  border-radius: 8px;
  padding: 9px 12px 10px 12px;
}
.dishy-sky-callout-name {
  font-family: monospace;
  font-size: 11.5px;
  font-weight: 600;
}
.dishy-sky-callout-row {
  font-family: monospace;
  font-size: 11px;
}
.dishy-sky-callout-key { color: alpha(currentColor, 0.55); }
"#;

/// The strings this page paints, resolved once per locale.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyCopy {
    pub waiting: String,
    pub controls_hint: String,
    pub pause_rotation: String,
    pub resume_rotation: String,
    pub reset_view: String,
    pub hide_dome: String,
    pub show_dome: String,
    pub hide_unmapped: String,
    pub show_unmapped: String,
    pub legend_unmapped: String,
    pub legend_clear: String,
    pub legend_partial: String,
    pub legend_obstructed: String,
    pub sky_obstructed: String,
    pub observed_for: String,
    pub no_gl: String,
    pub no_location: String,
    pub change: String,
    pub set: String,
    pub clear: String,
    pub loading_feed: String,
    pub feed_offline: String,
    pub feed_unresponsive: String,
    pub legend_serving: String,
    pub overhead: String,
    pub overhead_tip: String,
    pub next_30_min: String,
    pub next_30_min_tip: String,
    pub likely_serving: String,
    pub likely_serving_tip: String,
    pub close_details: String,
    pub elevation: String,
    pub azimuth: String,
    pub altitude: String,
    pub distance: String,
    pub speed: String,
}

pub fn sky_copy(translator: &Translator) -> SkyCopy {
    let text = |key: &str| translator.text(key);
    SkyCopy {
        waiting: text("obstruction.waiting"),
        controls_hint: text("satellite.controlsHintWindow"),
        pause_rotation: text("obstruction.pauseRotation"),
        resume_rotation: text("obstruction.resumeRotation"),
        reset_view: text("satellite.resetView"),
        hide_dome: text("satellite.hideDome"),
        show_dome: text("satellite.showDome"),
        hide_unmapped: text("obstruction.hideUnmapped"),
        show_unmapped: text("obstruction.showUnmapped"),
        legend_unmapped: text("obstruction.legendUnmapped"),
        legend_clear: text("obstruction.legendClear"),
        legend_partial: text("obstruction.legendPartial"),
        legend_obstructed: text("obstruction.legendObstructions"),
        sky_obstructed: text("metrics.skyObstructed"),
        observed_for: text("obstruction.observedFor"),
        no_gl: text("satellite.noGtkGl"),
        no_location: text("satellite.noLocation"),
        change: text("satellite.change"),
        set: text("satellite.set"),
        clear: text("satellite.clear"),
        loading_feed: text("satellite.loadingFeed"),
        feed_offline: text("satellite.feedOffline"),
        feed_unresponsive: text("satellite.feedUnresponsive"),
        legend_serving: text("obstruction.legendServing"),
        overhead: text("obstruction.satellitesOverhead"),
        overhead_tip: text("obstruction.satellitesOverheadTip"),
        next_30_min: text("obstruction.next30Min"),
        next_30_min_tip: text("obstruction.next30MinTip"),
        likely_serving: text("obstruction.likelyServing"),
        likely_serving_tip: text("obstruction.likelyServingTip"),
        close_details: text("satellite.closeDetails"),
        elevation: text("satellite.elevation"),
        azimuth: text("satellite.azimuth"),
        altitude: text("satellite.altitude"),
        distance: text("satellite.distance"),
        speed: text("satellite.speed"),
    }
}

/// A stored snapshot as a survey the dome can draw, using the live status for
/// the dish's aim and the live map's frame when there is one (spec 0179).
pub fn snapshot_survey(
    snapshot: &dishy_core::store::ObstructionSnapshot,
    status: Option<&DishStatus>,
    frame: Option<&str>,
) -> Option<Survey> {
    let bytes = snapshot.packed_bytes()?;
    let grid_size = snapshot.grid_size as usize;
    let cells = dishy_sky::unpack_cells(&bytes, grid_size);
    Survey::from_packed(
        &cells,
        grid_size,
        snapshot.max_theta_deg.unwrap_or(80.0),
        boresight_azimuth_deg(status),
        status
            .map(|status| status.boresight_elevation_deg.0)
            .filter(|value| value.is_finite())
            .unwrap_or(90.0),
        frame,
        &dish_model_for(status),
    )
}

/// The scrubber's label: the snapshot's local time, or LIVE / OFFLINE at the
/// live stop.
pub fn scrub_label(
    snapshots: &[dishy_core::store::ObstructionSnapshot],
    index: Option<usize>,
    stale: bool,
    translator: &Translator,
) -> String {
    match index.and_then(|index| snapshots.get(index)) {
        Some(snapshot) => {
            use chrono::TimeZone;
            chrono::Local
                .timestamp_millis_opt(snapshot.taken_at_ms)
                .single()
                .map_or_else(String::new, |at| at.format("%H:%M").to_string())
        }
        None => translator
            .text(if stale {
                "obstruction.offline"
            } else {
                "obstruction.live"
            })
            .to_uppercase(),
    }
}

/// The live obstruction reply as the survey the dome draws, aimed by the
/// latest status. Shared by the Sky page and the dashboard card (spec 0187).
pub fn live_survey(status: Option<&DishStatus>, map: &DishObstructionMap) -> Option<Survey> {
    if map.num_rows == 0 || map.snr.is_empty() {
        return None;
    }
    let elevation = status
        .map(|status| status.boresight_elevation_deg.0)
        .filter(|value| value.is_finite())
        .unwrap_or(90.0);
    Survey::from_live(
        &map.snr,
        map.num_rows as usize,
        map.max_theta_deg.unwrap_or(80.0),
        boresight_azimuth_deg(status),
        elevation,
        map.map_reference_frame.as_deref(),
        &dish_model_for(status),
    )
}

/// The dome's colour key: unmapped, clear, partial, obstructed.
pub(crate) fn obstruction_legend(translator: &Translator) -> gtk::Box {
    install_css();
    let legend = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    legend.add_css_class("dishy-sky-legend");
    for (key, class) in [
        ("obstruction.legendUnmapped", "dishy-sky-unmapped"),
        ("obstruction.legendClear", "dishy-sky-clear"),
        ("obstruction.legendPartial", "dishy-sky-partial"),
        ("obstruction.legendObstructions", "dishy-sky-obstructed"),
    ] {
        legend.append(&legend_item(&translator.text(key), class));
    }
    legend
}

/// The site row's text: four decimals, as the Electron panel shows it.
pub fn format_site(location: Option<ObserverLocation>, translator: &Translator) -> String {
    match location {
        Some(location) => translator.text_with(
            "satellite.site",
            &[(
                "site",
                &format!(
                    "{:.4}, {:.4}",
                    location.latitude_deg, location.longitude_deg
                ),
            )],
        ),
        None => translator.text("satellite.noLocation"),
    }
}

/// The three constellation figures under the obstruction ones: overhead,
/// the 30-minute floor, and the likely serving satellite.
pub fn format_constellation_stats(
    snapshot: &SkySnapshot,
    translator: &Translator,
) -> (String, String, String) {
    let overhead = translator.text_with(
        "obstruction.overheadValue",
        &[
            ("inView", &snapshot.in_view.to_string()),
            ("serviceable", &snapshot.serviceable.to_string()),
        ],
    );
    let forecast = match snapshot.forecast_min_serviceable {
        Some(count) => translator.text_with(
            "obstruction.serviceableCount",
            &[("count", &count.to_string())],
        ),
        None => "…".into(),
    };
    let serving = match &snapshot.serving {
        Some(look) => translator.text_with(
            "obstruction.servingValue",
            &[
                ("name", &display_name(&look.name)),
                ("elevation", &format!("{:.0}", look.elevation_deg)),
                ("range", &format!("{:.0}", look.range_km)),
            ],
        ),
        None => translator.text("obstruction.noneAbove25"),
    };
    (overhead, forecast, serving)
}

/// Direct-to-cell satellites carry a `[DTC]` tag in the catalogue name. The
/// callout shows it as a badge, not as part of the name.
pub fn display_name(name: &str) -> String {
    name.replace("[DTC]", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The callout's five rows, in the Electron order.
pub fn callout_rows(look: &SatelliteLook, copy: &SkyCopy) -> [(String, String); 5] {
    [
        (
            copy.elevation.clone(),
            format!("{:.1}°", look.elevation_deg),
        ),
        (
            copy.azimuth.clone(),
            format!("{:.1}°", look.azimuth_deg.rem_euclid(360.0)),
        ),
        (copy.altitude.clone(), format!("{:.0} km", look.altitude_km)),
        (copy.distance.clone(), format!("{:.0} km", look.range_km)),
        (copy.speed.clone(), format!("{:.1} km/s", look.speed_km_s)),
    ]
}

/// What the feed line under the stats says, if anything.
pub fn feed_message(state: TleFeedState, copy: &SkyCopy) -> Option<String> {
    match state {
        TleFeedState::Loading => Some(copy.loading_feed.clone()),
        TleFeedState::Error(TleFeedError::Offline) => Some(copy.feed_offline.clone()),
        TleFeedState::Error(TleFeedError::Source) => Some(copy.feed_unresponsive.clone()),
        TleFeedState::LocationNeeded | TleFeedState::Active => None,
    }
}

/// The two obstruction figures the Electron card shows under the dome.
pub fn format_sky_stats(status: Option<&DishStatus>, translator: &Translator) -> (String, String) {
    let Some(stats) = status.and_then(|status| status.obstruction_stats.as_ref()) else {
        return ("—".into(), "—".into());
    };
    let fraction = stats.fraction_obstructed.0;
    let fraction = if fraction.is_finite() {
        fraction.max(0.0)
    } else {
        0.0
    };
    let hours = if stats.valid_s.0.is_finite() {
        stats.valid_s.0.max(0.0) / 3_600.0
    } else {
        0.0
    };
    (
        format!("{:.2}%", fraction * 100.0),
        translator.text_with(
            "obstruction.observedHours",
            &[("hours", &format!("{hours:.1}"))],
        ),
    )
}

/// The kit identity the obstruction frame fallback depends on. Prefix tests and
/// their order match upstream `lib/dishMesh.ts`; only the Mini distinction changes any
/// geometry here, but the full table keeps this honest for later milestones.
pub fn dish_model_for(status: Option<&DishStatus>) -> String {
    let Some(status) = status else {
        return "unknown".into();
    };
    let hardware = status
        .device_info
        .as_ref()
        .and_then(|info| info.hardware_version.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    let hardware = hardware.strip_prefix("rev_").unwrap_or(&hardware);
    if hardware.is_empty() {
        return "unknown".into();
    }
    let motorised = status.has_actuators.as_deref() == Some("HAS_ACTUATORS_YES");
    if hardware.contains("aviation") {
        "aviation"
    } else if hardware.starts_with("hp") {
        if motorised {
            "performanceGen1"
        } else {
            "performanceGen2"
        }
    } else if hardware.starts_with("rev3") || hardware.starts_with("dishy") {
        "rev3Rectangular"
    } else if hardware.starts_with("rev1") || hardware.starts_with("rev2") {
        "rev2Circular"
    } else if hardware.starts_with("rev4_hp") {
        "performanceGen3"
    } else if hardware.starts_with("rev4") {
        "rev4Standard"
    } else if hardware.starts_with("rev5") {
        "rev5Standard"
    } else if hardware.starts_with("mini1") {
        "mini1"
    } else if hardware.starts_with("mini2") {
        "mini2"
    } else {
        "unknown"
    }
    .to_string()
}

#[derive(Clone)]
pub struct SkyPage {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Stack,
    area: gtk::GLArea,
    translator: Translator,
    copy: SkyCopy,
    status: RefCell<Option<DishStatus>>,
    obstruction: RefCell<Option<DishObstructionMap>>,
    survey: RefCell<Option<Survey>>,
    canvas: crate::sky_canvas::SkyCanvas,
    sky_label: gtk::Label,
    observed_label: gtk::Label,
    gl_error: gtk::Label,
    rotation_button: gtk::Button,
    constellation: Constellation,
    lapse: TimeLapse,
}

/// The time-lapse scrubber and the snapshots it walks (spec 0179).
struct TimeLapse {
    row: gtk::Box,
    scale: gtk::Scale,
    label: gtk::Label,
    note: gtk::Label,
    snapshots: RefCell<Vec<dishy_core::store::ObstructionSnapshot>>,
    /// `Some(index)` while viewing a snapshot; `None` is live.
    index: Cell<Option<usize>>,
    history: RefCell<Option<Survey>>,
    commands: RefCell<Option<std::sync::mpsc::Sender<crate::recorder::RecorderCommand>>>,
    stale: Cell<bool>,
}

/// Told when the user saves or clears a location, so the host persists it.
type LocationHandler = Box<dyn Fn(Option<ObserverLocation>)>;

/// The live constellation's state and widgets.
struct Constellation {
    worker: RefCell<Option<EphemerisWorker>>,
    location: Cell<Option<ObserverLocation>>,
    location_changed: RefCell<Option<LocationHandler>>,
    feed_state: Cell<TleFeedState>,
    snapshot: RefCell<SkySnapshot>,
    selected: RefCell<Option<String>>,
    site_label: gtk::Label,
    change_button: gtk::Button,
    clear_button: gtk::Button,
    setup_revealer: gtk::Revealer,
    setup: LocationSetup,
    serving_legend: gtk::Box,
    stats: gtk::Box,
    overhead_label: gtk::Label,
    forecast_label: gtk::Label,
    serving_label: gtk::Label,
    feed_label: gtk::Label,
    callout: gtk::Box,
    callout_name: gtk::Label,
    callout_dtc: gtk::Label,
    callout_values: Vec<gtk::Label>,
}

impl SkyPage {
    pub fn new(translator: &Translator) -> Self {
        install_css();
        let copy = sky_copy(translator);

        let canvas = crate::sky_canvas::SkyCanvas::new();
        let area = canvas.area().clone();

        let waiting = adw::StatusPage::builder()
            .title(copy.waiting.clone())
            .icon_name("globe-symbolic")
            .build();

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&area));

        let rotation_button = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .tooltip_text(copy.pause_rotation.clone())
            .build();
        let reset_button = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text(copy.reset_view.clone())
            .build();
        let dome_button = gtk::ToggleButton::builder()
            .icon_name("view-reveal-symbolic")
            .active(true)
            .tooltip_text(copy.hide_dome.clone())
            .build();
        let trim_button = gtk::ToggleButton::builder()
            .icon_name("view-fullscreen-symbolic")
            .active(false)
            .tooltip_text(copy.hide_unmapped.clone())
            .build();
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        controls.add_css_class("dishy-sky-controls");
        controls.append(&rotation_button);
        controls.append(&reset_button);
        controls.append(&dome_button);
        controls.append(&trim_button);
        controls.set_halign(gtk::Align::End);
        controls.set_valign(gtk::Align::Start);
        controls.set_margin_top(14);
        controls.set_margin_end(14);

        let gl_error = gtk::Label::builder()
            .label(copy.no_gl.clone())
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        gl_error.add_css_class("dishy-sky-error");

        let legend = obstruction_legend(translator);
        let serving_legend = legend_item(&copy.legend_serving, "dishy-sky-serving");
        serving_legend.set_visible(false);
        legend.append(&serving_legend);

        let sky_label = stat_value();
        let observed_label = stat_value();
        let stats = gtk::Box::new(gtk::Orientation::Horizontal, 28);
        stats.append(&stat_block(&copy.sky_obstructed, &sky_label));
        stats.append(&stat_block(&copy.observed_for, &observed_label));

        let overhead_label = stat_value();
        let forecast_label = stat_value();
        let serving_label = stat_value();
        serving_label.set_wrap(true);
        let constellation_stats = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let counts = gtk::Box::new(gtk::Orientation::Horizontal, 28);
        counts.append(&tipped(
            stat_block(&copy.overhead, &overhead_label),
            &copy.overhead_tip,
        ));
        counts.append(&tipped(
            stat_block(&copy.next_30_min, &forecast_label),
            &copy.next_30_min_tip,
        ));
        constellation_stats.append(&counts);
        constellation_stats.append(&tipped(
            stat_block(&copy.likely_serving, &serving_label),
            &copy.likely_serving_tip,
        ));
        constellation_stats.set_visible(false);

        let feed_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        feed_label.add_css_class("dishy-sky-caption");

        let site_label = gtk::Label::builder()
            .label(copy.no_location.clone())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        site_label.add_css_class("dishy-sky-site");
        let change_button = site_action(&copy.set);
        let clear_button = site_action(&copy.clear);
        clear_button.set_visible(false);
        let site_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        site_row.append(&site_label);
        site_row.append(&change_button);
        site_row.append(&clear_button);

        let setup = LocationSetup::new(translator);
        let setup_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(280)
            .reveal_child(false)
            .child(setup.widget())
            .build();

        let callout_name = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        callout_name.add_css_class("dishy-sky-callout-name");
        let callout_dtc = gtk::Label::builder().label("DTC").visible(false).build();
        callout_dtc.add_css_class("caption");
        let callout_close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(copy.close_details.clone())
            .build();
        callout_close.add_css_class("flat");
        callout_close.add_css_class("circular");
        let callout_header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        callout_header.append(&callout_name);
        callout_header.append(&callout_dtc);
        callout_header.append(&callout_close);
        let callout_grid = gtk::Grid::builder()
            .column_spacing(14)
            .row_spacing(3)
            .build();
        let mut callout_values = Vec::new();
        let keys = [
            &copy.elevation,
            &copy.azimuth,
            &copy.altitude,
            &copy.distance,
            &copy.speed,
        ];
        for (row, key) in keys.into_iter().enumerate() {
            let key_label = gtk::Label::builder()
                .label(key.as_str())
                .xalign(0.0)
                .build();
            key_label.add_css_class("dishy-sky-callout-row");
            key_label.add_css_class("dishy-sky-callout-key");
            let value_label = gtk::Label::builder().xalign(1.0).hexpand(true).build();
            value_label.add_css_class("dishy-sky-callout-row");
            callout_grid.attach(&key_label, 0, row as i32, 1, 1);
            callout_grid.attach(&value_label, 1, row as i32, 1, 1);
            callout_values.push(value_label);
        }
        let callout = gtk::Box::new(gtk::Orientation::Vertical, 7);
        callout.add_css_class("dishy-sky-callout");
        callout.append(&callout_header);
        callout.append(&callout_grid);
        callout.set_halign(gtk::Align::Start);
        callout.set_valign(gtk::Align::Start);
        callout.set_size_request(176, -1);
        callout.set_visible(false);

        let hint = gtk::Label::builder()
            .label(copy.controls_hint.clone())
            .xalign(0.0)
            .wrap(true)
            .build();
        hint.add_css_class("dishy-sky-caption");

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        panel.add_css_class("dishy-sky-panel");
        panel.append(&gl_error);
        panel.append(&site_row);
        panel.append(&setup_revealer);
        panel.append(&legend);
        panel.append(&stats);
        panel.append(&constellation_stats);
        panel.append(&feed_label);

        let lapse_label = gtk::Label::builder()
            .label(translator.text("obstruction.timeLapse"))
            .build();
        lapse_label.add_css_class("dishy-sky-caption");
        let lapse_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
        lapse_scale.set_hexpand(true);
        lapse_scale.set_draw_value(false);
        lapse_scale.update_property(&[gtk::accessible::Property::Label(
            &translator.text("obstruction.timeLapse"),
        )]);
        let lapse_value = gtk::Label::builder().width_chars(8).xalign(0.0).build();
        lapse_value.add_css_class("dishy-sky-stat-label");
        let lapse_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        lapse_row.append(&lapse_label);
        lapse_row.append(&lapse_scale);
        lapse_row.append(&lapse_value);
        lapse_row.set_visible(false);
        let lapse_note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        lapse_note.add_css_class("dishy-sky-caption");
        panel.append(&lapse_row);
        panel.append(&lapse_note);
        panel.append(&hint);
        panel.set_halign(gtk::Align::Fill);
        panel.set_valign(gtk::Align::End);
        panel.set_margin_start(16);
        panel.set_margin_end(16);
        panel.set_margin_bottom(16);

        overlay.add_overlay(&controls);
        overlay.add_overlay(&panel);
        overlay.add_overlay(&callout);

        let root = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(220)
            .vexpand(true)
            .hexpand(true)
            .build();
        root.add_named(&waiting, Some("waiting"));
        root.add_named(&overlay, Some("sky"));
        root.set_visible_child_name("waiting");

        let inner = Rc::new(Inner {
            root,
            area,
            translator: translator.clone(),
            copy,
            status: RefCell::new(None),
            obstruction: RefCell::new(None),
            survey: RefCell::new(None),
            canvas,
            sky_label,
            observed_label,
            gl_error,
            rotation_button: rotation_button.clone(),
            lapse: TimeLapse {
                row: lapse_row,
                scale: lapse_scale,
                label: lapse_value,
                note: lapse_note,
                snapshots: RefCell::new(Vec::new()),
                index: Cell::new(None),
                history: RefCell::new(None),
                commands: RefCell::new(None),
                stale: Cell::new(false),
            },
            constellation: Constellation {
                worker: RefCell::new(None),
                location: Cell::new(None),
                location_changed: RefCell::new(None),
                feed_state: Cell::new(TleFeedState::LocationNeeded),
                snapshot: RefCell::new(SkySnapshot::empty()),
                selected: RefCell::new(None),
                site_label,
                change_button,
                clear_button,
                setup_revealer,
                setup,
                serving_legend,
                stats: constellation_stats,
                overhead_label,
                forecast_label,
                serving_label,
                feed_label,
                callout,
                callout_name,
                callout_dtc,
                callout_values,
            },
        });

        connect_canvas(&inner);
        connect_controls(&inner, &reset_button, &dome_button, &trim_button);
        connect_constellation(&inner, &callout_close);

        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.inner.root
    }

    pub fn set_status(&self, status: Option<DishStatus>) {
        *self.inner.status.borrow_mut() = status;
        self.inner.rebuild();
    }

    pub fn set_obstruction(&self, obstruction: Option<DishObstructionMap>) {
        *self.inner.obstruction.borrow_mut() = obstruction;
        self.inner.rebuild();
    }

    /// Seed a survey directly. Used by the integration test and by any future
    /// historical frame, where no live reply is in hand.
    pub fn set_survey(&self, survey: Option<Survey>) {
        *self.inner.survey.borrow_mut() = survey;
        self.inner.show_survey();
    }

    /// Whether this display could not open a GL context. The page falls back to
    /// a translated message rather than a blank canvas.
    pub fn has_gl_error(&self) -> bool {
        self.inner.canvas.is_failed()
    }

    /// Start the constellation worker. `cache_path` keeps the element
    /// catalogue between runs; the location is the saved one, if any.
    pub fn start_constellation(
        &self,
        cache_path: Option<PathBuf>,
        location: Option<ObserverLocation>,
    ) {
        match EphemerisWorker::start(cache_path) {
            Ok(worker) => {
                worker.send(EphemerisRequest::Demand(self.inner.root.is_mapped()));
                *self.inner.constellation.worker.borrow_mut() = Some(worker);
            }
            Err(error) => tracing::warn!("could not start the ephemeris worker: {error}"),
        }
        self.inner.send_obstruction();
        self.inner.apply_location(location);
    }

    /// The recorder handle, for the stored obstruction snapshots.
    pub fn set_command_sender(
        &self,
        sender: Option<std::sync::mpsc::Sender<crate::recorder::RecorderCommand>>,
    ) {
        *self.inner.lapse.commands.borrow_mut() = sender;
    }

    /// The hourly snapshots, oldest first. The scrubber gains a stop each.
    pub fn set_obstruction_snapshots(
        &self,
        snapshots: Vec<dishy_core::store::ObstructionSnapshot>,
    ) {
        let lapse = &self.inner.lapse;
        let count = snapshots.len();
        *lapse.snapshots.borrow_mut() = snapshots;
        lapse.row.set_visible(count > 0);
        lapse.scale.set_range(0.0, count as f64);
        lapse.scale.clear_marks();
        for stop in 0..=count {
            lapse
                .scale
                .add_mark(stop as f64, gtk::PositionType::Bottom, None);
        }
        if lapse.index.get().is_none() {
            lapse.scale.set_value(count as f64);
        }
        self.inner.scrub_to(lapse.index.get());
    }

    /// Scrub to a snapshot, or back to live, as the slider does.
    pub fn scrub_to(&self, index: Option<usize>) {
        let value = index.unwrap_or(self.inner.lapse.snapshots.borrow().len());
        self.inner.lapse.scale.set_value(value as f64);
        self.inner.scrub_to(index);
    }

    pub fn is_viewing_history(&self) -> bool {
        self.inner.lapse.index.get().is_some()
    }

    /// Mark the live stop OFFLINE while the dish is not answering.
    pub fn set_stale(&self, stale: bool) {
        self.inner.lapse.stale.set(stale);
        if self.inner.lapse.index.get().is_none() {
            self.inner.lapse.label.set_label(&scrub_label(
                &self.inner.lapse.snapshots.borrow(),
                None,
                stale,
                &self.inner.translator,
            ));
        }
    }

    /// Called with the new location when the user saves or clears one, so the
    /// host can persist it.
    pub fn connect_location_changed(&self, handler: impl Fn(Option<ObserverLocation>) + 'static) {
        *self.inner.constellation.location_changed.borrow_mut() = Some(Box::new(handler));
    }

    pub fn observer_location(&self) -> Option<ObserverLocation> {
        self.inner.constellation.location.get()
    }

    pub fn feed_state(&self) -> TleFeedState {
        self.inner.constellation.feed_state.get()
    }
}

impl Inner {
    fn rebuild(&self) {
        let obstruction = self.obstruction.borrow().clone();
        let status = self.status.borrow().clone();
        let Some(map) = obstruction else {
            return;
        };
        *self.survey.borrow_mut() = live_survey(status.as_ref(), &map);
        self.update_stats(status.as_ref());
        self.show_survey();
        self.send_obstruction();
    }

    /// The survey on screen: a scrubbed snapshot, else the live map.
    fn shown_survey(&self) -> Option<Survey> {
        self.lapse
            .history
            .borrow()
            .clone()
            .or_else(|| self.survey.borrow().clone())
    }

    fn show_survey(&self) {
        let Some(survey) = self.shown_survey() else {
            // Back from a snapshot with no live map yet: wait for one, unless
            // the GL error message is what the page is showing.
            if !self.canvas.is_failed() {
                self.root.set_visible_child_name("waiting");
            }
            return;
        };
        self.root.set_visible_child_name("sky");
        self.canvas
            .with_renderer(|renderer| renderer.set_survey(survey));
        self.area.queue_render();
    }

    /// Move the scrubber: `Some(index)` shows that snapshot and hides the live
    /// constellation; `None` returns to live (spec 0179).
    fn scrub_to(&self, index: Option<usize>) {
        let snapshots = self.lapse.snapshots.borrow().clone();
        let index = index.filter(|index| *index < snapshots.len());
        let history = index.and_then(|index| {
            let frame = self
                .obstruction
                .borrow()
                .as_ref()
                .and_then(|map| map.map_reference_frame.clone());
            snapshot_survey(
                &snapshots[index],
                self.status.borrow().as_ref(),
                frame.as_deref(),
            )
        });
        let index = index.filter(|_| history.is_some());
        self.lapse.index.set(index);
        *self.lapse.history.borrow_mut() = history;
        self.lapse.label.set_label(&scrub_label(
            &snapshots,
            index,
            self.lapse.stale.get(),
            &self.translator,
        ));
        match index {
            Some(index) => {
                use chrono::TimeZone;
                let when = chrono::Local
                    .timestamp_millis_opt(snapshots[index].taken_at_ms)
                    .single()
                    .map_or_else(String::new, |at| at.format("%-d %b %H:%M").to_string());
                self.lapse.note.set_label(
                    &self
                        .translator
                        .text_with("satellite.viewingAsOf", &[("when", &when)]),
                );
                self.lapse.note.set_visible(true);
                self.constellation.stats.set_visible(false);
                self.constellation.serving_legend.set_visible(false);
                self.constellation.callout.set_visible(false);
                self.canvas
                    .with_renderer(|renderer| renderer.set_satellites(SatelliteLayer::default()));
            }
            None => {
                self.lapse.note.set_visible(false);
                self.show_feed_state(self.constellation.feed_state.get());
                let snapshot = self.constellation.snapshot.borrow().clone();
                self.show_snapshot(snapshot);
            }
        }
        self.show_survey();
    }

    fn update_stats(&self, status: Option<&DishStatus>) {
        let (obstructed, observed) = format_sky_stats(status, &self.translator);
        self.sky_label.set_label(&obstructed);
        self.observed_label.set_label(&observed);
    }

    /// The worker judges "clear" from the same grid the dome draws.
    fn send_obstruction(&self) {
        if self.constellation.worker.borrow().is_none() {
            return;
        }
        let status = self.status.borrow();
        let lookup = self.obstruction.borrow().as_ref().and_then(|map| {
            ObstructionLookup::new(
                &map.snr,
                map.num_rows as usize,
                map.max_theta_deg.unwrap_or(80.0),
                map.map_reference_frame.as_deref(),
                &dish_model_for(status.as_ref()),
                boresight_azimuth_deg(status.as_ref()),
            )
        });
        self.send(EphemerisRequest::Obstruction(lookup));
    }

    fn send(&self, request: EphemerisRequest) {
        if let Some(worker) = self.constellation.worker.borrow().as_ref() {
            worker.send(request);
        }
    }

    fn apply_location(&self, location: Option<ObserverLocation>) {
        let constellation = &self.constellation;
        constellation.location.set(location);
        constellation
            .site_label
            .set_label(&format_site(location, &self.translator));
        constellation
            .change_button
            .set_label(if location.is_some() {
                &self.copy.change
            } else {
                &self.copy.set
            });
        constellation.clear_button.set_visible(location.is_some());
        constellation.setup_revealer.set_reveal_child(false);
        if location.is_none() {
            self.show_snapshot(SkySnapshot::empty());
        }
        self.send(EphemerisRequest::Observer(location));
    }

    /// A user's save or clear: apply it, then let the host persist it.
    fn choose_location(&self, location: Option<ObserverLocation>) {
        self.apply_location(location);
        if let Some(handler) = self.constellation.location_changed.borrow().as_ref() {
            handler(location);
        }
    }

    fn drain_ephemeris(&self) {
        loop {
            let update = self
                .constellation
                .worker
                .borrow()
                .as_ref()
                .and_then(EphemerisWorker::try_recv);
            let Some(update) = update else {
                return;
            };
            match update {
                EphemerisUpdate::Feed(state) => self.show_feed_state(state),
                EphemerisUpdate::Sky(snapshot) => {
                    // A reply computed before a clear must not repaint the sky.
                    if self.constellation.location.get().is_some() {
                        self.show_snapshot(snapshot);
                    }
                }
                EphemerisUpdate::IpLocation(result) => {
                    self.constellation.setup.ip_lookup_finished(result);
                }
            }
        }
    }

    fn show_feed_state(&self, state: TleFeedState) {
        let constellation = &self.constellation;
        constellation.feed_state.set(state);
        let message = feed_message(state, &self.copy);
        constellation
            .feed_label
            .set_label(message.as_deref().unwrap_or(""));
        constellation.feed_label.set_visible(message.is_some());
        // A scrubbed past sky shows no live constellation figures.
        let active = state == TleFeedState::Active && self.lapse.index.get().is_none();
        constellation.stats.set_visible(active);
        constellation.serving_legend.set_visible(active);
    }

    fn show_snapshot(&self, snapshot: SkySnapshot) {
        let constellation = &self.constellation;
        let (overhead, forecast, serving) = format_constellation_stats(&snapshot, &self.translator);
        constellation.overhead_label.set_label(&overhead);
        constellation.forecast_label.set_label(&forecast);
        constellation.serving_label.set_label(&serving);
        let selected = constellation
            .selected
            .borrow()
            .clone()
            .filter(|name| snapshot.satellites.iter().any(|look| &look.name == name));
        *constellation.selected.borrow_mut() = selected.clone();
        // A scrubbed past sky has no live satellites.
        if self.lapse.index.get().is_none() {
            self.canvas.with_renderer(|renderer| {
                renderer.set_satellites(satellite_layer(&snapshot, selected));
            });
        }
        *constellation.snapshot.borrow_mut() = snapshot;
        self.update_callout();
    }

    fn select(&self, name: Option<String>) {
        *self.constellation.selected.borrow_mut() = name;
        let snapshot = self.constellation.snapshot.borrow().clone();
        self.show_snapshot(snapshot);
    }

    /// Fill the callout for the tapped satellite. Its position follows the
    /// satellite every frame (`position_callout`).
    fn update_callout(&self) {
        let constellation = &self.constellation;
        let snapshot = constellation.snapshot.borrow();
        let selected = constellation.selected.borrow();
        let look = selected
            .as_ref()
            .and_then(|name| snapshot.satellites.iter().find(|look| &look.name == name));
        let Some(look) = look else {
            constellation.callout.set_visible(false);
            return;
        };
        let serving = snapshot
            .serving
            .as_ref()
            .is_some_and(|serving| serving.name == look.name);
        constellation
            .callout_name
            .set_label(&display_name(&look.name));
        constellation
            .callout_dtc
            .set_visible(look.name.contains("[DTC]"));
        if serving {
            constellation
                .callout_name
                .add_css_class("dishy-sky-serving");
        } else {
            constellation
                .callout_name
                .remove_css_class("dishy-sky-serving");
        }
        for (label, (_, value)) in constellation
            .callout_values
            .iter()
            .zip(callout_rows(look, &self.copy))
        {
            label.set_label(&value);
        }
        self.position_callout();
    }

    fn position_callout(&self) {
        let constellation = &self.constellation;
        let snapshot = constellation.snapshot.borrow();
        let selected = constellation.selected.borrow();
        let projected = selected
            .as_ref()
            .and_then(|name| snapshot.satellites.iter().find(|look| &look.name == name))
            .and_then(|look| {
                self.canvas
                    .with_renderer(|renderer| renderer.project_satellite(look))
                    .flatten()
            });
        let Some((x, y)) = projected else {
            constellation.callout.set_visible(false);
            return;
        };
        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        let callout_width = f64::from(constellation.callout.width().max(176));
        let callout_height = f64::from(constellation.callout.height().max(120));
        // Beside the satellite, kept inside the canvas.
        let left = (x + 14.0).min(width - callout_width - 8.0).max(8.0);
        let top = (y - callout_height / 2.0)
            .min(height - callout_height - 8.0)
            .max(8.0);
        constellation.callout.set_margin_start(left as i32);
        constellation.callout.set_margin_top(top as i32);
        constellation.callout.set_visible(true);
    }
}

/// The dome and the obstruction lookup must agree on the grid's rotation.
fn boresight_azimuth_deg(status: Option<&DishStatus>) -> f64 {
    status
        .map(|status| status.boresight_azimuth_deg.0)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn satellite_layer(snapshot: &SkySnapshot, selected: Option<String>) -> SatelliteLayer {
    SatelliteLayer {
        satellites: snapshot.satellites.clone(),
        serving: snapshot.serving.as_ref().map(|look| look.name.clone()),
        selected,
        sampled_at_us: snapshot.sampled_at_us,
    }
}

fn connect_constellation(inner: &Rc<Inner>, callout_close: &gtk::Button) {
    let constellation = &inner.constellation;

    let this = Rc::downgrade(inner);
    constellation.change_button.connect_clicked(move |_| {
        if let Some(inner) = this.upgrade() {
            let revealer = &inner.constellation.setup_revealer;
            let reveal = !revealer.reveals_child();
            if reveal {
                inner.constellation.setup.reset();
            }
            revealer.set_reveal_child(reveal);
        }
    });

    let this = Rc::downgrade(inner);
    constellation.clear_button.connect_clicked(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.choose_location(None);
        }
    });

    let this = Rc::downgrade(inner);
    constellation.setup.connect_saved(move |location| {
        if let Some(inner) = this.upgrade() {
            inner.choose_location(Some(location));
        }
    });

    let this = Rc::downgrade(inner);
    constellation.setup.connect_ip_requested(move || {
        if let Some(inner) = this.upgrade() {
            inner.send(EphemerisRequest::LookupIpLocation);
        }
    });

    let this = Rc::downgrade(inner);
    constellation.setup.connect_device_requested(move || {
        let this = this.clone();
        crate::location_portal::request_location(move |result| {
            if let Some(inner) = this.upgrade() {
                inner.constellation.setup.device_location_finished(result);
            }
        });
    });

    let this = Rc::downgrade(inner);
    callout_close.connect_clicked(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.select(None);
        }
    });

    // Nothing is fetched or propagated while the page is off screen.
    let this = Rc::downgrade(inner);
    inner.root.connect_map(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.send(EphemerisRequest::Demand(true));
            if let Some(sender) = inner.lapse.commands.borrow().as_ref() {
                let _ = sender.send(crate::recorder::RecorderCommand::GetObstructionSnapshots);
            }
        }
    });
    let this = Rc::downgrade(inner);
    inner.lapse.scale.connect_value_changed(move |scale| {
        if let Some(inner) = this.upgrade() {
            let stop = scale.value().round() as usize;
            let count = inner.lapse.snapshots.borrow().len();
            let index = (stop < count).then_some(stop);
            if index != inner.lapse.index.get() {
                inner.scrub_to(index);
            }
        }
    });
    let this = Rc::downgrade(inner);
    inner.root.connect_unmap(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.send(EphemerisRequest::Demand(false));
        }
    });

    let click = gtk::GestureClick::new();
    let this = Rc::downgrade(inner);
    click.connect_released(move |_, presses, x, y| {
        if presses != 1 {
            return;
        }
        if let Some(inner) = this.upgrade() {
            let picked = inner
                .canvas
                .with_renderer(|renderer| renderer.pick(x, y))
                .flatten();
            inner.select(picked);
        }
    });
    inner.area.add_controller(click);

    let this = Rc::downgrade(inner);
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(inner) = this.upgrade() else {
            return glib::ControlFlow::Break;
        };
        inner.drain_ephemeris();
        glib::ControlFlow::Continue
    });
}

/// What the shared canvas needs from this page: the scene to seed a new
/// renderer with, the GL error message, and the callout's frame tick.
fn connect_canvas(inner: &Rc<Inner>) {
    let this = Rc::downgrade(inner);
    inner.canvas.set_seed(move |renderer| {
        let Some(inner) = this.upgrade() else { return };
        if let Some(survey) = inner.shown_survey() {
            renderer.set_survey(survey);
        }
        if inner.lapse.index.get().is_none() {
            renderer.set_satellites(satellite_layer(
                &inner.constellation.snapshot.borrow(),
                inner.constellation.selected.borrow().clone(),
            ));
        }
    });
    let this = Rc::downgrade(inner);
    inner.canvas.connect_failed(move || {
        if let Some(inner) = this.upgrade() {
            inner.gl_error.set_visible(true);
            inner.root.set_visible_child_name("sky");
        }
    });
    let this = Rc::downgrade(inner);
    inner.canvas.connect_frame(move || {
        if let Some(inner) = this.upgrade()
            && inner.constellation.selected.borrow().is_some()
        {
            inner.position_callout();
        }
    });
}

fn connect_controls(
    inner: &Rc<Inner>,
    reset: &gtk::Button,
    dome: &gtk::ToggleButton,
    trim: &gtk::ToggleButton,
) {
    let this = Rc::downgrade(inner);
    inner.rotation_button.connect_clicked(move |button| {
        let Some(inner) = this.upgrade() else {
            return;
        };
        let rotating = inner
            .canvas
            .with_renderer(|renderer| renderer.toggle_rotation())
            .unwrap_or(true);
        button.set_icon_name(if rotating {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        button.set_tooltip_text(Some(if rotating {
            inner.copy.pause_rotation.as_str()
        } else {
            inner.copy.resume_rotation.as_str()
        }));
        inner.area.queue_render();
    });

    let this = Rc::downgrade(inner);
    reset.connect_clicked(move |_| {
        if let Some(inner) = this.upgrade() {
            inner.canvas.with_renderer(|renderer| renderer.reset_view());
            inner.area.queue_render();
        }
    });

    let this = Rc::downgrade(inner);
    dome.connect_toggled(move |button| {
        if let Some(inner) = this.upgrade() {
            let visible = inner
                .canvas
                .with_renderer(|renderer| renderer.toggle_dome())
                .unwrap_or(true);
            button.set_icon_name(if visible {
                "view-reveal-symbolic"
            } else {
                "view-conceal-symbolic"
            });
            button.set_tooltip_text(Some(if visible {
                inner.copy.hide_dome.as_str()
            } else {
                inner.copy.show_dome.as_str()
            }));
            inner.area.queue_render();
        }
    });

    let this = Rc::downgrade(inner);
    trim.connect_toggled(move |button| {
        if let Some(inner) = this.upgrade() {
            let trimming = button.is_active();
            inner
                .canvas
                .with_renderer(|renderer| renderer.set_trim_unmapped(trimming));
            button.set_tooltip_text(Some(if trimming {
                inner.copy.show_unmapped.as_str()
            } else {
                inner.copy.hide_unmapped.as_str()
            }));
            inner.area.queue_render();
        }
    });
}

fn legend_item(label: &str, class: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class(class);
    dot.add_css_class("dishy-sky-legend");
    let text = gtk::Label::new(Some(label));
    row.append(&dot);
    row.append(&text);
    row
}

pub(crate) fn stat_value() -> gtk::Label {
    let label = gtk::Label::builder().label("—").xalign(0.0).build();
    label.add_css_class("dishy-sky-stat-value");
    label
}

pub(crate) fn stat_block(label: &str, value: &gtk::Label) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let heading = gtk::Label::builder().label(label).xalign(0.0).build();
    heading.add_css_class("dishy-sky-stat-label");
    block.append(&heading);
    block.append(value);
    block
}

fn site_action(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("flat");
    button.add_css_class("dishy-sky-site-action");
    button
}

fn tipped(widget: gtk::Box, tip: &str) -> gtk::Box {
    widget.set_tooltip_text(Some(tip));
    widget
}

pub(crate) fn install_css() {
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

    fn status(hardware: &str, actuators: Option<&str>) -> DishStatus {
        serde_json::from_value(serde_json::json!({
            "deviceInfo": { "hardwareVersion": hardware },
            "hasActuators": actuators,
            "boresightAzimuthDeg": 120.0,
            "boresightElevationDeg": 65.0,
            "obstructionStats": { "fractionObstructed": 0.0123, "validS": 5400.0 }
        }))
        .expect("status")
    }

    // SPEC-SKY-006: every string on the page comes from the shared catalogs,
    // and the two figures use the Electron card's wording.
    #[test]
    fn page_copy_and_stats_are_translated_and_shared() {
        let english = Translator::new(Locale::En);
        let copy = sky_copy(&english);
        assert_eq!(copy.waiting, "Waiting for obstruction data…");
        assert_eq!(copy.legend_clear, "Clear view");
        assert_eq!(copy.controls_hint, "Drag to orbit · Scroll to zoom");
        let (obstructed, observed) = format_sky_stats(Some(&status("rev4_prod1", None)), &english);
        assert_eq!(obstructed, "1.23%");
        assert_eq!(observed, "1.5 h");
        assert_eq!(
            format_sky_stats(None, &english),
            ("—".to_string(), "—".to_string())
        );

        let spanish = Translator::new(Locale::Es);
        assert_ne!(sky_copy(&spanish).waiting, copy.waiting);
    }

    // SPEC-SKY-002, SPEC-SKY-006: the kit resolver feeds the frame fallback, and the
    // Mini family is the only one that changes it.
    #[test]
    fn mini_hardware_is_named_for_the_frame_fallback() {
        assert_eq!(dish_model_for(Some(&status("mini2_prod1", None))), "mini2");
        assert_eq!(
            dish_model_for(Some(&status("rev4_prod1", None))),
            "rev4Standard"
        );
        assert_eq!(dish_model_for(None), "unknown");
    }

    fn look(name: &str, elevation_deg: f64) -> SatelliteLook {
        SatelliteLook {
            name: name.into(),
            azimuth_deg: -10.0,
            elevation_deg,
            range_km: 612.4,
            altitude_km: 548.6,
            speed_km_s: 7.61,
            enu_km: [0.0; 3],
            enu_velocity_km_s: [0.0; 3],
            attitude: None,
        }
    }

    // SPEC-CONST-005, SPEC-CONST-009: the constellation figures, feed line, and callout
    // use the Electron wording and formats.
    #[test]
    fn constellation_copy_matches_the_electron_panel() {
        let english = Translator::new(Locale::En);
        let copy = sky_copy(&english);
        let snapshot = SkySnapshot {
            satellites: vec![look("STARLINK-1007 [DTC]", 61.6)],
            serving: Some(look("STARLINK-1007 [DTC]", 61.6)),
            in_view: 14,
            serviceable: 6,
            forecast_min_serviceable: None,
            sampled_at_us: 0,
        };
        let (overhead, forecast, serving) = format_constellation_stats(&snapshot, &english);
        assert_eq!(overhead, "14 · 6 serviceable");
        assert_eq!(forecast, "…");
        assert_eq!(serving, "STARLINK-1007 · 62° el · 612 km");

        assert_eq!(feed_message(TleFeedState::Active, &copy), None);
        assert_eq!(
            feed_message(TleFeedState::Loading, &copy),
            Some(copy.loading_feed.clone())
        );
        assert_eq!(
            feed_message(TleFeedState::Error(TleFeedError::Offline), &copy),
            Some(copy.feed_offline.clone())
        );

        let rows = callout_rows(&look("x", 61.64), &copy);
        assert_eq!(rows[0], ("elevation".to_string(), "61.6°".to_string()));
        assert_eq!(rows[1].1, "350.0°");
        assert_eq!(rows[4].1, "7.6 km/s");

        let site = ObserverLocation {
            latitude_deg: 6.52441,
            longitude_deg: 3.37919,
            altitude_m: 0.0,
        };
        assert_eq!(format_site(Some(site), &english), "site 6.5244, 3.3792");
        assert_eq!(format_site(None, &english), copy.no_location);
    }

    // SPEC-LAPSE-001, SPEC-LAPSE-002: snapshots decode into a survey with the
    // live kinds; the stops read as times, then LIVE or OFFLINE.
    #[test]
    fn time_lapse_stops_decode_and_label() {
        use dishy_core::store::ObstructionSnapshot;
        let english = Translator::new(Locale::En);
        let snapshot = ObstructionSnapshot {
            taken_at_ms: 1_800_000_000_000,
            grid_size: 2,
            packed_cells: dishy_core::store::pack_cells(&[1.0, 0.9, 0.5, -1.0]),
            max_theta_deg: Some(80.0),
        };
        let survey = snapshot_survey(&snapshot, None, None).expect("decodes");
        assert_eq!(survey.kind_at(0, 0), dishy_sky::DomePointKind::Clear);
        assert_eq!(survey.kind_at(1, 1), dishy_sky::DomePointKind::Unmapped);
        let broken = ObstructionSnapshot {
            packed_cells: "!".into(),
            ..snapshot.clone()
        };
        assert!(snapshot_survey(&broken, None, None).is_none());

        let snapshots = [snapshot];
        assert_eq!(scrub_label(&snapshots, None, false, &english), "LIVE");
        assert_eq!(scrub_label(&snapshots, None, true, &english), "OFFLINE");
        assert_eq!(
            scrub_label(&snapshots, Some(1), false, &english),
            "LIVE",
            "past the end is live"
        );
        let label = scrub_label(&snapshots, Some(0), false, &english);
        assert_eq!(label.len(), 5, "a local HH:MM time: {label}");
    }

    // SPEC-SKY-005: the canvas asks for the version the shaders target.
    #[test]
    fn the_canvas_targets_gl_3_3() {
        assert_eq!(crate::sky_render::required_version(), (3, 3));
    }
}
