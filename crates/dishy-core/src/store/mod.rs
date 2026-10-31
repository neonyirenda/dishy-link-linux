//! Durable stores: JSON Lines policy on top of the shared primitives.

pub mod energy;
pub mod events;
pub mod latency;
pub mod obstruction;
pub mod samples;
pub mod thermal;

pub use energy::EnergyStore;
pub use events::EventStore;
pub use latency::LatencyStore;
pub use obstruction::{ObstructionSnapshot, ObstructionStore, pack_cells};
pub use samples::SamplesStore;
pub use thermal::{ThermalEpisode, ThermalStore};

pub use crate::jsonl::{JsonlError as StoreError, JsonlResult as StoreResult};
