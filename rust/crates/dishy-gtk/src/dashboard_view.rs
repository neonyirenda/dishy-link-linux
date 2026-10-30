//! The dashboard page: native GTK4 tiles and charts.
//!
//! `dashboard.rs` decides what the figures are; this module places them. The
//! page owns no network client and starts no timer of its own. The application
//! hands it the recorder's latest sample window, and the same `now` the tray
//! uses, so every surface describes one instant.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Once};

use dishy_core::telemetry::{DishStatus, TelemetrySample};
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::chart::{ChartCanvas, Sparkline};
use crate::dashboard::{
    DASHBOARD_WINDOWS, DEFAULT_WINDOW_MINUTES, DashboardSnapshot, format_latency,
    format_ping_success, format_relative_age, format_sky_obstructed, format_throughput,
    format_watts, kwh_per_day,
};
use crate::i18n::Translator;
use crate::readings::STALE_AFTER_MS;
use crate::stat_detail_model::DetailKind;
use crate::stat_detail_view::{DetailInputs, StatDetailWindow};
use crate::terminal_view::TerminalFacts;

const CSS: &str = r#"
.dishy-wordmark {
  font-size: 17px;
  font-weight: 700;
  letter-spacing: 0.16em;
}
.dishy-card-title {
  font-size: 16px;
  font-weight: 600;
}
.dishy-accent-link {
  color: #e0a422;
  font-weight: 600;
  padding-left: 6px;
  padding-right: 6px;
}
.dishy-callout {
  background-color: rgba(0, 0, 0, 0.7);
  border: 1px solid rgba(139, 151, 168, 0.14);
  border-radius: 10px;
  padding: 10px 14px;
}
.dishy-event-row {
  padding: 8px 2px;
}
.dishy-event-mark {
  font-size: 10px;
}
.dishy-event-outage {
  color: #f43f5e;
}
.dishy-event-degraded {
  color: #f09048;
}
.dishy-event-info {
  color: #7c7c7c;
}
.dishy-page {
  padding: 18px 24px 28px 24px;
}
.dishy-tile {
  padding: 15px 17px;
}
.dishy-tile-title {
  font-size: 13px;
  font-weight: 600;
  opacity: 0.72;
}
.dishy-tile-value {
  font-size: 28px;
  font-weight: 700;
}
.dishy-tile-unit {
  font-size: 14px;
  font-weight: 600;
  opacity: 0.7;
}
.dishy-tile-caption {
  font-size: 12px;
  opacity: 0.58;
}
.dishy-card {
  padding: 14px 16px 16px 16px;
}
.dishy-section-title {
  font-size: 16px;
  font-weight: 700;
}
.dishy-section-meta {
  font-size: 12px;
  opacity: 0.56;
}
.dishy-chart-empty {
  font-size: 13px;
  opacity: 0.55;
}
.dishy-legend {
  font-size: 12px;
  opacity: 0.82;
}
"#;
/// Electron's `max-[1080px]` breakpoint: below it, three tiles a row and the
/// cards stacked (spec 0187).
const NARROW_MAX_WIDTH: f64 = 1_080.0;

#[derive(Clone)]
pub struct DashboardPage {
    inner: Rc<Inner>,
}

struct Inner {
    root: gtk::Stack,
    banner: adw::Banner,
    translator: Translator,
    samples: RefCell<Arc<Vec<TelemetrySample>>>,
    status: RefCell<Option<DishStatus>>,
    window_minutes: Cell<u32>,
    last_now_ms: Cell<i64>,
    tiles: [StatTile; 6],
    throughput_chart: ChartCanvas,
    latency_chart: ChartCanvas,
    power_chart: ChartCanvas,
    power_meta: gtk::Label,
    commands: RefCell<Option<std::sync::mpsc::Sender<crate::recorder::RecorderCommand>>>,
    detail: RefCell<Option<StatDetailWindow>>,
    terminal_card: gtk::Box,
    terminal: TerminalFacts,
    terminal_window: RefCell<Option<(adw::Window, TerminalFacts)>>,
    last_snapshot: RefCell<Option<DashboardSnapshot>>,
    tiles_flow: gtk::FlowBox,
    columns: gtk::Grid,
    left_column: gtk::Box,
    right_column: gtk::Box,
    obstruction: crate::dashboard_cards::ObstructionCard,
    obstruction_map: RefCell<Option<dishy_core::telemetry::DishObstructionMap>>,
    events: crate::dashboard_cards::EventsCard,
}

