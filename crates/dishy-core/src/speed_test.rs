//! The speed test: throughput through the real link against Cloudflare.
//!
//! Port of upstream `lib/speedTest.ts`. Throughput only: timing a fetch measures
//! DNS/TCP/TLS setup and edge processing, not the link, so latency, jitter, and
//! loss come from the dish's own per-second samples over the run's window
//! (`link_quality`). The dish's speed-test RPCs are unimplemented on current
//! firmware, and nothing here talks to the dish or router.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::calendar::now_ms;
use crate::telemetry::TelemetrySample;

/// Upload bodies are produced in pieces this size, and each piece is counted
/// as the HTTP stack takes it. That tracks bytes drained to the socket under
/// backpressure, as XHR's upload progress does in Electron.
const UPLOAD_PIECE_BYTES: usize = 64 * 1024;

pub const CLOUDFLARE_BASE_URL: &str = "https://speed.cloudflare.com";

/// Several concurrent streams saturate the link. One TCP stream over
/// Starlink's latency is limited by slow start and under-reads.
pub const STREAMS: usize = 6;

/// The gauge's full sweep and its tick labels.
pub const GAUGE_MAX_MBPS: f64 = 500.0;
pub const GAUGE_TICKS: [u32; 8] = [0, 10, 25, 50, 100, 200, 350, 500];

/// Every duration and size a run uses. The defaults are Electron's; tests
/// shrink them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedTestConfig {
    /// Connection setup and slow start, excluded from the figure.
    pub ramp_ms: f64,
    /// How wide the post-ramp window must be before the live rate is shown.
    pub settle_ms: f64,
    /// The steady-state window, the same for both directions.
    pub measure_ms: f64,
    pub sample_ms: f64,
    /// Keep a phase's result on the gauge before moving on.
    pub hold_ms: f64,
    /// Let the needle ease back to zero between phases.
    pub rest_ms: f64,
    /// Request sizes, not budgets: a stream re-requests until its phase ends.
    pub download_chunk_bytes: usize,
    pub upload_chunk_bytes: usize,
}

