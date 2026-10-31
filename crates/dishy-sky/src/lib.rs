//! Pure sky maths: obstruction geometry, camera, and generated vertex buffers.
//!
//! The TypeScript app renders the sky in hand-written WebGL. The renderer is
//! replaced with `gtk::GLArea` in milestone M6; the maths below it is portable
//! and must be tested without a GPU, so it lives here.

pub mod camera;
pub mod constellation;
pub mod dish_models;
pub mod frame;
pub mod geometry;
pub mod math;
pub mod satellite_mesh;
pub mod serving;
pub mod survey;
pub mod tle_feed;

pub use camera::{CameraView, SkyCamera};
pub use constellation::{
    ConstellationError, ObserverLocation, SERVING_ELEVATION_FLOOR_DEG, SatelliteAttitude,
    SatelliteLook, StarlinkTracker, TleCatalog, serving_candidate,
};
pub use frame::{
    MountFamily, ObstructionFrame, grid_cell_to_enu, resolve_frame, sky_direction_to_grid,
};
pub use geometry::{
    DishMesh, LIGHT, SATELLITE_MODEL_SCALE, TRAIL_FLOATS, build_compass, build_compass_labels,
    build_dish, build_dome_points, build_stars, build_terrain, build_trail_ribbon, drawn_position,
    satellite_basis, satellite_position, trail_points,
};
pub use math::{Mat4, Vec3, look_at, multiply, perspective};
pub use serving::{
    HANDOFF_SLOT_MS, ObstructionLookup, ServingHold, choose_serving_candidate, is_unobstructed,
    slot_index_at,
};
pub use survey::{
    CELL_CLEAR, CELL_OBSTRUCTED, CELL_PARTIAL, CELL_UNMAPPED, DomePointKind,
    OBSTRUCTED_FRACTION_FLOOR, OBSTRUCTION_GRID, PARTIAL_FRACTION_CEILING, Survey, classify_live,
    unpack_cells,
};