/// The detail each clickable tile opens, by tile index (spec 0172).
const TILE_DETAILS: [Option<DetailKind>; 6] = [
    Some(DetailKind::Download),
    Some(DetailKind::Upload),
    Some(DetailKind::Latency),
    Some(DetailKind::Power),
    Some(DetailKind::PingSuccess),
    None,
];

fn detail_current(kind: DetailKind, snapshot: &DashboardSnapshot) -> f64 {
    match kind {
        DetailKind::Download => snapshot.download_bps,
        DetailKind::Upload => snapshot.upload_bps,
        DetailKind::Latency => snapshot.latency_ms.unwrap_or(0.0),
        DetailKind::Power => snapshot.power_w,
        DetailKind::PingSuccess => snapshot.ping_success_percent,
    }
}

impl DashboardPage {
    pub fn new(translator: &Translator, now_ms: i64) -> Self {
        install_css();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(220)
            .vexpand(true)
            .hexpand(true)
            .build();

        let searching = adw::StatusPage::builder()
            .title(translator.text("metrics.searching"))
            .description(translator.text("metrics.searchingHint"))
            .icon_name("network-wireless-symbolic")
            .build();
        stack.add_named(&searching, Some("searching"));

        let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
        page.add_css_class("dishy-page");
        page.set_hexpand(true);

        let banner = adw::Banner::new("");
        banner.set_revealed(false);
        page.append(&banner);

        let tiles_flow = gtk::FlowBox::builder()
            .column_spacing(14)
            .row_spacing(14)
            .min_children_per_line(3)
            .max_children_per_line(3)
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .build();
        tiles_flow.set_widget_name("dashboard-stats-grid");
        let tiles = [
            StatTile::new(
                translator.text("metrics.download"),
                crate::dashboard::DOWN_COLOR,
            ),
            StatTile::new(
                translator.text("metrics.upload"),
                crate::dashboard::UP_COLOR,
            ),
            StatTile::new(
                translator.text("metrics.latency"),
                crate::dashboard::LATENCY_COLOR,
            ),
            StatTile::new(
                translator.text("metrics.powerDraw"),
                crate::dashboard::POWER_COLOR,
            ),
            StatTile::new(
                translator.text("metrics.pingSuccess"),
                crate::dashboard::LATENCY_COLOR,
            ),
            StatTile::new(
                translator.text("metrics.skyObstructed"),
                crate::dashboard::POWER_COLOR,
            ),
        ];
        for (index, tile) in tiles.iter().enumerate() {
            tiles_flow.insert(&tile.root, index as i32);
            tile.chevron.set_visible(TILE_DETAILS[index].is_some());
        }
        page.append(&tiles_flow);

        let throughput = section_card(&translator.text("metrics.throughput"), None);
        let range_control = range_control(translator, DEFAULT_WINDOW_MINUTES);
        throughput.header.append(&range_control.root);
        let throughput_chart = ChartCanvas::new(220, DEFAULT_WINDOW_MINUTES, translator);
        throughput.content.append(throughput_chart.widget());
        let legend = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        legend.set_halign(gtk::Align::Center);
        legend.append(&legend_item(
            &translator.text("metrics.download"),
            crate::dashboard::DOWN_COLOR,
        ));
        legend.append(&legend_item(
            &translator.text("metrics.upload"),
            crate::dashboard::UP_COLOR,
        ));
        legend.add_css_class("dishy-legend");
        throughput.content.append(&legend);

        let latency = section_card(
            &translator.text("metrics.latency"),
            Some(&translator.text("metrics.latencyMeta")),
        );
        let latency_chart = ChartCanvas::new(160, DEFAULT_WINDOW_MINUTES, translator);
        latency.content.append(latency_chart.widget());

        let power = section_card(
            &translator.text("metrics.powerDraw"),
            Some(&translator.text_with("metrics.powerMeta", &[("value", &kwh_per_day(0.0))])),
        );
        let power_chart = ChartCanvas::new(160, DEFAULT_WINDOW_MINUTES, translator);
        power.content.append(power_chart.widget());
        let power_meta = power.meta.clone().expect("power card has a meta label");

        // Electron's two columns: the charts on the left, the Obstructions and
        // Events cards on the right. `arrange` moves them for the breakpoint.
        let left_column = gtk::Box::new(gtk::Orientation::Vertical, 14);
        left_column.set_hexpand(true);
        left_column.append(&throughput.root);
        left_column.append(&latency.root);
        left_column.append(&power.root);
        let obstruction = crate::dashboard_cards::ObstructionCard::new(translator);
        let events = crate::dashboard_cards::EventsCard::new(translator);
        let right_column = gtk::Box::new(gtk::Orientation::Vertical, 14);
        right_column.set_hexpand(true);
        right_column.append(obstruction.widget());
        right_column.append(events.widget());
        let columns = gtk::Grid::builder()
            .column_spacing(14)
            .row_spacing(14)
            .column_homogeneous(true)
            .build();
        page.append(&columns);

        // The dish terminal card (spec 0173), from the status already polled.
        let terminal_expand = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text(translator.text("navigation.openTerminalView"))
            .valign(gtk::Align::Center)
            .build();
        terminal_expand.add_css_class("flat");
        terminal_expand.add_css_class("circular");
        let terminal = TerminalFacts::new(
            translator,
            Some(&terminal_expand),
            Some(&translator.text("navigation.dishTerminal")),
        );
        let terminal_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        terminal_card.add_css_class("card");
        terminal_card.add_css_class("dishy-card");
        terminal_card.set_visible(false);
        terminal_card.append(terminal.widget());
        page.append(&terminal_card);

        let clamp = adw::Clamp::builder()
            .maximum_size(1400)
            .child(&page)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        scrolled.add_css_class("dishy-page-scroller");
        scrolled.add_css_class("dishy-dashboard");
        let breakpoints = adw::BreakpointBin::builder()
            .width_request(360)
            .height_request(240)
            .child(&scrolled)
            .build();
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW_MAX_WIDTH,
            adw::LengthUnit::Px,
        ));
        breakpoints.add_breakpoint(narrow.clone());
        stack.add_named(&breakpoints, Some("dashboard"));
        stack.set_visible_child_name("searching");

        let inner = Rc::new(Inner {
            root: stack,
            banner,
            translator: translator.clone(),
            samples: RefCell::new(Arc::new(Vec::new())),
            status: RefCell::new(None),
            window_minutes: Cell::new(DEFAULT_WINDOW_MINUTES),
            last_now_ms: Cell::new(now_ms),
            tiles,
            throughput_chart,
            latency_chart,
            power_chart,
            power_meta,
            commands: RefCell::new(None),
            detail: RefCell::new(None),
            terminal_card,
            terminal,
            terminal_window: RefCell::new(None),
            last_snapshot: RefCell::new(None),
            tiles_flow,
            columns,
            left_column,
            right_column,
            obstruction,
            obstruction_map: RefCell::new(None),
            events,
        });
        inner.arrange(false);
        let weak = Rc::downgrade(&inner);
        narrow.connect_apply(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.arrange(true);
            }
        });
        let weak = Rc::downgrade(&inner);
        narrow.connect_unapply(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.arrange(false);
            }
        });
        // The event log is a local recorder read, made when the page shows.
        let weak = Rc::downgrade(&inner);
        inner.root.connect_map(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.request_events();
            }
        });

        let weak = Rc::downgrade(&inner);
        terminal_expand.connect_clicked(move |button| {
            if let Some(inner) = weak.upgrade() {
                let parent = button.root().and_downcast::<gtk::Window>();
                inner.open_terminal(parent.as_ref());
            }
        });

        for (index, kind) in TILE_DETAILS.into_iter().enumerate() {
            let Some(kind) = kind else { continue };
            let tile = &inner.tiles[index].root;
            tile.add_css_class("activatable");
            tile.set_cursor_from_name(Some("pointer"));
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(&inner);
            click.connect_released(move |gesture, _, _, _| {
                if let Some(inner) = weak.upgrade() {
                    let parent = gesture
                        .widget()
                        .and_then(|widget| widget.root())
                        .and_downcast::<gtk::Window>();
                    inner.open_detail(kind, parent.as_ref());
                }
            });
            tile.add_controller(click);
        }

        let weak = Rc::downgrade(&inner);
        range_control.connect_selected(move |minutes| {
            if let Some(inner) = weak.upgrade() {
                inner.set_window_minutes(minutes);
            }
        });

        Self { inner }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.inner.root
    }

    pub fn set_samples(&self, samples: Arc<Vec<TelemetrySample>>) {
        *self.inner.samples.borrow_mut() = samples;
    }

    pub fn set_status(&self, status: Option<DishStatus>) {
        *self.inner.status.borrow_mut() = status;
        self.inner.update_obstruction();
    }

    /// The obstruction reply the Sky page also draws (spec 0187).
    pub fn set_obstruction(&self, map: Option<dishy_core::telemetry::DishObstructionMap>) {
        *self.inner.obstruction_map.borrow_mut() = map;
        self.inner.update_obstruction();
    }

    pub fn obstruction_waiting(&self) -> bool {
        self.inner.obstruction.is_waiting()
    }

    pub fn connect_open_sky(&self, handler: impl Fn() + 'static) {
        self.inner.obstruction.connect_open_sky(handler);
    }

    pub fn activate_open_sky(&self) {
        self.inner.obstruction.activate_open_sky();
    }

    pub fn set_events(&self, snapshot: &crate::recorder::EventsSnapshot) {
        self.inner.events.set_events(snapshot);
    }

    pub fn event_row_count(&self) -> usize {
        self.inner.events.row_count()
    }

    /// Apply a breakpoint by hand, as the window width does.
    pub fn set_narrow(&self, narrow: bool) {
        self.inner.arrange(narrow);
    }

    pub fn is_two_column(&self) -> bool {
        self.inner.columns.query_child(&self.inner.right_column).0 == 2
    }

    /// Which tiles show the `›` that opens their detail.
    pub fn tile_chevrons(&self) -> [bool; 6] {
        std::array::from_fn(|index| self.inner.tiles[index].chevron.is_visible())
    }

    pub fn refresh(&self, now_ms: i64) {
        self.inner.refresh(now_ms);
    }

    pub fn window_minutes(&self) -> u32 {
        self.inner.window_minutes.get()
    }

    pub fn set_window_minutes(&self, minutes: u32) {
        self.inner.set_window_minutes(minutes);
    }

    /// The recorder handle, for the summaries a tile detail asks for.
    pub fn set_command_sender(
        &self,
        sender: Option<std::sync::mpsc::Sender<crate::recorder::RecorderCommand>>,
    ) {
        *self.inner.commands.borrow_mut() = sender;
    }

    /// Open the expanded terminal window, as its expand button does.
    pub fn open_terminal(&self) {
        self.inner.open_terminal(None);
    }

    pub fn terminal_visible(&self) -> bool {
        self.inner.terminal_card.is_visible()
    }

    pub fn open_detail(&self, kind: DetailKind) {
        self.inner.open_detail(kind, None);
    }

    pub fn open_detail_kind(&self) -> Option<DetailKind> {
        self.inner
            .detail
            .borrow()
            .as_ref()
            .filter(|detail| detail.is_open())
            .map(StatDetailWindow::kind)
    }

    pub fn deliver_detail_energy(&self, summary: Arc<dishy_core::usage::UsageSummary>) {
        if let Some(detail) = self.inner.detail.borrow().as_ref() {
            detail.set_energy(summary);
        }
    }

    pub fn deliver_latency_summary(
        &self,
        summary: Arc<dishy_core::latency_summary::LatencySummary>,
    ) {
        if let Some(detail) = self.inner.detail.borrow().as_ref() {
            detail.set_latency_summary(summary);
        }
    }
}

