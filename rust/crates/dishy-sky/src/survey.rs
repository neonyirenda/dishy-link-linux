//! Turning the dish's obstruction reply into the survey the renderer draws.
//!
//! The classification thresholds and 2-bit snapshot packing are shared with the
//! TypeScript app. This module is the one place a cell becomes a kind, so the
//! dome cannot disagree with the dashboard about what counts as obstructed.

use crate::frame::{MountFamily, ObstructionFrame, resolve_frame};

/// The obstruction grid is 123×123 on current firmware.
pub const OBSTRUCTION_GRID: usize = 123;
/// Anything blocked beyond this reads as obstructed rather than clear.
pub const OBSTRUCTED_FRACTION_FLOOR: f64 = 0.005;
/// Up to this much blockage is "partial" — a thin branch, not a roofline.
pub const PARTIAL_FRACTION_CEILING: f64 = 0.25;

pub const CELL_UNMAPPED: u8 = 0;
pub const CELL_CLEAR: u8 = 1;
pub const CELL_PARTIAL: u8 = 2;
pub const CELL_OBSTRUCTED: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomePointKind {
    Unmapped,
    Clear,
    Partial,
    Obstructed,
}

impl DomePointKind {
    pub fn code(self) -> u8 {
        match self {
            Self::Unmapped => CELL_UNMAPPED,
            Self::Clear => CELL_CLEAR,
            Self::Partial => CELL_PARTIAL,
            Self::Obstructed => CELL_OBSTRUCTED,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            CELL_CLEAR => Self::Clear,
            CELL_PARTIAL => Self::Partial,
            CELL_OBSTRUCTED => Self::Obstructed,
            _ => Self::Unmapped,
        }
    }

    /// The palette the sky view pins itself to, independent of the app theme:
    /// this is a night sky, not a light-mode card.
    pub fn color(self) -> [f32; 3] {
        match self {
            Self::Unmapped => [0.486, 0.486, 0.486],
            Self::Clear => [1.0, 1.0, 1.0],
            Self::Partial => [0.431, 0.059, 0.059],
            Self::Obstructed => [0.961, 0.118, 0.118],
        }
    }
}