impl Default for SpeedTestConfig {
    fn default() -> Self {
        Self {
            ramp_ms: 1_000.0,
            settle_ms: 900.0,
            measure_ms: 10_000.0,
            sample_ms: 250.0,
            hold_ms: 900.0,
            rest_ms: 1_100.0,
            download_chunk_bytes: 25_000_000,
            upload_chunk_bytes: 8_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedTestPhase {
    Idle,
    Download,
    Upload,
    Resting,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedTestProgress {
    pub phase: SpeedTestPhase,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    /// Wall-clock span of the measurement, for reading latency off the dish's
    /// samples. It ends when upload ends, before the closing hold.
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

impl SpeedTestProgress {
    pub const IDLE: Self = Self {
        phase: SpeedTestPhase::Idle,
        download_mbps: None,
        upload_mbps: None,
        started_at_ms: None,
        ended_at_ms: None,
    };
}

/// Measures one phase: a fixed ramp, then a fixed window. The rate is the
/// cumulative average since the ramp ended. It converges and cannot freefall,
/// because its numerator and denominator only grow. Per-interval rates are far
/// too bursty to show.
#[derive(Debug, Clone)]
pub struct PhaseMeter {
    config: SpeedTestConfig,
    started_ms: f64,
    ramp: Option<(u64, f64)>,
}

impl PhaseMeter {
    pub fn new(started_ms: f64, config: SpeedTestConfig) -> Self {
        Self {
            config,
            started_ms,
            ramp: None,
        }
    }

    /// Take a sample. Marks the end of the ramp on the first sample past it,
    /// and returns the live rate once the post-ramp window is wide enough.
    pub fn sample(&mut self, now_ms: f64, bytes: u64) -> Option<f64> {
        let Some((_, ramp_at)) = self.ramp else {
            if now_ms - self.started_ms >= self.config.ramp_ms {
                self.ramp = Some((bytes, now_ms));
            }
            return None;
        };
        (now_ms - ramp_at >= self.config.settle_ms).then(|| self.rate_since_ramp(now_ms, bytes))
    }

    pub fn is_over(&self, now_ms: f64) -> bool {
        now_ms - self.started_ms >= self.config.ramp_ms + self.config.measure_ms
    }

    /// The phase's figure, over the post-ramp window only.
    pub fn finish(&self, now_ms: f64, bytes: u64) -> f64 {
        self.rate_since_ramp(now_ms, bytes)
    }

    fn rate_since_ramp(&self, now_ms: f64, bytes: u64) -> f64 {
        let (mark_bytes, mark_at) = self.ramp.unwrap_or((0, self.started_ms));
        let seconds = (now_ms - mark_at) / 1_000.0;
        if seconds <= 0.0 {
            return 0.0;
        }
        bytes.saturating_sub(mark_bytes) as f64 * 8.0 / seconds / 1e6
    }
}

/// Cancels a run. Cloning shares the token.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled.
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

pub type ProgressHandler = Arc<dyn Fn(SpeedTestProgress) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SpeedTestClient {
    http: reqwest::Client,
    base_url: String,
    config: SpeedTestConfig,
}

impl Default for SpeedTestClient {
    fn default() -> Self {
        Self::with_base_url(CLOUDFLARE_BASE_URL, SpeedTestConfig::default())
    }
}

impl SpeedTestClient {
    pub fn with_base_url(base_url: &str, config: SpeedTestConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            config,
        }
    }

    /// Run one test, reporting progress as it goes. A cancelled run stops
    /// reporting: whoever cancelled is no longer listening.
    pub async fn run(&self, cancel: &CancelToken, on_progress: ProgressHandler) {
        let report = |progress: &SpeedTestProgress| {
            if !cancel.is_cancelled() {
                on_progress(progress.clone());
            }
        };
        let mut progress = SpeedTestProgress {
            phase: SpeedTestPhase::Download,
            started_at_ms: Some(now_ms()),
            ..SpeedTestProgress::IDLE
        };
        report(&progress);
        let outcome = async {
            let download = self
                .measure(Direction::Download, cancel, |mbps| {
                    progress.download_mbps = Some(mbps);
                    report(&progress);
                })
                .await?;
            progress.download_mbps = Some(download);
            report(&progress);
            self.pause(self.config.hold_ms, cancel).await?;

            // Upload starts with no value, so the needle eases down and rests.
            progress.phase = SpeedTestPhase::Upload;
            progress.upload_mbps = None;
            report(&progress);
            self.pause(self.config.rest_ms, cancel).await?;

            let upload = self
                .measure(Direction::Upload, cancel, |mbps| {
                    progress.upload_mbps = Some(mbps);
                    report(&progress);
                })
                .await?;
            progress.upload_mbps = Some(upload);
            // The window closes here. The hold and rest after it are idle time
            // and would drag post-test pings into the latency figures.
            progress.ended_at_ms = Some(now_ms());
            report(&progress);
            self.pause(self.config.hold_ms, cancel).await?;

            progress.phase = SpeedTestPhase::Resting;
            report(&progress);
            self.pause(self.config.rest_ms, cancel).await?;
            Ok::<(), Stop>(())
        }
        .await;
        match outcome {
            Ok(()) => progress.phase = SpeedTestPhase::Done,
            // Cancelling is not failing: nobody is left to tell.
            Err(Stop::Cancelled) => return,
            Err(Stop::NoBytes) => progress.phase = SpeedTestPhase::Error,
        }
        report(&progress);
    }

    async fn pause(&self, ms: f64, cancel: &CancelToken) -> Result<(), Stop> {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs_f64(ms / 1_000.0)) => {}
            () = cancel.cancelled() => {}
        }
        if cancel.is_cancelled() {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }

    /// One phase: streams run until the phase clock or a cancel stops them,
    /// and the sampler reports the live rate meanwhile.
    async fn measure(
        &self,
        direction: Direction,
        cancel: &CancelToken,
        mut on_live: impl FnMut(f64),
    ) -> Result<f64, Stop> {
        if cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        let bytes = Arc::new(AtomicU64::new(0));
        let stop = CancelToken::new();
        let streams: Vec<_> = (0..STREAMS)
            .map(|index| {
                let stream = Stream {
                    http: self.http.clone(),
                    base_url: self.base_url.clone(),
                    config: self.config,
                    bytes: bytes.clone(),
                    stop: stop.clone(),
                    index,
                };
                tokio::spawn(stream.run(direction))
            })
            .collect();

        let started = Instant::now();
        let elapsed_ms = || started.elapsed().as_secs_f64() * 1_000.0;
        let mut meter = PhaseMeter::new(0.0, self.config);
        let mut ticks =
            tokio::time::interval(Duration::from_secs_f64(self.config.sample_ms / 1_000.0));
        loop {
            tokio::select! {
                _ = ticks.tick() => {
                    let now = elapsed_ms();
                    if let Some(mbps) = meter.sample(now, bytes.load(Ordering::Relaxed)) {
                        on_live(mbps);
                    }
                    if meter.is_over(now) {
                        break;
                    }
                }
                () = cancel.cancelled() => break,
            }
        }
        stop.cancel();
        for stream in streams {
            let _ = stream.await;
        }
        if cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        let total = bytes.load(Ordering::Relaxed);
        if total == 0 {
            return Err(Stop::NoBytes);
        }
        Ok(meter.finish(elapsed_ms(), total))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Download,
    Upload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancelled,
    NoBytes,
}

/// One of the concurrent streams. It re-requests until told to stop. A
/// failed request ends the stream but keeps the bytes it delivered, so one
/// failure cannot fail the run.
struct Stream {
    http: reqwest::Client,
    base_url: String,
    config: SpeedTestConfig,
    bytes: Arc<AtomicU64>,
    stop: CancelToken,
    index: usize,
}

impl Stream {
    async fn run(self, direction: Direction) {
        let mut request = 0_u64;
        while !self.stop.is_cancelled() {
            request += 1;
            let transfer = async {
                match direction {
                    Direction::Download => self.download(request).await,
                    Direction::Upload => self.upload().await,
                }
            };
            let succeeded = tokio::select! {
                result = transfer => result.is_ok(),
                () = self.stop.cancelled() => return,
            };
            if !succeeded {
                return;
            }
        }
    }

    async fn download(&self, request: u64) -> Result<(), ()> {
        // A unique query per request keeps any cache out of the measurement.
        let url = format!(
            "{}/__down?bytes={}&r={}-{request}-{}",
            self.base_url,
            self.config.download_chunk_bytes,
            self.index,
            now_ms()
        );
        let mut response = self.http.get(url).send().await.map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            self.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn upload(&self) -> Result<(), ()> {
        let counter = self.bytes.clone();
        let piece = Bytes::from(vec![0_u8; UPLOAD_PIECE_BYTES]);
        let pieces =
            futures_util::stream::unfold(self.config.upload_chunk_bytes, move |remaining| {
                let counter = counter.clone();
                let piece = piece.clone();
                async move {
                    if remaining == 0 {
                        return None;
                    }
                    let size = remaining.min(piece.len());
                    counter.fetch_add(size as u64, Ordering::Relaxed);
                    Some((
                        Ok::<_, std::io::Error>(piece.slice(..size)),
                        remaining - size,
                    ))
                }
            });
        let response = self
            .http
            .post(format!("{}/__up", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::Body::wrap_stream(pieces))
            .send()
            .await
            .map_err(|_| ())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkQuality {
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_pct: Option<f64>,
}

impl LinkQuality {
    pub const UNKNOWN: Self = Self {
        latency_ms: None,
        jitter_ms: None,
        loss_pct: None,
    };
}

/// Latency, jitter, and loss over a run's window, from the dish's samples. An
/// open window (still running) ends at `now_ms`.
pub fn link_quality(
    samples: &[TelemetrySample],
    started_at_ms: Option<i64>,
    ended_at_ms: Option<i64>,
    now_ms: i64,
) -> LinkQuality {
    let Some(started) = started_at_ms else {
        return LinkQuality::UNKNOWN;
    };
    let until = ended_at_ms.unwrap_or(now_ms);
    let window: Vec<&TelemetrySample> = samples
        .iter()
        .filter(|sample| (started..=until).contains(&sample.timestamp_ms))
        .collect();
    let latencies: Vec<f64> = window
        .iter()
        .filter_map(|sample| sample.latency_ms)
        .filter(|latency| latency.is_finite())
        .collect();
    if latencies.is_empty() {
        return LinkQuality::UNKNOWN;
    }
    // Ookla-style jitter: the mean absolute step between consecutive pings.
    let jitter = if latencies.len() > 1 {
        latencies
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .sum::<f64>()
            / (latencies.len() - 1) as f64
    } else {
        0.0
    };
    let mut sorted = latencies;
    sorted.sort_by(f64::total_cmp);
    let loss = window.iter().map(|sample| sample.drop_rate).sum::<f64>() / window.len() as f64;
    LinkQuality {
        latency_ms: Some(sorted[sorted.len() / 2]),
        jitter_ms: Some(jitter),
        loss_pct: Some(loss * 100.0),
    }
}

/// Square root spreads out the low end, so slow links still move the needle.
pub fn gauge_fraction(mbps: f64) -> f64 {
    let clamped = if mbps.is_finite() {
        mbps.clamp(0.0, GAUGE_MAX_MBPS)
    } else {
        0.0
    };
    clamped.sqrt() / GAUGE_MAX_MBPS.sqrt()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Instant;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const FAST: SpeedTestConfig = SpeedTestConfig {
        ramp_ms: 40.0,
        settle_ms: 20.0,
        measure_ms: 200.0,
        sample_ms: 10.0,
        hold_ms: 10.0,
        rest_ms: 10.0,
        download_chunk_bytes: 64_000,
        upload_chunk_bytes: 64_000,
    };

    fn sample(timestamp_ms: i64, latency_ms: Option<f64>, drop_rate: f64) -> TelemetrySample {
        TelemetrySample {
            timestamp_ms,
            latency_ms,
            drop_rate,
            downlink_bps: 0.0,
            uplink_bps: 0.0,
            power_w: 0.0,
            router_latency_ms: None,
            router_ping_success_percent: None,
        }
    }

    fn recorder() -> (ProgressHandler, Arc<Mutex<Vec<SpeedTestProgress>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (
            Arc::new(move |progress| sink.lock().unwrap().push(progress)),
            seen,
        )
    }

    fn phases(seen: &Mutex<Vec<SpeedTestProgress>>) -> Vec<SpeedTestPhase> {
        let mut phases: Vec<_> = seen.lock().unwrap().iter().map(|p| p.phase).collect();
        phases.dedup();
        phases
    }

    async fn cloudflare_mock(download_status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(ResponseTemplate::new(download_status).set_body_bytes(
                if download_status == 200 {
                    vec![0_u8; 64_000]
                } else {
                    Vec::new()
                },
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        server
    }

    // SPEC-SPEED-002: the ramp is excluded, the live figure waits for the
    // settle window, and the rate is the cumulative average since the ramp.
    #[test]
    fn the_phase_meter_excludes_the_ramp_and_waits_to_settle() {
        let config = SpeedTestConfig::default();
        let mut meter = PhaseMeter::new(0.0, config);
        assert_eq!(meter.sample(250.0, 1_000_000), None, "still ramping");
        assert_eq!(meter.sample(1_000.0, 2_000_000), None, "ramp just ended");
        assert_eq!(meter.sample(1_500.0, 4_000_000), None, "not yet settled");
        // 10 MB in the 1 s since the ramp = 80 Mbps.
        let live = meter.sample(2_000.0, 12_000_000).expect("settled");
        assert!((live - 80.0).abs() < 1e-9, "{live}");
        assert!(!meter.is_over(10_999.0));
        assert!(meter.is_over(11_000.0));
        let result = meter.finish(11_000.0, 102_000_000);
        assert!((result - 80.0).abs() < 1e-9, "{result}");
    }

    // SPEC-SPEED-005: median latency, mean absolute step as jitter, and mean
    // drop rate as loss, over the run's window only.
    #[test]
    fn link_quality_reads_the_dish_samples_in_the_window() {
        let samples = [
            sample(900, Some(99.0), 1.0),
            sample(1_000, Some(20.0), 0.0),
            sample(2_000, Some(30.0), 0.0),
            sample(3_000, None, 1.0),
            sample(4_000, Some(25.0), 0.0),
            sample(9_000, Some(99.0), 1.0),
        ];
        let quality = link_quality(&samples, Some(1_000), Some(4_000), 10_000);
        assert_eq!(quality.latency_ms, Some(25.0));
        assert_eq!(quality.jitter_ms, Some(7.5));
        assert_eq!(quality.loss_pct, Some(25.0));

        let open = link_quality(&samples, Some(1_000), None, 9_000);
        assert_eq!(open.latency_ms, Some(30.0), "an open window runs to now");
        assert_eq!(
            link_quality(&samples, None, None, 9_000),
            LinkQuality::UNKNOWN
        );
        assert_eq!(
            link_quality(&samples, Some(3_000), Some(3_500), 9_000),
            LinkQuality::UNKNOWN
        );
    }

    // SPEC-SPEED-006: the square-root scale and its fixed ends.
    #[test]
    fn the_gauge_scale_spreads_the_low_end() {
        assert_eq!(gauge_fraction(0.0), 0.0);
        assert_eq!(gauge_fraction(-5.0), 0.0);
        assert_eq!(gauge_fraction(500.0), 1.0);
        assert_eq!(gauge_fraction(900.0), 1.0);
        assert!((gauge_fraction(125.0) - 0.5).abs() < 1e-12);
        assert_eq!(GAUGE_TICKS.first(), Some(&0));
        assert_eq!(GAUGE_TICKS.last(), Some(&500));
    }

    // SPEC-SPEED-001, SPEC-SPEED-003: a full run downloads then uploads, rests, finishes,
    // and closes the measurement window at the end of upload.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_measures_both_directions_in_order() {
        let server = cloudflare_mock(200).await;
        let client = SpeedTestClient::with_base_url(&server.uri(), FAST);
        let (handler, seen) = recorder();
        client.run(&CancelToken::new(), handler).await;

        assert_eq!(
            phases(&seen),
            [
                SpeedTestPhase::Download,
                SpeedTestPhase::Upload,
                SpeedTestPhase::Resting,
                SpeedTestPhase::Done
            ]
        );
        let last = seen.lock().unwrap().last().cloned().expect("progress");
        assert!(last.download_mbps.is_some_and(|mbps| mbps > 0.0));
        assert!(last.upload_mbps.is_some_and(|mbps| mbps > 0.0));
        let (started, ended) = (last.started_at_ms.unwrap(), last.ended_at_ms.unwrap());
        assert!(ended > started);

        let requests = server.received_requests().await.unwrap_or_default();
        let downloads = requests
            .iter()
            .filter(|r| r.url.path() == "/__down")
            .count();
        let uploads = requests.iter().filter(|r| r.url.path() == "/__up").count();
        assert!(downloads >= STREAMS, "{downloads} downloads");
        assert!(uploads >= STREAMS, "{uploads} uploads");
        assert!(
            requests
                .iter()
                .filter(|r| r.url.path() == "/__down")
                .all(|r| r
                    .url
                    .query()
                    .is_some_and(|query| query.contains("bytes=64000")))
        );
    }

    // SPEC-SPEED-001: a phase with no bytes fails the run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_phase_with_no_bytes_fails_the_run() {
        let server = cloudflare_mock(503).await;
        let client = SpeedTestClient::with_base_url(&server.uri(), FAST);
        let (handler, seen) = recorder();
        client.run(&CancelToken::new(), handler).await;
        assert_eq!(
            phases(&seen),
            [SpeedTestPhase::Download, SpeedTestPhase::Error]
        );
    }

    // SPEC-SPEED-004: cancelling mid-download stops promptly, never reaches
    // upload, reports nothing more, and is not an error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_mid_download_stops_quietly() {
        let server = cloudflare_mock(200).await;
        let slow = SpeedTestConfig {
            measure_ms: 10_000.0,
            ..FAST
        };
        let client = SpeedTestClient::with_base_url(&server.uri(), slow);
        let (handler, seen) = recorder();
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let started = Instant::now();
        client.run(&cancel, handler).await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let reported = seen.lock().unwrap().len();
        assert_eq!(phases(&seen), [SpeedTestPhase::Download]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(seen.lock().unwrap().len(), reported, "nothing after cancel");

        let (handler, seen) = recorder();
        let cancelled = CancelToken::new();
        cancelled.cancel();
        client.run(&cancelled, handler).await;
        assert!(seen.lock().unwrap().is_empty());
    }
}
