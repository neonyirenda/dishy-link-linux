//! Native GTK4/libadwaita host for Dishylink.
//!
//! The window, tray, and preferences live here; device calls and recording stay
//! in `dishy-core` (spec 0001). Only this crate links GTK or libadwaita.

pub mod account_devices_model;
pub mod activation;
pub mod alert_sound;
pub mod alerts_view;
pub mod alignment_view;
pub mod app;
pub mod autostart;
pub mod chart;
pub mod cloud_login;
pub mod cloud_session;
pub mod cloud_view;
pub mod dashboard;
pub mod dashboard_cards;
pub mod dashboard_view;
pub mod ephemeris;
pub mod events_model;
pub mod events_view;
pub mod format;
pub mod i18n;
pub mod line_icons;
pub mod location_portal;
pub mod location_setup;
pub mod merge_model;
pub mod network_detail_model;
pub mod network_detail_view;
pub mod network_view;
pub mod notifications;
pub mod observer_location;
pub mod paths;
pub mod preferences;
pub mod readings;
pub mod recorder;
pub mod rules_model;
pub mod rules_view;
pub mod settings_view;
pub mod shell_extension;
pub mod sky_canvas;
pub mod sky_render;
pub mod sky_view;
pub mod speed_beam;
pub mod speed_test_view;
pub mod stat_detail_model;
pub mod stat_detail_view;
pub mod status_bus;
pub mod terminal_model;
pub mod terminal_view;
pub mod theme;
pub mod throughput;
pub mod tray;
pub mod ui;
pub mod usage_view;
pub mod window_bounds;

pub use app::run;
