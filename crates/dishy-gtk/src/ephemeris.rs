//! The constellation worker: fetches the public element catalogue, propagates
//! it, and hands the sky page finished look angles once a second.
//!
//! SGP4 over the whole Starlink catalogue takes tens of milliseconds, and the
//! 30-minute forecast several times that, so none of it runs on the GTK main
//! thread. The worker computes only while the page asks for it (`Demand`),
//! touches neither the dish nor the router, and makes its HTTP requests
//! through `dishy-core::external`.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use chrono::Utc;
use dishy_core::external::{ExternalClient, ExternalFetchError};
use dishy_sky::tle_feed::{TleFeed, TleFeedError, TleFeedState};
use dishy_sky::{
    ObserverLocation, ObstructionLookup, SERVING_ELEVATION_FLOOR_DEG, SatelliteLook, ServingHold,
    StarlinkTracker, choose_serving_candidate, is_unobstructed,
};
use serde::{Deserialize, Serialize};

/// The Electron view refreshes its figures at 1 Hz.
const STATS_INTERVAL: Duration = Duration::from_secs(1);
/// And sweeps the catalogue for the forecast once a minute.
const FORECAST_INTERVAL_MS: i64 = 60_000;

pub enum EphemerisRequest {
    Observer(Option<ObserverLocation>),
    /// Whether the sky page is on screen. Nothing is fetched or propagated
    /// while it is not.
    Demand(bool),
    Obstruction(Option<ObstructionLookup>),
    LookupIpLocation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkySnapshot {
    pub satellites: Vec<SatelliteLook>,
    pub serving: Option<SatelliteLook>,
    pub in_view: usize,
    pub serviceable: usize,
    /// `None` until the first forecast sweep finishes.
    pub forecast_min_serviceable: Option<usize>,
    /// When the look angles were sampled, on GLib's monotonic clock, so the
    /// renderer can carry them forward between samples (spec 0182).
    pub sampled_at_us: i64,
}

pub enum EphemerisUpdate {
    Feed(TleFeedState),
    Sky(SkySnapshot),
    IpLocation(Result<ObserverLocation, ()>),
}

pub struct EphemerisWorker {
    requests: Sender<EphemerisRequest>,
    updates: Receiver<EphemerisUpdate>,
}

impl EphemerisWorker {
    /// Start the worker. It stops when this handle is dropped.
    pub fn start(cache_path: Option<PathBuf>) -> std::io::Result<Self> {
        let (requests, request_receiver) = mpsc::channel();
        let (updates, update_receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("dishy-ephemeris".into())
            .spawn(move || run(request_receiver, updates, cache_path))?;
        Ok(Self {
            requests,
            updates: update_receiver,
        })
    }

    pub fn send(&self, request: EphemerisRequest) {
        let _ = self.requests.send(request);
    }

    pub fn try_recv(&self) -> Option<EphemerisUpdate> {
        self.updates.try_recv().ok()
    }
}

/// What the cache file holds. The field names match Electron's localStorage
/// entry, though the two hosts keep separate copies.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedCatalog {
    fetched_at_ms: i64,
    text: String,
}

struct Worker {
    updates: Sender<EphemerisUpdate>,
    cache_path: Option<PathBuf>,
    client: ExternalClient,
    runtime: Option<tokio::runtime::Runtime>,
    feed: TleFeed,
    tracker: Option<StarlinkTracker>,
    demand: bool,
    obstruction: Option<ObstructionLookup>,
    hold: Option<ServingHold>,
    forecast: Option<usize>,
    forecast_at_ms: Option<i64>,
    last_state: Option<TleFeedState>,
}

fn run(
    requests: Receiver<EphemerisRequest>,
    updates: Sender<EphemerisUpdate>,
    cache_path: Option<PathBuf>,
) {
    let feed = cache_path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<CachedCatalog>(&text).ok())
        .map(|cached| TleFeed::with_cached(&cached.text, cached.fetched_at_ms))
        .unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| tracing::warn!("ephemeris runtime unavailable: {error}"))
        .ok();
    let mut worker = Worker {
        updates,
        cache_path,
        client: ExternalClient::default(),
        runtime,
        feed,
        tracker: None,
        demand: false,
        obstruction: None,
        hold: None,
        forecast: None,
        forecast_at_ms: None,
        last_state: None,
    };
    loop {
        match requests.recv_timeout(STATS_INTERVAL) {
            Ok(request) => worker.handle(request),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Drain whatever else is queued before computing, so a burst of
        // requests costs one pass.
        while let Ok(request) = requests.try_recv() {
            worker.handle(request);
        }
        worker.tick();
    }
}