/// A live cell is the usable fraction; negative means never observed.
pub fn classify_live(fraction_usable: f64) -> DomePointKind {
    if fraction_usable < 0.0 {
        return DomePointKind::Unmapped;
    }
    let obstructed = 1.0 - fraction_usable;
    if obstructed <= OBSTRUCTED_FRACTION_FLOOR {
        DomePointKind::Clear
    } else if obstructed <= PARTIAL_FRACTION_CEILING {
        DomePointKind::Partial
    } else {
        DomePointKind::Obstructed
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Survey {
    pub grid_size: usize,
    pub max_theta_deg: f64,
    /// Row-major, one kind code per cell.
    pub kinds: Vec<u8>,
    pub boresight_azimuth_deg: f64,
    pub boresight_elevation_deg: f64,
    pub frame: ObstructionFrame,
    /// The kit, as `dish_model_for` names it, so the dome draws its mesh.
    pub dish_model: String,
}

const MINI_MODELS: [&str; 2] = ["mini1", "mini2"];

/// Mini kits report a terminal-relative grid; everything else is earth-fixed.
pub fn family_for_model(model: &str) -> MountFamily {
    if MINI_MODELS.contains(&model) {
        MountFamily::Mini
    } else {
        MountFamily::Other
    }
}

impl Survey {
    /// Build from the live grid. `None` when the grid is not square or empty,
    /// which is the same reply the TypeScript survey refuses to draw.
    #[allow(clippy::too_many_arguments)]
    pub fn from_live(
        snr: &[f64],
        grid_size: usize,
        max_theta_deg: f64,
        boresight_azimuth_deg: f64,
        boresight_elevation_deg: f64,
        reported_frame: Option<&str>,
        model: &str,
    ) -> Option<Self> {
        if grid_size == 0 || snr.len() != grid_size * grid_size {
            return None;
        }
        Some(Self {
            grid_size,
            max_theta_deg,
            kinds: snr
                .iter()
                .map(|value| classify_live(*value).code())
                .collect(),
            boresight_azimuth_deg,
            boresight_elevation_deg,
            frame: resolve_frame(reported_frame, family_for_model(model)),
            dish_model: model.to_owned(),
        })
    }

    /// Build from a stored snapshot, whose cells are already bucketed.
    pub fn from_packed(
        cells: &[u8],
        grid_size: usize,
        max_theta_deg: f64,
        boresight_azimuth_deg: f64,
        boresight_elevation_deg: f64,
        reported_frame: Option<&str>,
        model: &str,
    ) -> Option<Self> {
        if grid_size == 0 || cells.len() < grid_size * grid_size {
            return None;
        }
        Some(Self {
            grid_size,
            max_theta_deg,
            kinds: cells[..grid_size * grid_size]
                .iter()
                .map(|code| DomePointKind::from_code(*code).code())
                .collect(),
            boresight_azimuth_deg,
            boresight_elevation_deg,
            frame: resolve_frame(reported_frame, family_for_model(model)),
            dish_model: model.to_owned(),
        })
    }

    pub fn kind_at(&self, row: usize, col: usize) -> DomePointKind {
        DomePointKind::from_code(self.kinds[row * self.grid_size + col])
    }

    pub fn obstructed_fraction(&self) -> f64 {
        let observed: Vec<u8> = self
            .kinds
            .iter()
            .copied()
            .filter(|code| *code != CELL_UNMAPPED)
            .collect();
        if observed.is_empty() {
            return 0.0;
        }
        let obstructed = observed.iter().filter(|code| **code != CELL_CLEAR).count() as f64;
        obstructed / observed.len() as f64
    }
}

/// Unpack the recorder's 2-bit snapshot cells.
pub fn unpack_cells(packed: &[u8], grid_size: usize) -> Vec<u8> {
    let mut cells = vec![0_u8; grid_size * grid_size];
    for (index, cell) in cells.iter_mut().enumerate() {
        *cell = packed.get(index >> 2).copied().unwrap_or(0) >> ((index & 3) * 2) & 3;
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SKY-001: the TypeScript thresholds decide a cell's kind.
    #[test]
    fn cells_use_the_shared_thresholds() {
        assert_eq!(classify_live(-1.0), DomePointKind::Unmapped);
        assert_eq!(classify_live(1.0), DomePointKind::Clear);
        assert_eq!(classify_live(0.999), DomePointKind::Clear);
        assert_eq!(classify_live(0.99), DomePointKind::Partial);
        assert_eq!(classify_live(0.7), DomePointKind::Obstructed);
    }

    // SPEC-SKY-001: a live survey carries the status boresight and refuses a
    // map that is not a square grid.
    #[test]
    fn a_live_survey_carries_the_replys_shape_and_aim() {
        assert!(Survey::from_live(&[1.0; 4], 3, 80.0, 0.0, 90.0, None, "rev4").is_none());
        let survey = Survey::from_live(
            &[1.0, 0.99, -1.0, -1.0],
            2,
            80.0,
            135.0,
            60.0,
            Some("FRAME_EARTH"),
            "rev4",
        )
        .expect("survey");
        assert_eq!(survey.kind_at(0, 1), DomePointKind::Partial);
        assert_eq!(survey.kind_at(1, 0), DomePointKind::Unmapped);
        assert_eq!(survey.boresight_azimuth_deg, 135.0);
        assert_eq!(survey.frame, ObstructionFrame::Earth);
        assert_eq!(survey.obstructed_fraction(), 0.5);
    }

    // SPEC-SKY-007: stored snapshots unpack in the recorder's cell order.
    #[test]
    fn packed_snapshots_unpack_in_cell_order() {
        // 2, 2, 1, 1 packed two bits each, low cell first.
        let cells = unpack_cells(&[0b0101_1010], 3);
        assert_eq!(cells, vec![2, 2, 1, 1, 0, 0, 0, 0, 0]);
        let survey =
            Survey::from_packed(&cells, 2, 80.0, 0.0, 90.0, None, "mini2").expect("survey");
        assert_eq!(survey.kind_at(0, 0), DomePointKind::Partial);
        assert_eq!(survey.kind_at(0, 1), DomePointKind::Partial);
        assert_eq!(survey.kind_at(1, 0), DomePointKind::Clear);
        assert_eq!(survey.frame, ObstructionFrame::Ut);
    }
}
