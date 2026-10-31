//! Typed telemetry: the dish's ring buffer, decoded into absolute time.
//!
//! The port of upstream `core/telemetry.ts`, including the accumulator that stitches
//! successive 15-minute polls into a longer window. Everything here is pure
//! except [`TelemetryAccumulator`], whose only state is the samples it holds.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de};

/// A protobuf `uint64` in proto3 JSON: a string, or a number from a test or a
/// hand-written fixture. The dish reports counters that fit in `u64`; treating
/// them as JSON numbers would lose precision above 2^53.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct U64Value(pub u64);

impl<'de> Deserialize<'de> for U64Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = U64Value;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a uint64 as a string or an integer")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(U64Value(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u64::try_from(value)
                    .map(U64Value)
                    .map_err(|_| E::custom("negative value for a uint64"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value
                    .parse::<u64>()
                    .map(U64Value)
                    .map_err(|error| E::custom(format!("invalid uint64: {error}")))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for U64Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

// ---------- device models ----------

/// A protobuf `float`/`double` in proto3 JSON: usually a number, but the
/// canonical form spells non-finite values as strings (`"NaN"`, `"Infinity"`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProtoF64(pub f64);

impl<'de> Deserialize<'de> for ProtoF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = ProtoF64;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a float, or \"NaN\"/\"Infinity\"")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(ProtoF64(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(ProtoF64(value as f64))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(ProtoF64(value as f64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let parsed = match value {
                    "NaN" => f64::NAN,
                    "Infinity" => f64::INFINITY,
                    "-Infinity" => f64::NEG_INFINITY,
                    other => other
                        .parse::<f64>()
                        .map_err(|error| E::custom(format!("invalid float: {error}")))?,
                };
                Ok(ProtoF64(parsed))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for ProtoF64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.0.is_finite() {
            serializer.serialize_f64(self.0)
        } else {
            serializer.serialize_str(if self.0.is_nan() {
                "NaN"
            } else if self.0.is_sign_positive() {
                "Infinity"
            } else {
                "-Infinity"
            })
        }
    }
}

fn de_vec_f64<'de, D>(deserializer: D) -> Result<Vec<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Visitor;

    impl<'de> de::Visitor<'de> for Visitor {
        type Value = Vec<f64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an array of floats")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
            while let Some(value) = sequence.next_element::<ProtoF64>()? {
                values.push(value.0);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_seq(Visitor)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishHistory {
    pub current: Option<U64Value>,
    #[serde(deserialize_with = "de_vec_f64")]
    pub pop_ping_drop_rate: Vec<f64>,
    #[serde(deserialize_with = "de_vec_f64")]
    pub pop_ping_latency_ms: Vec<f64>,
    #[serde(deserialize_with = "de_vec_f64")]
    pub downlink_throughput_bps: Vec<f64>,
    #[serde(deserialize_with = "de_vec_f64")]
    pub uplink_throughput_bps: Vec<f64>,
    #[serde(deserialize_with = "de_vec_f64")]
    pub power_in: Vec<f64>,
    pub outages: Vec<DishOutage>,
    pub event_log: Option<EventLog>,
}

impl DishHistory {
    pub fn newest_counter(&self) -> u64 {
        self.current.map_or(0, |value| value.0)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishOutage {
    pub cause: Option<String>,
    pub start_timestamp_ns: Option<U64Value>,
    pub duration_ns: Option<U64Value>,
    pub did_switch: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventLog {
    pub events: Vec<DishEvent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishEvent {
    pub severity: Option<String>,
    pub reason: Option<String>,
    pub start_timestamp_ns: Option<U64Value>,
    pub duration_ns: Option<U64Value>,
    pub client_switching_band_metadata: Option<BandSwitchMetadata>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BandSwitchMetadata {
    pub client_id: Option<i64>,
    pub from_band: Option<String>,
    pub to_band: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishStatus {
    pub device_info: Option<DishDeviceInfo>,
    pub device_state: Option<DeviceState>,
    pub obstruction_stats: Option<ObstructionStats>,
    pub alerts: BTreeMap<String, bool>,
    pub downlink_throughput_bps: ProtoF64,
    pub uplink_throughput_bps: ProtoF64,
    pub pop_ping_latency_ms: ProtoF64,
    pub pop_ping_drop_rate: ProtoF64,
    pub boresight_azimuth_deg: ProtoF64,
    pub boresight_elevation_deg: ProtoF64,
    pub stow_requested: bool,
    pub gps_stats: Option<GpsStats>,
    pub eth_speed_mbps: ProtoF64,
    pub class_of_service: Option<String>,
    pub software_update_state: Option<String>,
    pub alignment_stats: Option<AlignmentStats>,
    pub connected_routers: Vec<String>,
    pub downstream_routers: BTreeMap<String, DownstreamRouter>,
    pub dl_bandwidth_restricted_reason: Option<String>,
    pub ul_bandwidth_restricted_reason: Option<String>,
    pub is_snr_above_noise_floor: Option<bool>,
    pub is_snr_persistently_low: bool,
    pub ready_states: Option<ReadyStates>,
    pub seconds_until_swupdate_reboot_possible: Option<i64>,
    pub nat_flag: Option<String>,
    pub ned2dish_quaternion: Option<Quaternion>,
    pub has_actuators: Option<String>,
    pub mobility_class: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishObstructionMap {
    pub num_rows: u32,
    pub num_cols: u32,
    /// Usable fraction per cell; negative means never observed.
    pub snr: Vec<f64>,
    pub max_theta_deg: Option<f64>,
    pub map_reference_frame: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DishDeviceInfo {
    pub id: Option<String>,
    pub hardware_version: Option<String>,
    pub software_version: Option<String>,
    pub country_code: Option<String>,
    pub bootcount: Option<U64Value>,
    pub generation_number: Option<String>,
    pub build_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeviceState {
    pub uptime_s: Option<U64Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ObstructionStats {
    pub fraction_obstructed: ProtoF64,
    pub valid_s: ProtoF64,
    pub avg_prolonged_obstruction_interval_s: Option<ProtoF64>,
    pub patches_valid: Option<ProtoF64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GpsStats {
    pub gps_valid: bool,
    pub gps_sats: u32,
    pub pnt_filter_convergence_state: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AlignmentStats {
    pub tilt_angle_deg: ProtoF64,
    pub boresight_azimuth_deg: ProtoF64,
    pub boresight_elevation_deg: ProtoF64,
    pub desired_boresight_azimuth_deg: ProtoF64,
    pub desired_boresight_elevation_deg: ProtoF64,
    pub attitude_estimation_state: Option<String>,
    pub attitude_uncertainty_deg: ProtoF64,
    pub has_actuators: Option<String>,
    pub actuator_state: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DownstreamRouter {
    pub role: Option<String>,
    pub last_seen: Option<U64Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReadyStates {
    pub scp: bool,
    pub l1l2: bool,
    pub xphy: bool,
    pub aap: bool,
    pub rf: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Quaternion {
    pub q_scalar: ProtoF64,
    pub q_x: ProtoF64,
    pub q_y: ProtoF64,
    pub q_z: ProtoF64,
}

// ---------- samples and events ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelemetrySample {
    pub timestamp_ms: i64,
    pub latency_ms: Option<f64>,
    pub drop_rate: f64,
    pub downlink_bps: f64,
    pub uplink_bps: f64,
    pub power_w: f64,
    /// Stamped from the router's live reply; the router has no replayable
    /// history. `None` means nothing was sampling, never a carried-forward value.
    pub router_latency_ms: Option<f64>,
    /// Router → PoP ping success (0–100), from `popPingDropRate5m` in a
    /// `get_status` reply already fetched. Never from the ping RPC.
    pub router_ping_success_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Advisory,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Outage,
    Degraded,
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BandSwitch {
    pub client_id: i64,
    pub from_band: String,
    pub to_band: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutageEvent {
    pub start_ms: i64,
    pub duration_ms: i64,
    pub cause: String,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_switch: Option<BandSwitch>,
}

/// The `outages[]` timestamps use the GPS epoch (1980-01-06); `eventLog` uses
/// Unix. Unix seconds at the GPS epoch minus the 18 leap seconds accumulated
/// since. Verified against this dish: the same outage appears in both lists
/// exactly this far apart.
const GPS_TO_UNIX_OFFSET_NS: u64 = (315_964_800 - 18) * 1_000_000_000;

/// Unroll the ring buffer. `current` counts samples written since boot; each
/// array holds the last `array_length` samples at 1 Hz, where absolute counter
/// `c` lives at index `c % array_length`. The newest sample is pinned to `now`.
pub fn decode_history_window(history: &DishHistory, now_ms: i64) -> HistoryWindow {
    let newest_counter = history.newest_counter();
    let array_length = history.pop_ping_latency_ms.len();
    if array_length == 0 || newest_counter == 0 {
        return HistoryWindow {
            samples: Vec::new(),
            newest_counter,
        };
    }

    let sample_count = usize::try_from(newest_counter)
        .unwrap_or(usize::MAX)
        .min(array_length);
    let mut samples = Vec::with_capacity(sample_count);
    for sample_index in 0..sample_count {
        let absolute_counter = newest_counter - (sample_count - sample_index) as u64;
        let ring_index = (absolute_counter as usize) % array_length;
        let latency = history.pop_ping_latency_ms[ring_index];
        samples.push(TelemetrySample {
            timestamp_ms: now_ms - ((sample_count - 1 - sample_index) as i64) * 1_000,
            latency_ms: (latency.is_finite() && latency > 0.0).then_some(latency),
            drop_rate: history
                .pop_ping_drop_rate
                .get(ring_index)
                .copied()
                .unwrap_or(0.0),
            downlink_bps: history
                .downlink_throughput_bps
                .get(ring_index)
                .copied()
                .unwrap_or(0.0),
            uplink_bps: history
                .uplink_throughput_bps
                .get(ring_index)
                .copied()
                .unwrap_or(0.0),
            power_w: history.power_in.get(ring_index).copied().unwrap_or(0.0),
            router_latency_ms: None,
            router_ping_success_percent: None,
        });
    }

    HistoryWindow {
        samples,
        newest_counter,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryWindow {
    pub samples: Vec<TelemetrySample>,
    pub newest_counter: u64,
}

fn unix_ns_to_ms(value: Option<U64Value>) -> i64 {
    let ns = value.map_or(0, |value| value.0);
    i64::try_from(ns / 1_000_000).unwrap_or(i64::MAX)
}

fn gps_ns_to_unix_ms(value: Option<U64Value>) -> i64 {
    let ns = value.map_or(0, |value| value.0);
    i64::try_from((ns + GPS_TO_UNIX_OFFSET_NS) / 1_000_000).unwrap_or(i64::MAX)
}

fn severity_of(value: Option<&str>) -> Severity {
    match value {
        Some("EVENT_SEVERITY_CRITICAL") => Severity::Critical,
        Some("EVENT_SEVERITY_WARNING") => Severity::Warning,
        _ => Severity::Advisory,
    }
}

fn decode_event_log(events: &[DishEvent]) -> Vec<OutageEvent> {
    events
        .iter()
        .map(|event| OutageEvent {
            start_ms: unix_ns_to_ms(event.start_timestamp_ns),
            duration_ms: unix_ns_to_ms(event.duration_ns),
            cause: event.reason.clone().unwrap_or_default(),
            severity: severity_of(event.severity.as_deref()),
            band_switch: decode_band_switch(event.client_switching_band_metadata.as_ref()),
        })
        .collect()
}

fn decode_band_switch(metadata: Option<&BandSwitchMetadata>) -> Option<BandSwitch> {
    let metadata = metadata?;
    let client_id = metadata.client_id?;
    let from_band = metadata.from_band.clone().filter(|band| !band.is_empty())?;
    let to_band = metadata.to_band.clone().filter(|band| !band.is_empty())?;
    Some(BandSwitch {
        client_id,
        from_band,
        to_band,
    })
}

/// Decode an outage list. The event log wins when both are present: it is the
/// richer record, and the same outage appears in both (in different epochs).
pub fn decode_outage_events(history: &DishHistory) -> Vec<OutageEvent> {
    if let Some(log) = &history.event_log
        && !log.events.is_empty()
    {
        return decode_event_log(&log.events);
    }

    history
        .outages
        .iter()
        .map(|outage| OutageEvent {
            start_ms: gps_ns_to_unix_ms(outage.start_timestamp_ns),
            duration_ms: unix_ns_to_ms(outage.duration_ns),
            cause: outage.cause.clone().unwrap_or_default(),
            severity: Severity::Warning,
            band_switch: None,
        })
        .collect()
}

/// Reduce a raw enum, or a legacy human label, to one stable token.
pub fn canonical_cause(cause: &str) -> String {
    let mut token = cause.trim().to_uppercase();
    token = collapse_separators(&token);
    for prefix in ["EVENT_REASON_", "OUTAGE_", "UT_ALERT_"] {
        if let Some(rest) = token.strip_prefix(prefix) {
            token = rest.to_string();
        }
    }
    token
}

fn collapse_separators(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_separator = false;
    for character in value.chars() {
        if character.is_whitespace() || character == '-' {
            if !in_separator {
                out.push('_');
                in_separator = true;
            }
        } else {
            out.push(character);
            in_separator = false;
        }
    }
    out
}

/// The catalogued kind, or `None` for a cause the catalogue does not know.
pub fn known_event_kind(cause: &str) -> Option<EventKind> {
    use EventKind::{Degraded, Info, Outage};
    Some(match canonical_cause(cause).as_str() {
        "NO_PINGS"
        | "NO_DOWNLINK"
        | "NO_SATS"
        | "NO_SCHEDULE"
        | "UNKNOWN"
        | "OBSTRUCTED"
        | "THERMAL_SHUTDOWN"
        | "RAIN_SNR_PERSISTENTLY_LOW"
        | "WEAK_SIGNAL_FROM_WEATHER"
        | "BOOTING"
        | "SKY_SEARCH"
        | "ACTUATOR_ACTIVITY"
        | "STOWED"
        | "SLEEPING"
        | "CABLE_TEST"
        | "INHIBIT_RF"
        | "ETH_NO_LINK" => Outage,
        "ROUTER_POP_IPV4_PING_DROP"
        | "ROUTER_POP_IPV6_PING_DROP"
        | "ROUTER_DISH_PING_DROP"
        | "HIGH_DOWNLINK_PACKET_LOSS" => Degraded,
        "ROUTER_POWER_CYCLE"
        | "CLIENT_SWITCHING_BAND"
        | "CLIENT_SWITCHING_UPSTREAM_MAC"
        | "ROUTER_PUBLIC_IPV4_CHANGE" => Info,
        _ => return None,
    })
}

/// What the event was. Unknown tokens default to `Info`: a red band is a claim
/// that service stopped, and under-claiming a new firmware event is the cheaper
/// mistake.
pub fn outage_event_kind(cause: &str) -> EventKind {
    known_event_kind(cause).unwrap_or_else(|| {
        if cause.starts_with("EVENT_REASON_OUTAGE_") || cause.starts_with("OUTAGE_") {
            EventKind::Outage
        } else {
            EventKind::Info
        }
    })
}

/// How many consecutive fully-dropping samples make an outage. One dropped
/// second is a satellite handover; eight is the link being down.
pub const OUTAGE_SAMPLE_RUN: usize = 8;

pub fn is_starlink_outage(samples: &[TelemetrySample]) -> bool {
    let recent = &samples[samples.len().saturating_sub(OUTAGE_SAMPLE_RUN)..];
    recent.len() >= OUTAGE_SAMPLE_RUN && recent.iter().all(|sample| sample.drop_rate >= 1.0)
}

/// A latency reading only if it is one. proto3 omits a zero; `NaN` is not a
/// measurement either. The historian and the live edge must agree on this.
pub fn read_router_latency_ms(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

/// Router ping success (0–100) from `popPingDropRate5M`. `None` is proto3's
/// absent zero — no drops, 100% — and only an answering router can assert that.
pub fn read_router_ping_success_percent(drop_rate_5m: Option<f64>) -> Option<f64> {
    match drop_rate_5m {
        None => Some(100.0),
        Some(rate) if rate.is_finite() && (0.0..=1.0).contains(&rate) => Some((1.0 - rate) * 100.0),
        Some(_) => None,
    }
}

// ---------- cursors and the accumulator ----------

/// How far a consumer has already read the ring buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCursor {
    /// Absolute count of samples written at the last read. 0 means unknown.
    pub counter: u64,
    /// Timestamp of the newest sample already taken; the fallback dedup key.
    pub newest_sample_ms: i64,
}

/// The samples in this window that have not been recorded yet.
///
/// New is judged by the dish's counter when it can be trusted, and by timestamp
/// when it cannot (a reboot restarted the counter, or a snapshot has none). The
/// 500 ms grace absorbs sub-second jitter in stamped times across polls.
pub fn select_fresh_samples(window: &HistoryWindow, cursor: SampleCursor) -> Vec<TelemetrySample> {
    if window.samples.is_empty() {
        return Vec::new();
    }
    let counter_trustworthy = cursor.counter > 0 && window.newest_counter >= cursor.counter;
    if !counter_trustworthy {
        if cursor.newest_sample_ms == 0 {
            return window.samples.clone();
        }
        return window
            .samples
            .iter()
            .filter(|sample| sample.timestamp_ms > cursor.newest_sample_ms + 500)
            .cloned()
            .collect();
    }
    let fresh_count = usize::try_from(window.newest_counter - cursor.counter)
        .unwrap_or(usize::MAX)
        .min(window.samples.len());
    window.samples[window.samples.len() - fresh_count..].to_vec()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RouterReadings {
    pub latency_ms: Option<f64>,
    pub ping_success_percent: Option<f64>,
}

/// The two router `get_status` fields the sample stamping uses.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterStatus {
    pub pop_ping_latency_ms: Option<ProtoF64>,
    #[serde(rename = "popPingDropRate5m")]
    pub pop_ping_drop_rate_5m: Option<ProtoF64>,
    pub alerts: BTreeMap<String, bool>,
}

impl RouterStatus {
    pub fn readings(&self) -> RouterReadings {
        RouterReadings {
            latency_ms: read_router_latency_ms(self.pop_ping_latency_ms.map(|value| value.0)),
            ping_success_percent: read_router_ping_success_percent(
                self.pop_ping_drop_rate_5m.map(|value| value.0),
            ),
        }
    }
}

/// Stitches ring-buffer polls into one continuous series holding a fixed span of
/// real time. Silence occupies room in the buffer: an outage is part of what the
/// retained window has to say.
#[derive(Debug, Clone)]
pub struct TelemetryAccumulator {
    retention_ms: i64,
    samples: Vec<TelemetrySample>,
    newest_counter: u64,
}

impl TelemetryAccumulator {
    pub fn new(retention_ms: i64) -> Self {
        Self {
            retention_ms,
            samples: Vec::new(),
            newest_counter: 0,
        }
    }

    pub fn samples(&self) -> &[TelemetrySample] {
        &self.samples
    }

    pub fn newest_counter(&self) -> u64 {
        self.newest_counter
    }

    /// Backfill with persisted samples before live polling. No-op once live
    /// data exists.
    pub fn seed(&mut self, persisted: &[TelemetrySample]) -> &[TelemetrySample] {
        if self.samples.is_empty() && !persisted.is_empty() {
            self.samples.extend_from_slice(persisted);
            self.trim();
        }
        &self.samples
    }

    pub fn ingest(
        &mut self,
        history: &DishHistory,
        now_ms: i64,
        router: RouterReadings,
    ) -> &[TelemetrySample] {
        let window = decode_history_window(history, now_ms);
        self.ingest_window(&window, router)
    }

    /// Ingest an already-decoded window, so one poll cycle does not unroll the
    /// same ring twice.
    pub fn ingest_window(
        &mut self,
        window: &HistoryWindow,
        router: RouterReadings,
    ) -> &[TelemetrySample] {
        if window.samples.is_empty() {
            return &self.samples;
        }

        let cursor = SampleCursor {
            counter: self.newest_counter,
            newest_sample_ms: self.samples.last().map_or(0, |sample| sample.timestamp_ms),
        };
        let mut fresh = select_fresh_samples(window, cursor);
        for sample in &mut fresh {
            if router.latency_ms.is_some() {
                sample.router_latency_ms = router.latency_ms;
            }
            if router.ping_success_percent.is_some() {
                sample.router_ping_success_percent = router.ping_success_percent;
            }
        }
        self.samples.extend(fresh);
        self.newest_counter = window.newest_counter;
        self.trim();
        &self.samples
    }

    /// Drop everything older than the retention span, measured back from the
    /// newest sample held. A snapshot is entirely in the past; measuring from
    /// `now` would empty it on restore.
    fn trim(&mut self) {
        let Some(newest) = self.samples.last().map(|sample| sample.timestamp_ms) else {
            return;
        };
        let cutoff = newest - self.retention_ms;
        if let Some(first_kept) = self
            .samples
            .iter()
            .position(|sample| sample.timestamp_ms >= cutoff)
            && first_kept > 0
        {
            self.samples.drain(..first_kept);
        }
    }
}