impl Worker {
    fn handle(&mut self, request: EphemerisRequest) {
        match request {
            EphemerisRequest::Observer(observer) => {
                if observer != self.feed.observer() {
                    self.feed.set_observer(observer);
                    self.tracker = None;
                    self.hold = None;
                    self.forecast = None;
                    self.forecast_at_ms = None;
                    if self.feed.observer().is_none() {
                        let _ = self
                            .updates
                            .send(EphemerisUpdate::Sky(SkySnapshot::empty()));
                    }
                }
            }
            EphemerisRequest::Demand(wanted) => self.demand = wanted,
            EphemerisRequest::Obstruction(lookup) => self.obstruction = lookup,
            EphemerisRequest::LookupIpLocation => {
                let result = self
                    .block_on(self.client.ip_location())
                    .and_then(|result| result.ok())
                    .map(|(latitude_deg, longitude_deg)| ObserverLocation {
                        latitude_deg,
                        longitude_deg,
                        altitude_m: 0.0,
                    })
                    .ok_or(());
                let _ = self.updates.send(EphemerisUpdate::IpLocation(result));
            }
        }
    }

    fn tick(&mut self) {
        self.publish_state();
        if !self.demand {
            return;
        }
        let now_ms = Utc::now().timestamp_millis();
        if self.feed.wants_fetch(now_ms) {
            self.fetch(now_ms);
        }
        if self.tracker.is_none()
            && let (Some(observer), Some(catalog)) = (self.feed.observer(), self.feed.catalog())
        {
            self.tracker = catalog.clone().tracker(observer).ok();
        }
        let Some(tracker) = self.tracker.as_ref() else {
            return;
        };
        let at = Utc::now().naive_utc();
        if self
            .forecast_at_ms
            .is_none_or(|last| now_ms - last >= FORECAST_INTERVAL_MS)
        {
            self.forecast = Some(tracker.forecast_min_serviceable(at));
            self.forecast_at_ms = Some(now_ms);
        }
        let satellites = tracker.look_angles_at(at);
        let sampled_at_us = gtk::glib::monotonic_time();
        let obstruction = self.obstruction.as_ref();
        let (serving, hold) =
            choose_serving_candidate(&satellites, now_ms, self.hold.as_ref(), |look| {
                is_unobstructed(obstruction, look)
            });
        let serving = serving.cloned();
        self.hold = hold;
        let snapshot = SkySnapshot {
            in_view: satellites.len(),
            serviceable: satellites
                .iter()
                .filter(|look| look.elevation_deg >= SERVING_ELEVATION_FLOOR_DEG)
                .count(),
            serving,
            satellites,
            forecast_min_serviceable: self.forecast,
            sampled_at_us,
        };
        let _ = self.updates.send(EphemerisUpdate::Sky(snapshot));
    }

    fn fetch(&mut self, now_ms: i64) {
        self.feed.begin_fetch();
        self.publish_state();
        let result = self
            .block_on(self.client.starlink_tles())
            .unwrap_or_else(|| Err(ExternalFetchError::Offline("no runtime".into())));
        let text = result.as_ref().ok().cloned();
        let accepted = self.feed.finish_fetch(
            result.map_err(|error| match error {
                ExternalFetchError::Offline(_) => TleFeedError::Offline,
                ExternalFetchError::Source(_) => TleFeedError::Source,
            }),
            now_ms,
        );
        if accepted {
            self.tracker = None;
            if let (Some(path), Some(text)) = (self.cache_path.as_ref(), text) {
                write_cache(
                    path,
                    &CachedCatalog {
                        fetched_at_ms: now_ms,
                        text,
                    },
                );
            }
        }
        self.publish_state();
    }

    fn publish_state(&mut self) {
        let state = self.feed.state();
        if self.last_state != Some(state) {
            self.last_state = Some(state);
            let _ = self.updates.send(EphemerisUpdate::Feed(state));
        }
    }

    fn block_on<F: std::future::Future>(&self, future: F) -> Option<F::Output> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.block_on(future))
    }
}

impl SkySnapshot {
    pub fn empty() -> Self {
        Self {
            satellites: Vec::new(),
            serving: None,
            in_view: 0,
            serviceable: 0,
            forecast_min_serviceable: None,
            sampled_at_us: 0,
        }
    }
}

/// Atomic, like every other host file: a torn cache would only cost a refetch,
/// but there is no reason to write one.
fn write_cache(path: &std::path::Path, cached: &CachedCatalog) {
    let Ok(text) = serde_json::to_string(cached) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, text).is_ok() {
        let _ = std::fs::rename(temporary, path);
    }
}
