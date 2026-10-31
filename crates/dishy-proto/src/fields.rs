//! Oneof field numbers inside `SpaceX.API.Device.Request`.
//!
//! Mirrors `REQUEST_FIELD` in upstream `core/dishClient.ts`. The names are the app's, not
//! the schema's: they say what the call is for, so a reader does not have to
//! hold a protobuf tab open.
//!
//! Hardware safety: the router's ping RPC (field 1009) is deliberately absent.
//! Polling it has rebooted the router every time it was tried; see
//! `LOCAL-API.md` and `specs/0030-hardware-safety.md`.

pub const REBOOT: u32 = 1001;
pub const GET_STATUS: u32 = 1004;
pub const GET_HISTORY: u32 = 1007;
pub const GET_DEVICE_INFO: u32 = 1008;
pub const FACTORY_RESET: u32 = 1011;
pub const GET_LOCATION: u32 = 1017;
pub const GET_RADIO_STATS: u32 = 1036;
pub const DISH_STOW: u32 = 2002;
pub const DISH_GET_OBSTRUCTION_MAP: u32 = 2008;
pub const DISH_GET_CONFIG: u32 = 2011;
pub const DISH_CLEAR_OBSTRUCTION_MAP: u32 = 2017;
pub const DISH_FACTORY_RESET: u32 = 2021;
pub const WIFI_GET_CLIENTS: u32 = 3002;
pub const WIFI_GET_CONFIG: u32 = 3009;
pub const GET_DIAGNOSTICS: u32 = 6000;
