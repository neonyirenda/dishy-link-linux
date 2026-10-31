//! Obstruction grid orientation and cell lookup.
//!
//! The firmware labels its grid with `mapReferenceFrame`. `FRAME_EARTH` is
//! north-up. `FRAME_UT` is dish-relative: the bottom of the grid points along
//! the boresight azimuth. Mini kits on roam usually report the latter, and a
//! missing frame is resolved to the convention the hardware actually emits.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObstructionFrame {
    #[serde(rename = "FRAME_EARTH")]
    Earth,
    #[serde(rename = "FRAME_UT")]
    Ut,
}

/// The hardware family the frame fallback depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountFamily {
    Mini,
    Other,
}

pub fn resolve_frame(reported: Option<&str>, family: MountFamily) -> ObstructionFrame {
    match reported {
        Some("FRAME_EARTH") => ObstructionFrame::Earth,
        Some("FRAME_UT") => ObstructionFrame::Ut,
        _ if family == MountFamily::Mini => ObstructionFrame::Ut,
        _ => ObstructionFrame::Earth,
    }
}

/// Grid cell → local east/north in units of grid radius (−1…1).
pub fn grid_cell_to_enu(
    row: usize,
    col: usize,
    grid_size: usize,
    frame: ObstructionFrame,
    boresight_azimuth_deg: f64,
) -> (f64, f64) {
    if grid_size < 2 {
        return (0.0, 0.0);
    }
    let centre = (grid_size - 1) as f64 / 2.0;
    let east_grid = (col as f64 - centre) / centre;
    let north_grid = (centre - row as f64) / centre;
    if frame != ObstructionFrame::Ut {
        return (east_grid, north_grid);
    }

    // After a vertical flip, +forward is the bottom of the page (boresight) and
    // +right is still +column. Rotate that body frame by the boresight azimuth.
    let forward = -north_grid;
    let right = east_grid;
    let azimuth = boresight_azimuth_deg.to_radians();
    let (sine, cosine) = azimuth.sin_cos();
    (
        right * cosine + forward * sine,
        -right * sine + forward * cosine,
    )
}

/// Geographic az/el, as a polar fraction from zenith, → the grid cell holding it.
pub fn sky_direction_to_grid(
    azimuth_deg: f64,
    radial_fraction: f64,
    grid_size: usize,
    frame: ObstructionFrame,
    boresight_azimuth_deg: f64,
) -> (usize, usize) {
    let centre = (grid_size - 1) as f64 / 2.0;
    let azimuth = azimuth_deg.to_radians();
    let east = azimuth.sin() * radial_fraction;
    let north = azimuth.cos() * radial_fraction;
    let (row, col) = if frame != ObstructionFrame::Ut {
        (centre - north * centre, centre + east * centre)
    } else {
        let boresight = boresight_azimuth_deg.to_radians();
        let (sine, cosine) = boresight.sin_cos();
        let right = east * cosine - north * sine;
        let forward = east * sine + north * cosine;
        (centre + forward * centre, centre + right * centre)
    };
    (clamp_index(row, grid_size), clamp_index(col, grid_size))
}

fn clamp_index(value: f64, grid_size: usize) -> usize {
    value.round().clamp(0.0, (grid_size - 1) as f64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SKY-002: a missing frame is Mini = UT, everything else = earth.
    #[test]
    fn a_missing_frame_falls_back_by_hardware_family() {
        assert_eq!(resolve_frame(None, MountFamily::Mini), ObstructionFrame::Ut);
        assert_eq!(
            resolve_frame(None, MountFamily::Other),
            ObstructionFrame::Earth
        );
        assert_eq!(
            resolve_frame(Some("FRAME_EARTH"), MountFamily::Mini),
            ObstructionFrame::Earth
        );
        assert_eq!(
            resolve_frame(Some("FRAME_UT"), MountFamily::Other),
            ObstructionFrame::Ut
        );
    }

    // SPEC-SKY-002: north-up puts a northern cell above the centre; a UT grid
    // rotated 90° east puts that cell on the right.
    #[test]
    fn the_same_cell_moves_with_the_frame() {
        let earth = grid_cell_to_enu(10, 20, 41, ObstructionFrame::Earth, 0.0);
        assert!(earth.0.abs() < f64::EPSILON);
        assert!(earth.1 > 0.0);

        let ut = grid_cell_to_enu(30, 20, 41, ObstructionFrame::Ut, 90.0);
        assert!(ut.0 > 0.0);
        assert!(ut.1.abs() < f64::EPSILON);
    }

    #[test]
    fn lookups_round_trip_and_clamp_at_the_rim() {
        let (row, col) = sky_direction_to_grid(90.0, 0.5, 41, ObstructionFrame::Earth, 0.0);
        assert_eq!((row, col), (20, 30));
        let (row, col) = sky_direction_to_grid(0.0, 2.0, 41, ObstructionFrame::Earth, 0.0);
        assert_eq!((row, col), (0, 20));
    }
}
