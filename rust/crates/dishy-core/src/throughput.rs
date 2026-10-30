//! Per-device throughput from the router's byte counters.
//!
//! The port of `core/throughputTracker.ts`. The router recomputes each client's
//! counters about every 1005 ms, on its own metronome; a delta between observed
//! changes is one refresh interval of traffic, so dividing by that interval
//! recovers the rate without smoothing. Our own poll-to-poll gap only decides
//! how many intervals a delta spans, and whether it is still measurable at all.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// How often the router recomputes a client's counters (median 1005 ms measured).
const REFRESH_PERIOD_MS: f64 = 1_005.0;
/// A counter still this long is genuinely idle, not just between edges.
const IDLE_AFTER_MS: f64 = 2.0 * REFRESH_PERIOD_MS;
/// Longest polling gap still treated as measurable.
const MAX_POLL_GAP_MS: f64 = 15_000.0;

/// The interval each rate is measured over — the router's refresh period.
pub const RATE_WINDOW_MS: f64 = REFRESH_PERIOD_MS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteCounters {
    pub rx_bytes: f64,
    pub tx_bytes: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThroughputRates {
    pub down_mbps: f64,
    pub up_mbps: f64,
}

#[derive(Debug, Clone)]
struct DeviceState {
    rx_bytes: f64,
    tx_bytes: f64,
    /// When we last successfully read this device, edge or not.
    last_poll_at_ms: f64,
    /// When the counter last actually moved.
    last_change_at_ms: f64,
    /// Last exactly-measured rate, held between edges.
    latest: Option<ThroughputRates>,
}

fn to_mbps(bytes_delta: f64, elapsed_ms: f64) -> f64 {
    (bytes_delta * 8.0) / 1_000_000.0 / (elapsed_ms / 1000.0)
}

#[derive(Debug, Clone, Default)]
pub struct ThroughputTracker {
    devices: HashMap<String, DeviceState>,
}

impl ThroughputTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one reading and return the device's current rate. Call on every
    /// poll — several times per refresh period — since the polls between edges
    /// are what locate them.
    ///
    /// Falls back to the router's own averages when a delta would lie: the first
    /// reading, a counter that went backwards (a re-association), or a polling
    /// gap longer than any one interval can describe.
    pub fn rates(
        &mut self,
        mac_address: &str,
        counters: Option<ByteCounters>,
        at_ms: f64,
        fallback: ThroughputRates,
    ) -> ThroughputRates {
        let Some(counters) = counters else {
            return fallback;
        };

        let Some(previous) = self.devices.get(mac_address) else {
            self.seed(mac_address, counters, at_ms);
            return fallback;
        };

        // Backwards means the association restarted; the previous session's
        // total is not something this one can be measured against.
        if counters.rx_bytes < previous.rx_bytes || counters.tx_bytes < previous.tx_bytes {
            self.seed(mac_address, counters, at_ms);
            return fallback;
        }

        let poll_gap_ms = at_ms - previous.last_poll_at_ms;
        if poll_gap_ms > MAX_POLL_GAP_MS {
            self.seed(mac_address, counters, at_ms);
            return fallback;
        }

        let changed =
            counters.rx_bytes != previous.rx_bytes || counters.tx_bytes != previous.tx_bytes;
        // Re-borrow after the immutable reads; one map lookup per call either way.
        let state = self
            .devices
            .get_mut(mac_address)
            .expect("device state was just read");

        if changed {
            // Polling faster than the refresh catches every edge as it happens,
            // so a change is one interval; `intervals` only exceeds 1 when our
            // own polling stalled. Never derive it from the time since the last
            // *change*: an idle device that then bursts would have its burst
            // divided by the whole idle stretch.
            let intervals = (poll_gap_ms / REFRESH_PERIOD_MS).round().max(1.0);
            let elapsed_ms = intervals * REFRESH_PERIOD_MS;
            state.latest = Some(ThroughputRates {
                down_mbps: to_mbps(counters.rx_bytes - state.rx_bytes, elapsed_ms),
                up_mbps: to_mbps(counters.tx_bytes - state.tx_bytes, elapsed_ms),
            });
            state.last_change_at_ms = at_ms;
        } else if at_ms - state.last_change_at_ms >= IDLE_AFTER_MS {
            state.latest = Some(ThroughputRates {
                down_mbps: 0.0,
                up_mbps: 0.0,
            });
        }

        state.rx_bytes = counters.rx_bytes;
        state.tx_bytes = counters.tx_bytes;
        state.last_poll_at_ms = at_ms;
        state.latest.unwrap_or(fallback)
    }

    /// Drop state for devices no longer present, so the map cannot grow forever.
    pub fn retain<'a>(&mut self, mac_addresses: impl IntoIterator<Item = &'a str>) {
        let live: HashSet<&str> = mac_addresses.into_iter().collect();
        self.devices.retain(|mac, _| live.contains(mac.as_str()));
    }

    fn seed(&mut self, mac_address: &str, counters: ByteCounters, at_ms: f64) {
        self.devices.insert(
            mac_address.to_string(),
            DeviceState {
                rx_bytes: counters.rx_bytes,
                tx_bytes: counters.tx_bytes,
                last_poll_at_ms: at_ms,
                last_change_at_ms: at_ms,
                latest: None,
            },
        );
    }
}