impl Inner {
    /// Wide: the charts take two of three columns beside the cards. Narrow:
    /// everything stacks, and three tiles share a row.
    fn arrange(&self, narrow: bool) {
        let columns = dashboard_tile_columns(narrow);
        self.tiles_flow.set_min_children_per_line(columns);
        self.tiles_flow.set_max_children_per_line(columns);
        for child in [&self.left_column, &self.right_column] {
            if child.parent().is_some() {
                self.columns.remove(child);
            }
        }
        if narrow {
            self.columns.attach(&self.left_column, 0, 0, 3, 1);
            self.columns.attach(&self.right_column, 0, 1, 3, 1);
        } else {
            self.columns.attach(&self.left_column, 0, 0, 2, 1);
            self.columns.attach(&self.right_column, 2, 0, 1, 1);
        }
    }

    fn update_obstruction(&self) {
        self.obstruction.update(
            self.status.borrow().as_ref(),
            self.obstruction_map.borrow().as_ref(),
        );
    }

    fn request_events(&self) {
        if let Some(sender) = self.commands.borrow().as_ref() {
            let _ = sender.send(crate::recorder::RecorderCommand::GetEvents);
        }
    }

    fn detail_inputs(&self, kind: DetailKind) -> DetailInputs {
        let snapshot = self.last_snapshot.borrow();
        DetailInputs {
            samples: self.samples.borrow().clone(),
            current: snapshot
                .as_ref()
                .map_or(0.0, |snapshot| detail_current(kind, snapshot)),
            now_ms: self.last_now_ms.get(),
            power_end_ms: snapshot
                .as_ref()
                .map_or(self.last_now_ms.get(), |snapshot| {
                    snapshot.power_window_end_ms
                }),
        }
    }

