//! The alignment math, ported 1:1 from the dish's own web app via
//! `components/alignment/alignmentMath.ts`. This is SpaceX's algorithm, not an
//! interpretation:
//!   - alignment logic  = their `Nd`: great-circle separation < 5° against a
//!     target elevation band (70°…75° for fixed standard kits, up to 90° for
//!     mobile/low-tilt kits), azimuth tolerance widening near zenith
//!   - azimuth tolerance = their `Ld` (spherical law of cosines solved for the
//!     azimuth offset that produces a 5° separation)
//!   - separation        = their `Ed`
//!
//! Kept apart from the instruments that draw it: this is the whole reason the
//! panel can say "aligned", and it is checkable without a GUI.

use crate::telemetry::DishStatus;

pub const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
pub const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;
pub const SEPARATION_LIMIT_DEG: f64 = 5.0;

/// Their `Ed`: great-circle angular separation between two pointing directions.
pub fn angular_separation_deg(
    azimuth_a: f64,
    elevation_a: f64,
    azimuth_b: f64,
    elevation_b: f64,
) -> f64 {
    let cos_separation = (elevation_a * DEG_TO_RAD).sin() * (elevation_b * DEG_TO_RAD).sin()
        + (elevation_a * DEG_TO_RAD).cos()
            * (elevation_b * DEG_TO_RAD).cos()
            * ((azimuth_a - azimuth_b) * DEG_TO_RAD).cos();
    let separation = cos_separation.clamp(-1.0, 1.0).acos() * RAD_TO_DEG;
    if separation.is_nan() { 0.0 } else { separation }
}

/// Their `Ld`: azimuth offset at which separation hits 5°, given the two elevations.
pub fn azimuth_tolerance_deg(target_elevation: f64, current_elevation: f64) -> f64 {
    let target_rad = target_elevation * DEG_TO_RAD;
    let current_rad = current_elevation * DEG_TO_RAD;
    let limit_rad = SEPARATION_LIMIT_DEG * DEG_TO_RAD;
    let denominator = target_rad.cos() * current_rad.cos();
    let cos_azimuth = (limit_rad.cos() - current_rad.sin() * target_rad.sin()) / denominator;
    if cos_azimuth < -1.0 {
        return 180.0;
    }
    let tolerance = cos_azimuth.acos() * RAD_TO_DEG;
    if tolerance.is_nan() { 0.0 } else { tolerance }
}

/// Signed shortest way round, in (-180, 180].
pub fn wrap_degrees(angle_deg: f64) -> f64 {
    let wrapped = angle_deg.rem_euclid(360.0);
    if wrapped < 180.0 {
        wrapped
    } else {
        wrapped - 360.0
    }
}

/// The kit models SpaceX's own web app distinguishes (their `zl`). One variant
/// per model, resolved once, so geometry and alignment can't disagree about
/// which dish this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DishModel {
    Rev2Circular,
    Rev3Rectangular,
    Rev4Standard,
    Rev5Standard,
    PerformanceGen1,
    PerformanceGen2,
    PerformanceGen3,
    Aviation,
    Mini1,
    Mini2,
    /// Hardware this table doesn't know. Draws the Standard as a stand-in, but
    /// is named as unknown rather than claiming to be a kit the user doesn't
    /// own.
    Unknown,
}

