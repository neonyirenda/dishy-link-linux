//! Which satellite the dish is probably serving from, and whether a sky
//! direction is clear on the dish's own obstruction map.
//!
//! Ports `lib/servingSlot.ts` and the `isUnobstructed` check in
//! `hooks/useSatellites.ts`. Starlink reassigns terminals on 15-second
//! boundaries of a clock every terminal shares, so the guess is made when the
//! slot turns over and held until the next one, not re-picked on every sample.

use crate::constellation::{SERVING_ELEVATION_FLOOR_DEG, SatelliteLook, serving_candidate};
use crate::frame::{ObstructionFrame, resolve_frame, sky_direction_to_grid};
use crate::survey::family_for_model;

pub const HANDOFF_SLOT_MS: i64 = 15_000;

/// A cell counts as clear when at most this much of it is unusable.
const CLEAR_UNUSABLE_CEILING: f64 = 0.05;

/// Counted from the epoch, so observers on the same clock agree on boundaries.
pub fn slot_index_at(now_ms: i64) -> i64 {
    now_ms.div_euclid(HANDOFF_SLOT_MS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingHold {
    pub slot_index: i64,
    pub name: String,
}

/// Keep the held satellite while its slot lasts and it is still clear and
/// above the floor. Otherwise pick again. A replacement inherits the slot in
/// progress, because losing a satellite does not restart the dish's clock.
pub fn choose_serving_candidate<'a>(
    in_view: &'a [SatelliteLook],
    now_ms: i64,
    hold: Option<&ServingHold>,
    mut is_unobstructed: impl FnMut(&SatelliteLook) -> bool,
) -> (Option<&'a SatelliteLook>, Option<ServingHold>) {
    let slot_index = slot_index_at(now_ms);
    if let Some(hold) = hold
        && hold.slot_index == slot_index
        && let Some(held) = in_view.iter().find(|look| look.name == hold.name)
        && held.elevation_deg >= SERVING_ELEVATION_FLOOR_DEG
        && is_unobstructed(held)
    {
        return (Some(held), Some(hold.clone()));
    }
    let candidate = serving_candidate(in_view, is_unobstructed);
    let hold = candidate.map(|look| ServingHold {
        slot_index,
        name: look.name.clone(),
    });
    (candidate, hold)
}

/// The live obstruction grid, kept in the raw `fraction usable` form the dish
/// reports, so a lookup gives the same answer as the Electron check.
#[derive(Debug, Clone, PartialEq)]
pub struct ObstructionLookup {
    snr: Vec<f64>,
    grid_size: usize,
    max_theta_deg: f64,
    frame: ObstructionFrame,
    boresight_azimuth_deg: f64,
}

impl ObstructionLookup {
    /// `None` when the grid is empty or not square.
    pub fn new(
        snr: &[f64],
        grid_size: usize,
        max_theta_deg: f64,
        reported_frame: Option<&str>,
        model: &str,
        boresight_azimuth_deg: f64,
    ) -> Option<Self> {
        if grid_size == 0 || snr.len() != grid_size * grid_size {
            return None;
        }
        Some(Self {
            snr: snr.to_vec(),
            grid_size,
            max_theta_deg,
            frame: resolve_frame(reported_frame, family_for_model(model)),
            boresight_azimuth_deg,
        })
    }

    /// Outside the mapped cone is never clear. An unmeasured cell (negative)
    /// is given the benefit of the doubt, as the Electron check does.
    pub fn is_unobstructed(&self, look: &SatelliteLook) -> bool {
        let radial_fraction = (90.0 - look.elevation_deg) / self.max_theta_deg;
        if radial_fraction > 1.0 {
            return false;
        }
        let (row, col) = sky_direction_to_grid(
            look.azimuth_deg,
            radial_fraction,
            self.grid_size,
            self.frame,
            self.boresight_azimuth_deg,
        );
        let usable = self.snr[row * self.grid_size + col];
        usable < 0.0 || 1.0 - usable <= CLEAR_UNUSABLE_CEILING
    }
}

/// Is a missing map clear? Yes: with no map there is nothing to rule a
/// satellite out, which matches the Electron fallback.
pub fn is_unobstructed(lookup: Option<&ObstructionLookup>, look: &SatelliteLook) -> bool {
    lookup.is_none_or(|lookup| lookup.is_unobstructed(look))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(name: &str, azimuth_deg: f64, elevation_deg: f64) -> SatelliteLook {
        SatelliteLook {
            name: name.into(),
            azimuth_deg,
            elevation_deg,
            range_km: 800.0,
            altitude_km: 550.0,
            speed_km_s: 7.6,
            enu_km: [0.0; 3],
            enu_velocity_km_s: [0.0; 3],
            attitude: None,
        }
    }

    // SPEC-CONST-008: the guess holds for its 15 s slot while still usable, and a
    // replacement inherits the slot in progress.
    #[test]
    fn the_serving_guess_is_held_for_its_handoff_slot() {
        let in_view = [look("high", 0.0, 80.0), look("lower", 90.0, 40.0)];
        let slot_start = HANDOFF_SLOT_MS * 1_000;
        let (first, hold) = choose_serving_candidate(&in_view, slot_start, None, |_| true);
        assert_eq!(first.map(|look| look.name.as_str()), Some("high"));

        let held = ServingHold {
            slot_index: slot_index_at(slot_start),
            name: "lower".into(),
        };
        let (kept, _) =
            choose_serving_candidate(&in_view, slot_start + 14_000, Some(&held), |_| true);
        assert_eq!(kept.map(|look| look.name.as_str()), Some("lower"));

        let (repicked, next) =
            choose_serving_candidate(&in_view, slot_start + HANDOFF_SLOT_MS, Some(&held), |_| {
                true
            });
        assert_eq!(repicked.map(|look| look.name.as_str()), Some("high"));
        assert_eq!(
            next.map(|hold| hold.slot_index),
            Some(slot_index_at(slot_start) + 1)
        );

        let (replaced, inherited) =
            choose_serving_candidate(&in_view, slot_start + 5_000, Some(&held), |look| {
                look.name != "lower"
            });
        assert_eq!(replaced.map(|look| look.name.as_str()), Some("high"));
        assert_eq!(
            inherited.map(|hold| hold.slot_index),
            hold.map(|hold| hold.slot_index)
        );
    }

    // SPEC-CONST-008: the clear test reads the dish's raw grid the same way the
    // Electron check does.
    #[test]
    fn obstruction_lookup_matches_the_electron_rule() {
        // 3×3 earth-frame grid: centre (zenith) clear, north cell blocked.
        let mut snr = vec![1.0; 9];
        snr[1] = 0.2; // row 0, col 1 = north edge
        snr[5] = -1.0; // east edge, unmeasured
        let lookup =
            ObstructionLookup::new(&snr, 3, 80.0, Some("FRAME_EARTH"), "rev4Standard", 0.0)
                .expect("square grid");
        assert!(lookup.is_unobstructed(&look("zenith", 0.0, 89.0)));
        assert!(!lookup.is_unobstructed(&look("north-low", 0.0, 10.0)));
        assert!(lookup.is_unobstructed(&look("east-unmeasured", 90.0, 10.0)));
        assert!(!lookup.is_unobstructed(&look("below-cone", 90.0, 5.0)));
        assert!(is_unobstructed(None, &look("no-map", 0.0, 10.0)));
        assert!(ObstructionLookup::new(&snr[..8], 3, 80.0, None, "unknown", 0.0).is_none());
    }
}