    fn open_terminal(&self, parent: Option<&gtk::Window>) {
        *self.terminal_window.borrow_mut() = Some(crate::terminal_view::open_terminal_window(
            parent,
            &self.translator,
        ));
        self.refresh_terminal();
    }

    /// Both terminal views from the latest status; marked stale with the age of
    /// the facts while the dish is not answering.
    fn refresh_terminal(&self) {
        let status = self.status.borrow();
        let Some(status) = status.as_ref() else {
            self.terminal_card.set_visible(false);
            return;
        };
        self.terminal_card.set_visible(true);
        let snapshot = self.last_snapshot.borrow();
        let stale_age = snapshot
            .as_ref()
            .filter(|snapshot| snapshot.stale)
            .map(|snapshot| {
                snapshot
                    .latest_age_ms
                    .map(|age| format_relative_age(age, &self.translator))
            });
        let now = self.last_now_ms.get();
        self.terminal.set(status, now, stale_age.clone());
        if let Some((window, facts)) = self.terminal_window.borrow().as_ref()
            && window.is_visible()
        {
            facts.set(status, now, stale_age);
        }
    }

    fn open_detail(&self, kind: DetailKind, parent: Option<&gtk::Window>) {
        let detail = StatDetailWindow::open(
            parent,
            kind,
            &self.translator,
            self.commands.borrow().clone(),
            self.detail_inputs(kind),
        );
        *self.detail.borrow_mut() = Some(detail);
    }