/// Their `Vl`, ported: the model from the hardware string plus the actuator
/// flag.
///
/// Prefix tests, in their order, and the order carries meaning — every
/// Performance Gen 3 reports a `rev4_hp…` string, so testing `rev4` first would
/// swallow it. A substring match does the same thing, which is why this is
/// prefix-based rather than a list of regexes.
///
/// Some families spell the revision with a redundant `rev_` before the real
/// token — `rev_hp1_proto0` (Performance Gen 1 and 2), `rev_rev1_proto3` (a V1
/// prototype). Stripping it lets the one prefix table below cover both
/// spellings; without it those strings match nothing and fall through to
/// `unknown`, drawing a mast-mounted Performance kit as a Standard on a
/// kickstand.
///
/// `high_perf` and `flat_hp` are deliberately not matched: no dish has been
/// observed reporting either, and an unmatched dish falls through to `unknown`
/// rather than asserting a guess.
pub fn resolve_dish_model(hardware_version: Option<&str>, motorised: bool) -> DishModel {
    let raw = hardware_version.unwrap_or("").to_ascii_lowercase();
    let hardware = raw.strip_prefix("rev_").unwrap_or(&raw);
    if hardware.is_empty() {
        return DishModel::Unknown;
    }
    // Before both HP tests: the aviation kits spell themselves `hp1_aviation_*`
    // and `rev4_hp_aviation_*`, so either prefix below would swallow them and
    // draw an airliner's terminal as a mast or flat Performance kit.
    if hardware.contains("aviation") {
        DishModel::Aviation
    } else if hardware.starts_with("hp") {
        if motorised {
            DishModel::PerformanceGen1
        } else {
            DishModel::PerformanceGen2
        }
    } else if hardware.starts_with("rev3") || hardware.starts_with("dishy") {
        DishModel::Rev3Rectangular
    } else if hardware.starts_with("rev1") || hardware.starts_with("rev2") {
        DishModel::Rev2Circular
    } else if hardware.starts_with("rev4_hp") {
        DishModel::PerformanceGen3
    } else if hardware.starts_with("rev4") {
        DishModel::Rev4Standard
    } else if hardware.starts_with("rev5") {
        DishModel::Rev5Standard
    } else if hardware.starts_with("mini1") {
        DishModel::Mini1
    } else if hardware.starts_with("mini2") {
        DishModel::Mini2
    } else {
        DishModel::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DishMount {
    Kickstand,
    Mast,
    Flat,
}

/// The per-model facts that are not geometry. `default_tilt_deg` is from the
/// dish web app's own model table.
pub fn spec_for_model(model: DishModel) -> (&'static str, DishMount, f64) {
    match model {
        DishModel::Rev4Standard => ("Standard 4", DishMount::Kickstand, 20.0),
        DishModel::Rev5Standard => ("Starlink V5", DishMount::Kickstand, 13.0),
        DishModel::Rev3Rectangular => ("Actuated", DishMount::Mast, 25.0),
        DishModel::Rev2Circular => ("Standard Circular", DishMount::Mast, 25.0),
        DishModel::Mini1 => ("Mini", DishMount::Kickstand, 20.0),
        DishModel::Mini2 => ("Mini 2", DishMount::Kickstand, 20.0),
        // The app names Performance kits by physical generation: the mast kit
        // is Gen 1, the flat rev_hp1 kit is Gen 2, the flat rev4_hp kit is
        // Gen 3.
        DishModel::PerformanceGen1 => ("Performance (Gen 1)", DishMount::Mast, 25.0),
        DishModel::PerformanceGen2 => ("Performance (Gen 2)", DishMount::Flat, 0.0),
        DishModel::PerformanceGen3 => ("Performance (Gen 3)", DishMount::Flat, 0.0),
        // Flat like the Performance kits it is built from, so it clears the 8°
        // bar and may aim to zenith — right for a terminal that spends its life
        // level.
        DishModel::Aviation => ("Aviation", DishMount::Flat, 0.0),
        // Mount and tilt match the Standard, whose body stands in for it:
        // unrecognised hardware gets the commonest kit's alignment limits, and
        // a name that admits it is a guess rather than asserting a model.
        DishModel::Unknown => ("Unknown Model", DishMount::Kickstand, 20.0),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignmentReading {
    pub is_valid: bool,
    pub is_aligned: bool,
    pub boresight_azimuth_deg: f64,
    pub boresight_elevation_deg: f64,
    pub desired_azimuth_deg: f64,
    pub azimuth_tolerance_deg: f64,
    pub upper_elevation_limit_deg: f64,
    pub lower_elevation_limit_deg: f64,
    pub is_elevation_valid: bool,
    /// Floor of the acceptable elevation band, NOT the dish's target: their
    /// `Nd` caps it at 70° (`min(70, desired)`), so a dish asking for 76° still
    /// yields 70 here and is judged against the band [70°, 75°]. Anything that
    /// wants the dish's actual target — a pointing error, a printed figure —
    /// must use `desired_elevation_deg`, which is unclamped.
    pub target_elevation_deg: f64,
    pub max_target_elevation_deg: f64,
    /// The dish's own target elevation, exactly as it reports it. proto3 omits
    /// a zero, so an absent field rides in as 0 and falls back to the current
    /// elevation.
    pub desired_elevation_deg: f64,
    /// Great-circle angle between where the dish points and where it wants to
    /// point — the single figure Starlink support quotes as "boresight error".
    pub boresight_error_deg: f64,
    /// Signed current − target, per axis, for the "move it this way" readouts
    /// the old web portal called its tilt and rotate recommendations.
    pub elevation_error_deg: f64,
    pub azimuth_error_deg: f64,
    pub tilt_angle_deg: f64,
}

impl Default for AlignmentReading {
    fn default() -> Self {
        Self {
            is_valid: false,
            is_aligned: false,
            boresight_azimuth_deg: 0.0,
            boresight_elevation_deg: 0.0,
            desired_azimuth_deg: 0.0,
            azimuth_tolerance_deg: 0.0,
            upper_elevation_limit_deg: 0.0,
            lower_elevation_limit_deg: 0.0,
            is_elevation_valid: false,
            target_elevation_deg: 0.0,
            max_target_elevation_deg: 0.0,
            desired_elevation_deg: 0.0,
            boresight_error_deg: 0.0,
            elevation_error_deg: 0.0,
            azimuth_error_deg: 0.0,
            tilt_angle_deg: 0.0,
        }
    }
}

/// Their `Nd`, ported. Uses top-level boresight fields exactly as their code
/// does. (SPEC-ALIGN-001: the reading is pure — status in, figures out.)
pub fn compute_alignment(status: &DishStatus) -> AlignmentReading {
    let stats = status.alignment_stats.as_ref();
    // Their band ceiling, ported exactly:
    //   (mobilityClass === MOBILE || defaultTiltDeg < 8) ? 90 : 75
    // Only the flat kits (Performance Gen 2 and Gen 3, both 0°) clear the 8°
    // bar — the Mini's default tilt is 20°, so it takes 75 like a standard kit.
    // A MOBILE install gets 90 whatever the model, since it can be aimed from
    // anywhere. Top-level `hasActuators` only, as their `Vl` reads it — not the
    // copy on alignmentStats. The two can disagree, and on an `hp` string this
    // flag alone decides Gen 1 (tilt 25°, ceiling 75°) versus Gen 2 (tilt 0°,
    // ceiling 90°). (SPEC-ALIGN-003: the ceiling follows the kit identity.)
    let motorised = status.has_actuators.as_deref() == Some("HAS_ACTUATORS_YES");
    let model = resolve_dish_model(
        status
            .device_info
            .as_ref()
            .and_then(|info| info.hardware_version.as_deref()),
        motorised,
    );
    let max_target_elevation =
        if status.mobility_class.as_deref() == Some("MOBILE") || spec_for_model(model).2 < 8.0 {
            90.0
        } else {
            75.0
        };
    let desired_elevation_raw = stats
        .map(|stats| stats.desired_boresight_elevation_deg.0)
        .unwrap_or(0.0);
    let target_elevation = if desired_elevation_raw != 0.0 {
        70.0f64.min(desired_elevation_raw)
    } else {
        70.0
    };
    let desired_azimuth = stats
        .map(|stats| stats.desired_boresight_azimuth_deg.0)
        .unwrap_or(0.0);
    let current_azimuth = status.boresight_azimuth_deg.0;
    let current_elevation = status.boresight_elevation_deg.0;

    let separation_at_target = angular_separation_deg(
        desired_azimuth,
        target_elevation,
        current_azimuth,
        current_elevation,
    );
    let separation_at_band_top = angular_separation_deg(
        desired_azimuth,
        max_target_elevation,
        current_azimuth,
        current_elevation,
    );
    let azimuth_diff = wrap_degrees(desired_azimuth - current_azimuth);
    let is_valid = matches!(
        stats.and_then(|stats| stats.attitude_estimation_state.as_deref()),
        Some("FILTER_CONVERGED") | Some("FILTER_UNCONVERGED")
    );
    let band_usable = target_elevation >= 50.0;

    // their `w`: azimuth error projected onto the sky at the current elevation
    let effective_azimuth_error = ((azimuth_diff * DEG_TO_RAD).cos()).powi(2)
        * (current_elevation * DEG_TO_RAD).cos().powi(2)
        + (current_elevation * DEG_TO_RAD).sin().powi(2);
    let effective_azimuth_error = effective_azimuth_error.sqrt().acos() * RAD_TO_DEG;

    let aligned_at_target = is_valid && separation_at_target < SEPARATION_LIMIT_DEG;
    let aligned_at_band_top =
        is_valid && band_usable && separation_at_band_top < SEPARATION_LIMIT_DEG;
    let aligned_inside_band = is_valid
        && band_usable
        && current_elevation > target_elevation
        && current_elevation < max_target_elevation
        && azimuth_diff.abs() < 90.0
        && effective_azimuth_error.abs() < SEPARATION_LIMIT_DEG;
    let is_aligned = aligned_at_target || aligned_at_band_top || aligned_inside_band;

    let tolerance = (azimuth_tolerance_deg(target_elevation, current_elevation))
        .max(if band_usable {
            azimuth_tolerance_deg(max_target_elevation, current_elevation)
        } else {
            0.0
        })
        .max(
            if band_usable
                && current_elevation > target_elevation
                && current_elevation < max_target_elevation
            {
                azimuth_tolerance_deg(current_elevation, current_elevation)
            } else {
                0.0
            },
        );
    let upper_limit = ((if band_usable {
        max_target_elevation
    } else {
        target_elevation
    }) + 5.0)
        .clamp(0.0, 90.0);
    let lower_limit = (target_elevation - 5.0).clamp(0.0, 90.0);

    // Pointing error against the dish's *unclamped* target. Using
    // target_elevation here would read 6.5° on a dish asking for 76°, which is
    // band arithmetic, not a pointing error. (SPEC-ALIGN-004.)
    let desired_elevation = if desired_elevation_raw != 0.0 {
        desired_elevation_raw
    } else {
        current_elevation
    };
    let tilt_angle = stats.map(|stats| stats.tilt_angle_deg.0).unwrap_or(0.0);

    AlignmentReading {
        desired_elevation_deg: desired_elevation,
        boresight_error_deg: angular_separation_deg(
            current_azimuth,
            current_elevation,
            desired_azimuth,
            desired_elevation,
        ),
        elevation_error_deg: current_elevation - desired_elevation,
        azimuth_error_deg: wrap_degrees(current_azimuth - desired_azimuth),
        tilt_angle_deg: tilt_angle,
        is_valid,
        is_aligned,
        boresight_azimuth_deg: current_azimuth,
        boresight_elevation_deg: current_elevation,
        desired_azimuth_deg: desired_azimuth,
        azimuth_tolerance_deg: tolerance,
        upper_elevation_limit_deg: upper_limit,
        lower_elevation_limit_deg: lower_limit,
        is_elevation_valid: current_elevation > lower_limit && current_elevation < upper_limit,
        target_elevation_deg: target_elevation,
        max_target_elevation_deg: max_target_elevation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{AlignmentStats, DishDeviceInfo, ProtoF64};

    // SPEC-ALIGN-001: the reading is computed from a plain status reply — no
    // device client, recorder, or transport in scope here.

    /// Status with only what the math reads, shaped like `statusAt` in the TS test.
    fn status_at(
        boresight_azimuth_deg: f64,
        boresight_elevation_deg: f64,
        stats_overrides: impl FnOnce(&mut AlignmentStats),
    ) -> DishStatus {
        let mut stats = AlignmentStats {
            attitude_estimation_state: Some("FILTER_CONVERGED".into()),
            desired_boresight_azimuth_deg: ProtoF64(0.0),
            desired_boresight_elevation_deg: ProtoF64(70.0),
            ..Default::default()
        };
        stats_overrides(&mut stats);
        DishStatus {
            boresight_azimuth_deg: ProtoF64(boresight_azimuth_deg),
            boresight_elevation_deg: ProtoF64(boresight_elevation_deg),
            alignment_stats: Some(stats),
            ..Default::default()
        }
    }

    #[test]
    fn separation_is_zero_for_a_direction_against_itself() {
        assert!(angular_separation_deg(137.0, 62.0, 137.0, 62.0).abs() < 1e-9);
    }

    #[test]
    fn separation_is_symmetric() {
        let a = angular_separation_deg(10.0, 40.0, 80.0, 65.0);
        let b = angular_separation_deg(80.0, 65.0, 10.0, 40.0);
        assert!((a - b).abs() < 1e-9);
    }

    #[test]
    fn separation_reduces_to_the_elevation_difference_when_azimuth_matches() {
        assert!((angular_separation_deg(90.0, 70.0, 90.0, 55.0) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn separation_ignores_azimuth_entirely_at_the_zenith() {
        // Straight up has no bearing, so any azimuth pair must agree.
        assert!(angular_separation_deg(0.0, 90.0, 180.0, 90.0).abs() < 1e-9);
    }

    #[test]
    fn separation_gives_180_for_exactly_opposite_directions() {
        assert!((angular_separation_deg(0.0, 90.0, 0.0, -90.0) - 180.0).abs() < 1e-6);
    }

    #[test]
    fn separation_never_returns_nan_when_the_cosine_overruns_from_rounding() {
        // acos of anything a hair outside [-1,1] is NaN; the port clamps first.
        for elevation in [-90.0, 0.0, 45.0, 90.0] {
            assert!(!angular_separation_deg(0.0, elevation, 0.0, elevation).is_nan());
        }
    }

    #[test]
    fn tolerance_equals_the_separation_limit_when_both_point_at_the_horizon() {
        // At elevation 0 an azimuth offset IS the great-circle separation.
        assert!((azimuth_tolerance_deg(0.0, 0.0) - SEPARATION_LIMIT_DEG).abs() < 1e-9);
    }

    #[test]
    fn tolerance_widens_as_the_dish_points_higher() {
        // Near the zenith a given azimuth error sweeps a much smaller arc, so
        // more azimuth error is tolerable — this is the whole point of the
        // function.
        let low = azimuth_tolerance_deg(30.0, 30.0);
        let high = azimuth_tolerance_deg(80.0, 80.0);
        assert!(high > low);
    }

    #[test]
    fn tolerance_saturates_to_180_when_every_azimuth_is_within_the_limit() {
        // Two directions this close in elevation near the zenith can never be
        // 5° apart no matter the bearing.
        assert_eq!(azimuth_tolerance_deg(89.0, 89.0), 180.0);
    }

    #[test]
    fn tolerance_is_consistent_with_the_separation_it_solves_for() {
        // Offsetting azimuth by exactly the tolerance should land on ~5° apart.
        let (target, current) = (70.0, 70.0);
        let tolerance = azimuth_tolerance_deg(target, current);
        assert!(
            (angular_separation_deg(0.0, target, tolerance, current) - SEPARATION_LIMIT_DEG).abs()
                < 1e-3
        );
    }

    #[test]
    fn wrap_leaves_a_small_positive_angle_alone() {
        assert_eq!(wrap_degrees(10.0), 10.0);
    }

    #[test]
    fn wrap_takes_the_short_way_round_past_180() {
        assert_eq!(wrap_degrees(350.0), -10.0);
        assert_eq!(wrap_degrees(190.0), -170.0);
    }

    #[test]
    fn wrap_normalises_negatives_and_multiple_turns() {
        assert_eq!(wrap_degrees(-10.0), -10.0);
        assert_eq!(wrap_degrees(730.0), 10.0);
        assert_eq!(wrap_degrees(-370.0), -10.0);
    }

    // SPEC-ALIGN-002: the band judgment.

    #[test]
    fn reports_aligned_when_pointing_straight_at_the_target() {
        let reading = compute_alignment(&status_at(0.0, 70.0, |_| {}));
        assert!(reading.is_valid);
        assert!(reading.is_aligned);
        assert!(reading.is_elevation_valid);
    }

    #[test]
    fn reports_misaligned_when_swung_well_off_the_desired_azimuth() {
        assert!(!compute_alignment(&status_at(90.0, 70.0, |_| {})).is_aligned);
    }

    #[test]
    fn stays_aligned_anywhere_inside_the_70_to_75_band() {
        // The band, not a single angle, is the target — the branch that
        // distinguishes the port from a naive "within 5° of 70°".
        assert!(compute_alignment(&status_at(0.0, 73.0, |_| {})).is_aligned);
    }

    #[test]
    fn is_neither_valid_nor_aligned_before_the_attitude_filter_converges() {
        let reading = compute_alignment(&status_at(0.0, 70.0, |stats| {
            stats.attitude_estimation_state = Some("FILTER_INITIALIZING".into());
        }));
        assert!(!reading.is_valid);
        assert!(!reading.is_aligned);
    }

    #[test]
    fn treats_an_unconverged_but_running_filter_as_valid() {
        let reading = compute_alignment(&status_at(0.0, 70.0, |stats| {
            stats.attitude_estimation_state = Some("FILTER_UNCONVERGED".into());
        }));
        assert!(reading.is_valid);
    }

    #[test]
    fn brackets_the_elevation_band_with_5_degrees_of_slack_on_each_side() {
        let reading = compute_alignment(&status_at(0.0, 70.0, |_| {}));
        assert_eq!(reading.lower_elevation_limit_deg, 65.0); // target 70 − 5
        assert_eq!(reading.upper_elevation_limit_deg, 80.0); // band top 75 + 5
        assert_eq!(reading.target_elevation_deg, 70.0);
        assert_eq!(reading.max_target_elevation_deg, 75.0);
    }

    #[test]
    fn marks_elevation_invalid_outside_those_limits() {
        assert!(!compute_alignment(&status_at(0.0, 40.0, |_| {})).is_elevation_valid);
        assert!(!compute_alignment(&status_at(0.0, 88.0, |_| {})).is_elevation_valid);
    }

    #[test]
    fn never_clamps_a_limit_above_90() {
        let reading = compute_alignment(&status_at(0.0, 70.0, |stats| {
            stats.desired_boresight_elevation_deg = ProtoF64(89.0);
        }));
        assert!(reading.upper_elevation_limit_deg <= 90.0);
        assert!(reading.lower_elevation_limit_deg >= 0.0);
    }

    #[test]
    fn falls_back_to_a_70_target_when_the_dish_reports_none() {
        // proto3 omits a zero, so 0 means "not sent" rather than "aim at the
        // horizon".
        let reading = compute_alignment(&status_at(0.0, 70.0, |stats| {
            stats.desired_boresight_elevation_deg = ProtoF64(0.0);
        }));
        assert_eq!(reading.target_elevation_deg, 70.0);
    }

    // SPEC-ALIGN-003: kit identity drives the ceiling.

    #[test]
    fn reads_a_performance_gen3_as_its_own_model_not_as_a_standard() {
        // Every Performance Gen 3 string also starts with "rev4"; testing
        // "rev4" first — or matching it as a substring — swallows this case.
        assert_eq!(
            resolve_dish_model(Some("rev4_hp_prod1"), false),
            DishModel::PerformanceGen3
        );
        assert_eq!(
            resolve_dish_model(Some("rev4_panda_prod2"), false),
            DishModel::Rev4Standard
        );
    }

    #[test]
    fn splits_the_hp_line_on_whether_it_actually_has_motors() {
        assert_eq!(
            resolve_dish_model(Some("hp1_prod0"), true),
            DishModel::PerformanceGen1
        );
        assert_eq!(
            resolve_dish_model(Some("hp1_prod0"), false),
            DishModel::PerformanceGen2
        );
    }

    #[test]
    fn resolves_an_unknown_or_absent_string_to_the_unknown_kit() {
        assert_eq!(resolve_dish_model(None, false), DishModel::Unknown);
        assert_eq!(
            resolve_dish_model(Some("something_new"), false),
            DishModel::Unknown
        );
    }

    #[test]
    fn strips_the_redundant_rev_prefix_from_family_strings() {
        assert_eq!(
            resolve_dish_model(Some("rev_hp1_proto0"), false),
            DishModel::PerformanceGen2
        );
    }

    #[test]
    fn kit_specs_carry_the_web_apps_default_tilts() {
        assert_eq!(spec_for_model(DishModel::Rev4Standard).2, 20.0);
        assert_eq!(spec_for_model(DishModel::Mini1).2, 20.0);
        assert_eq!(spec_for_model(DishModel::PerformanceGen2).2, 0.0);
        assert_eq!(spec_for_model(DishModel::Unknown).2, 20.0);
    }

    #[test]
    fn holds_a_standard_kit_to_the_75_band() {
        let mut status = status_at(0.0, 85.0, |_| {});
        status.device_info = Some(DishDeviceInfo {
            hardware_version: Some("rev4_panda_prod2".into()),
            ..Default::default()
        });
        assert!(!compute_alignment(&status).is_aligned);
    }

    #[test]
    fn holds_a_mini_to_the_75_band_too() {
        // Its default tilt is 20°, not under 8° — the Mini is not a flat kit.
        let mut status = status_at(0.0, 85.0, |_| {});
        status.device_info = Some(DishDeviceInfo {
            hardware_version: Some("mini1_panda_prod1".into()),
            ..Default::default()
        });
        assert!(!compute_alignment(&status).is_aligned);
    }

    #[test]
    fn lets_a_flat_kit_aim_to_the_zenith() {
        let mut flat_performance = status_at(0.0, 85.0, |_| {});
        flat_performance.device_info = Some(DishDeviceInfo {
            hardware_version: Some("hp1_prod0".into()),
            ..Default::default()
        });
        flat_performance.has_actuators = Some("HAS_ACTUATORS_NO".into());
        let reading = compute_alignment(&flat_performance);
        assert!(reading.is_aligned);
        assert_eq!(reading.upper_elevation_limit_deg, 90.0);

        let mut gen3 = status_at(0.0, 85.0, |_| {});
        gen3.device_info = Some(DishDeviceInfo {
            hardware_version: Some("rev4_hp_prod1".into()),
            ..Default::default()
        });
        assert!(compute_alignment(&gen3).is_aligned);
    }

    #[test]
    fn lets_a_mobile_install_aim_to_the_zenith_whatever_the_model() {
        let mut roaming = status_at(0.0, 85.0, |_| {});
        roaming.device_info = Some(DishDeviceInfo {
            hardware_version: Some("rev4_panda_prod2".into()),
            ..Default::default()
        });
        roaming.mobility_class = Some("MOBILE".into());
        let reading = compute_alignment(&roaming);
        assert!(reading.is_aligned);
        assert_eq!(reading.upper_elevation_limit_deg, 90.0);
    }

    // SPEC-ALIGN-004: the figures are the dish's own, not band arithmetic.

    #[test]
    fn boresight_error_and_axis_errors_use_the_unclamped_target() {
        // A dish asked for 76° must not read 6.5° of pointing error (band floor
        // clamped at 70) — the error is against 76.
        let mut status = status_at(3.0, 76.0, |stats| {
            stats.desired_boresight_azimuth_deg = ProtoF64(0.0);
            stats.desired_boresight_elevation_deg = ProtoF64(76.0);
        });
        status.device_info = Some(DishDeviceInfo {
            hardware_version: Some("hp1_prod0".into()),
            ..Default::default()
        });
        status.has_actuators = Some("HAS_ACTUATORS_NO".into());
        let reading = compute_alignment(&status);
        assert_eq!(reading.target_elevation_deg, 70.0);
        assert_eq!(reading.desired_elevation_deg, 76.0);
        // 3° of azimuth at 76° elevation is 3°·cos(76°) of great-circle arc.
        assert!(
            (reading.boresight_error_deg - 3.0 * (76.0f64.to_radians()).cos()).abs() < 1e-3,
            "boresight_error_deg={}",
            reading.boresight_error_deg
        );
        assert_eq!(reading.elevation_error_deg, 0.0);
        assert_eq!(reading.azimuth_error_deg, 3.0);
    }

    #[test]
    fn azimuth_error_wraps_take_the_short_way_round() {
        // Desired 350°, actual 10°: the signed short way is +20.
        let reading = compute_alignment(&status_at(10.0, 70.0, |stats| {
            stats.desired_boresight_azimuth_deg = ProtoF64(350.0);
        }));
        assert_eq!(reading.azimuth_error_deg, 20.0);
    }
}
