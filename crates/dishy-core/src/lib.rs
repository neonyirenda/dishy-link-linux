//! Host-agnostic core for the GTK app: transport, protocol, polling, recording.
//!
//! Nothing in this crate knows about GTK, a window, or a tray. That boundary is
//! what lets the recorder, telemetry maths, and device clients be tested without
//! a display, and it is the same boundary the Electron app keeps between
//! `core/`, `collector/`, and `electron/`.

pub mod alert;
pub mod alignment;
pub mod calendar;
pub mod client;
pub mod client_window;
pub mod control;
pub mod data_meter;
pub mod device_group;
pub mod device_kind;
pub mod energy;
pub mod external;
pub mod grpc_web;
pub mod http_body;
pub mod jsonl;
pub mod latency;
pub mod latency_summary;
pub mod lock;
pub mod meter;
pub mod meter_store;
pub mod network;
pub mod odometer;
pub mod oui;
pub mod paths;
pub mod postmortem;
pub mod recorder;
pub mod runner;
pub mod schedule;
pub mod speed_test;
pub mod store;
pub mod telemetry;
pub mod throughput;
pub mod usage;

pub use client::{
    DISH_LAN_ADDRESS, DISH_LAN_HANDLE_URL, DishClient, DishClientError, ROUTER_LAN_ADDRESS,
    ROUTER_LAN_HANDLE_URL, Target,
};
pub use grpc_web::{GrpcCallBytes, GrpcWebClient, GrpcWebError};

/// The recorder's poll cadence. Carried over from upstream `collector/historian.mts`:
/// the dish's ring buffer holds 900 one-second samples, so a 5 s poll captures
/// every sample and only controls how fresh the newest point is.
///
/// Hardware safety: do not add a poll against the router without approval. See
/// `specs/0030-hardware-safety.md` and `specs/0031-polling-and-recording.md`.
pub const HISTORIAN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// The dish's status cadence, approved with the sky view. The reply carries the
/// boresight, obstruction statistics, thermal flags, and device identity, so
/// one poll feeds every status-only surface.
pub const STATUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// The obstruction map cadence. The grid is large and changes slowly; the
/// recorder keeps an hourly snapshot, so this rate only controls how fresh the
/// live dome is.
pub const OBSTRUCTION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// The network panel's client-roster cadence, approved for M7. It matches the
/// Electron panel, not the historian's much faster per-device sampling, and it
/// runs only while the page is visible.
pub const NETWORK_CLIENTS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// The network panel's radio-temperature cadence, approved for M7.
pub const NETWORK_RADIO_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// How long to wait before retrying a Wi-Fi config read that failed. A config
/// that has answered is not re-read until the router reconnects.
pub const NETWORK_CONFIG_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// The dish configuration is a heavier read, so Settings refreshes it at most
/// once a minute while that page is visible.
pub const DISH_CONFIG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// The dish's history ring holds 900 samples at 1 Hz.
pub const HISTORY_RING_SAMPLES: usize = 900;