    fn set_window_minutes(&self, minutes: u32) {
        if !DASHBOARD_WINDOWS.contains(&minutes) || minutes == self.window_minutes.get() {
            return;
        }
        self.window_minutes.set(minutes);
        self.refresh(self.last_now_ms.get());
    }

    fn refresh(&self, now_ms: i64) {
        self.last_now_ms.set(now_ms);
        let samples = self.samples.borrow();
        let snapshot = DashboardSnapshot::build(
            &samples,
            now_ms,
            self.window_minutes.get(),
            &self.translator,
        );
        let status = self.status.borrow();
        let mut snapshot = snapshot;
        if let Some(stats) = status
            .as_ref()
            .and_then(|status| status.obstruction_stats.as_ref())
        {
            let fraction = stats.fraction_obstructed.0;
            if fraction.is_finite() {
                snapshot.sky_obstructed_percent = Some((fraction * 100.0).max(0.0));
            }
        }

        if snapshot.latest.is_none() {
            self.root.set_visible_child_name("searching");
            return;
        }
        self.root.set_visible_child_name("dashboard");
        self.update_banner(&snapshot);
        self.update_tiles(&snapshot);
        self.throughput_chart.set_data(
            snapshot.throughput.clone(),
            snapshot.window_minutes,
            snapshot.now_ms,
        );
        self.latency_chart.set_data(
            snapshot.latency.clone(),
            snapshot.window_minutes,
            snapshot.now_ms,
        );
        self.power_chart.set_data(
            snapshot.power.clone(),
            snapshot.window_minutes,
            snapshot.power_window_end_ms,
        );
        self.power_meta.set_label(&self.translator.text_with(
            "metrics.powerMeta",
            &[("value", &kwh_per_day(snapshot.average_power_w))],
        ));
        drop(samples);
        drop(status);
        *self.last_snapshot.borrow_mut() = Some(snapshot);
        self.refresh_terminal();
        let detail = self.detail.borrow().clone();
        if let Some(detail) = detail.filter(StatDetailWindow::is_open) {
            detail.update(self.detail_inputs(detail.kind()));
        }
    }

    fn update_banner(&self, snapshot: &DashboardSnapshot) {
        if !snapshot.stale {
            self.banner.set_revealed(false);
            return;
        }
        let age = format_relative_age(
            snapshot.latest_age_ms.unwrap_or(STALE_AFTER_MS),
            &self.translator,
        );
        let title = format!(
            "{} · {}",
            self.translator.text("status.dishUnreachable"),
            self.translator
                .text_with("terminal.fromAgo", &[("when", &age)])
        );
        self.banner.set_title(&title);
        self.banner.set_revealed(true);
    }

    fn update_tiles(&self, snapshot: &DashboardSnapshot) {
        let (down_value, down_unit) = format_throughput(snapshot.download_bps);
        self.tiles[0].set(
            &down_value,
            &down_unit,
            &self.translator.text("metrics.currentTraffic"),
            snapshot.sparklines.downlink.clone(),
        );

        let (up_value, up_unit) = format_throughput(snapshot.upload_bps);
        self.tiles[1].set(
            &up_value,
            &up_unit,
            &self.translator.text("metrics.currentTraffic"),
            snapshot.sparklines.uplink.clone(),
        );

        let (latency_value, latency_unit) = format_latency(snapshot.latency_ms);
        let latency_caption = match snapshot.latency_quality {
            Some(quality) => format!(
                "{}: {}, {} {}",
                self.translator.text("metrics.quality"),
                quality.score,
                self.translator.text("metrics.grade"),
                quality.grade
            ),
            None => self.translator.text("metrics.popPingLive"),
        };
        self.tiles[2].set(
            &latency_value,
            &latency_unit,
            &latency_caption,
            snapshot.sparklines.latency.clone(),
        );

        let (power_value, power_unit) = format_watts(snapshot.power_w);
        self.tiles[3].set(
            &power_value,
            &power_unit,
            &self.translator.text("metrics.currentDraw"),
            snapshot.sparklines.power.clone(),
        );

        let (ping_value, ping_unit) = format_ping_success(snapshot.ping_success_percent);
        self.tiles[4].set(
            &ping_value,
            &ping_unit,
            &self.translator.text("metrics.lastMinute"),
            snapshot.sparklines.ping_success.clone(),
        );

        let (sky_value, sky_unit) = format_sky_obstructed(snapshot.sky_obstructed_percent);
        let sky_caption = self
            .status
            .borrow()
            .as_ref()
            .and_then(|status| status.obstruction_stats.as_ref())
            .and_then(|stats| stats.patches_valid.as_ref())
            .filter(|patches| patches.0.is_finite() && patches.0 > 0.0)
            .map(|patches| {
                self.translator.text_with(
                    "metrics.patchesMapped",
                    &[("count", &format!("{:.0}", patches.0))],
                )
            })
            .unwrap_or_else(|| self.translator.text("metrics.allTimeView"));
        self.tiles[5].set(&sky_value, &sky_unit, &sky_caption, Vec::new());
    }
}

#[derive(Clone)]
struct StatTile {
    root: gtk::Box,
    chevron: gtk::Image,
    value: gtk::Label,
    unit: gtk::Label,
    caption: gtk::Label,
    sparkline: Sparkline,
}

impl StatTile {
    fn new(title: String, color: (f64, f64, f64)) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("card");
        root.add_css_class("dishy-tile");
        root.set_hexpand(true);

        let title_label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        title_label.add_css_class("dishy-tile-title");
        title_label.set_hexpand(true);
        // The `›` Electron shows on a tile that opens its detail.
        let chevron = gtk::Image::from_icon_name("go-next-symbolic");
        chevron.set_pixel_size(12);
        chevron.add_css_class("dim-label");
        chevron.set_visible(false);
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        title_row.append(&title_label);
        title_row.append(&chevron);

        let value_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let value = gtk::Label::builder()
            .label("—")
            .xalign(0.0)
            .halign(gtk::Align::Start)
            .build();
        value.add_css_class("dishy-tile-value");
        let unit = gtk::Label::builder()
            .label("")
            .xalign(0.0)
            .valign(gtk::Align::End)
            .margin_bottom(4)
            .build();
        unit.add_css_class("dishy-tile-unit");
        let sparkline = Sparkline::new(color);
        value_row.append(&value);
        value_row.append(&unit);
        value_row.append(sparkline.widget());

        let caption = gtk::Label::builder()
            .label("")
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .build();
        caption.add_css_class("dishy-tile-caption");

        root.append(&title_row);
        root.append(&value_row);
        root.append(&caption);

        Self {
            root,
            chevron,
            value,
            unit,
            caption,
            sparkline,
        }
    }

    fn set(&self, value: &str, unit: &str, caption: &str, sparkline: Vec<Option<f64>>) {
        self.value.set_label(value);
        self.unit.set_label(unit);
        self.caption.set_label(caption);
        self.caption.set_tooltip_text(Some(caption));
        self.sparkline.set_values(sparkline);
    }
}

struct SectionCard {
    root: gtk::Box,
    header: gtk::Box,
    content: gtk::Box,
    meta: Option<gtk::Label>,
}

fn section_card(title: &str, meta: Option<&str>) -> SectionCard {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("card");
    card.add_css_class("dishy-card");
    card.set_hexpand(true);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .build();
    title_label.add_css_class("dishy-section-title");
    header.append(&title_label);
    let meta_label = meta.map(|meta| {
        let meta_label = gtk::Label::builder()
            .label(meta)
            .xalign(1.0)
            .halign(gtk::Align::End)
            .build();
        meta_label.add_css_class("dishy-section-meta");
        header.append(&meta_label);
        meta_label
    });
    card.append(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_hexpand(true);
    card.append(&content);
    SectionCard {
        root: card,
        header,
        content,
        meta: meta_label,
    }
}

#[derive(Clone)]
struct RangeControl {
    root: gtk::Box,
    buttons: [gtk::ToggleButton; 3],
}

impl RangeControl {
    fn connect_selected(&self, callback: impl Fn(u32) + 'static) {
        let callback = Rc::new(callback);
        for (index, button) in self.buttons.iter().enumerate() {
            let callback = callback.clone();
            button.connect_toggled(move |button| {
                if button.is_active() {
                    callback(DASHBOARD_WINDOWS[index]);
                }
            });
        }
    }
}

fn range_control(translator: &Translator, selected: u32) -> RangeControl {
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.add_css_class("linked");
    root.set_widget_name("dashboard-range-control");
    root.set_halign(gtk::Align::End);
    root.set_valign(gtk::Align::Center);
    root.set_tooltip_text(Some(&translator.text("metrics.chartTimeWindow")));

    let labels = ["ranges.15m", "ranges.1h", "ranges.6h"];
    let first = gtk::ToggleButton::with_label(&translator.text(labels[0]));
    first.set_active(selected == DASHBOARD_WINDOWS[0]);
    root.append(&first);

    let second = gtk::ToggleButton::with_label(&translator.text(labels[1]));
    second.set_group(Some(&first));
    second.set_active(selected == DASHBOARD_WINDOWS[1]);
    root.append(&second);

    let third = gtk::ToggleButton::with_label(&translator.text(labels[2]));
    third.set_group(Some(&first));
    third.set_active(selected == DASHBOARD_WINDOWS[2]);
    root.append(&third);

    RangeControl {
        root,
        buttons: [first, second, third],
    }
}

fn dashboard_tile_columns(narrow: bool) -> u32 {
    if narrow { 3 } else { 6 }
}

fn legend_item(label: &str, color: (f64, f64, f64)) -> gtk::Label {
    let markup = format!(
        "<span foreground=\"#{:02x}{:02x}{:02x}\">●</span> {}",
        (color.0 * 255.0).round() as u8,
        (color.1 * 255.0).round() as u8,
        (color.2 * 255.0).round() as u8,
        glib::markup_escape_text(label)
    );
    let label = gtk::Label::new(None);
    label.set_markup(&markup);
    label
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
mod responsive_grid_tests {
    use super::dashboard_tile_columns;

    #[test]
    fn dashboard_grid_uses_only_six_or_three_columns() {
        assert_eq!(dashboard_tile_columns(false), 6);
        assert_eq!(dashboard_tile_columns(true), 3);
    }
}
