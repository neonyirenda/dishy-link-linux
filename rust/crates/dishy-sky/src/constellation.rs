//! TLE parsing, SGP4 propagation, and observer-local satellite look angles.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sgp4::{Constants, Elements, MinutesSinceEpoch};
use thiserror::Error;

pub const SERVING_ELEVATION_FLOOR_DEG: f64 = 25.0;
const FINE_ELEVATION_FLOOR_DEG: f64 = 2.0;
/// The Electron forecast samples the next half hour at these offsets and keeps
/// the smallest serviceable count.
const FORECAST_OFFSETS_MINUTES: [i64; 6] = [5, 10, 15, 20, 25, 30];
const WGS84_EQUATORIAL_RADIUS_KM: f64 = 6378.137;
const WGS84_ECCENTRICITY_SQUARED: f64 = 6.694_379_990_14e-3;

/// Field names match the TypeScript `ObserverLocation`, so a saved location
/// reads the same in either host.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObserverLocation {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
}

impl ObserverLocation {
    pub fn is_valid(self) -> bool {
        self.latitude_deg.is_finite()
            && (-90.0..=90.0).contains(&self.latitude_deg)
            && self.longitude_deg.is_finite()
            && (-180.0..=180.0).contains(&self.longitude_deg)
            && self.altitude_m.is_finite()
            && (-500.0..=100_000.0).contains(&self.altitude_m)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SatelliteLook {
    pub name: String,
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub range_km: f64,
    pub altitude_km: f64,
    /// Orbital speed from the propagated velocity, for the satellite callout.
    pub speed_km_s: f64,
    /// Position relative to the observer, east/north/up (spec 0182).
    pub enu_km: [f64; 3],
    /// Earth-fixed velocity in the same axes; zero when unknown.
    pub enu_velocity_km_s: [f64; 3],
    /// The craft's orbital frame, for the 3-D model (spec 0184).
    pub attitude: Option<SatelliteAttitude>,
}

/// Unit vectors in east/north/up: radial points away from Earth's centre,
/// along-track is the direction of flight in the orbital plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SatelliteAttitude {
    pub radial: [f64; 3],
    pub along_track: [f64; 3],
}

impl SatelliteLook {
    /// Azimuth, elevation, and range `dt_seconds` after the sample, stepped
    /// along the ENU velocity. A look without a velocity stays put.
    pub fn advanced(&self, dt_seconds: f64) -> (f64, f64, f64) {
        if !self.has_velocity() {
            return (self.azimuth_deg, self.elevation_deg, self.range_km);
        }
        let [east, north, up] =
            [0, 1, 2].map(|axis| self.enu_km[axis] + self.enu_velocity_km_s[axis] * dt_seconds);
        let range_km = east.hypot(north).hypot(up).max(f64::MIN_POSITIVE);
        (
            east.atan2(north).to_degrees().rem_euclid(360.0),
            (up / range_km).clamp(-1.0, 1.0).asin().to_degrees(),
            range_km,
        )
    }

    pub fn has_velocity(&self) -> bool {
        self.enu_velocity_km_s
            .iter()
            .any(|component| *component != 0.0)
    }
}

#[derive(Clone)]
struct OrbitalSatellite {
    name: String,
    elements: Elements,
    constants: Constants,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConstellationError {
    #[error("observer coordinates are invalid")]
    InvalidObserver,
    #[error("TLE catalog contains no valid records")]
    EmptyCatalog,
}

/// A parsed catalog. Invalid records are skipped while the valid records remain
/// usable; `rejected_records` includes incomplete trailing lines.
#[derive(Clone)]
pub struct TleCatalog {
    satellites: Vec<OrbitalSatellite>,
    rejected_records: usize,
}

impl TleCatalog {
    pub fn parse(text: &str) -> Result<Self, ConstellationError> {
        let lines: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let mut satellites = Vec::new();
        let mut rejected_records = 0;

        for record in lines.chunks(3) {
            let [name, line1, line2] = record else {
                rejected_records += 1;
                continue;
            };
            let name = name.strip_prefix("0 ").unwrap_or(name).trim();
            if name.is_empty() || !line1.starts_with("1 ") || !line2.starts_with("2 ") {
                rejected_records += 1;
                continue;
            }
            let Ok(elements) =
                Elements::from_tle(Some(name.to_owned()), line1.as_bytes(), line2.as_bytes())
            else {
                rejected_records += 1;
                continue;
            };
            let Ok(constants) = Constants::from_elements_afspc_compatibility_mode(&elements) else {
                rejected_records += 1;
                continue;
            };
            satellites.push(OrbitalSatellite {
                name: name.to_owned(),
                elements,
                constants,
            });
        }

        if satellites.is_empty() {
            return Err(ConstellationError::EmptyCatalog);
        }
        Ok(Self {
            satellites,
            rejected_records,
        })
    }

    pub fn satellite_count(&self) -> usize {
        self.satellites.len()
    }

    pub fn rejected_records(&self) -> usize {
        self.rejected_records
    }

    pub fn tracker(
        self,
        observer: ObserverLocation,
    ) -> Result<StarlinkTracker, ConstellationError> {
        if !observer.is_valid() {
            return Err(ConstellationError::InvalidObserver);
        }
        Ok(StarlinkTracker {
            satellites: self.satellites,
            observer,
        })
    }
}

pub struct StarlinkTracker {
    satellites: Vec<OrbitalSatellite>,
    observer: ObserverLocation,
}

impl StarlinkTracker {
    pub fn constellation_size(&self) -> usize {
        self.satellites.len()
    }

    pub fn observer(&self) -> ObserverLocation {
        self.observer
    }

    /// Propagate the catalog for a UTC timestamp and return visible satellites,
    /// sorted from highest elevation to lowest.
    pub fn look_angles_at(&self, at_utc: NaiveDateTime) -> Vec<SatelliteLook> {
        let frame = ObserverFrame::new(self.observer, &at_utc);
        let mut in_view: Vec<SatelliteLook> = self
            .satellites
            .iter()
            .filter_map(|satellite| frame.look(satellite, &at_utc))
            .filter(|look| look.elevation_deg > FINE_ELEVATION_FLOOR_DEG)
            .collect();
        in_view.sort_by(|a, b| b.elevation_deg.total_cmp(&a.elevation_deg));
        in_view
    }

    /// The smallest number of satellites at or above the service floor at any
    /// of the forecast offsets in the next 30 minutes. It sweeps the whole
    /// catalog, because the satellites serving in half an hour are below the
    /// horizon now.
    pub fn forecast_min_serviceable(&self, at_utc: NaiveDateTime) -> usize {
        FORECAST_OFFSETS_MINUTES
            .iter()
            .map(|offset| {
                let later = at_utc + chrono::Duration::minutes(*offset);
                let frame = ObserverFrame::new(self.observer, &later);
                self.satellites
                    .iter()
                    .filter_map(|satellite| frame.look(satellite, &later))
                    .filter(|look| look.elevation_deg >= SERVING_ELEVATION_FLOOR_DEG)
                    .count()
            })
            .min()
            .unwrap_or(0)
    }
}

/// The observer's ECEF position and local axes, plus Earth's rotation at one
/// instant: everything a look angle needs that is shared across satellites.
struct ObserverFrame {
    observer_ecef: [f64; 3],
    sin_lat: f64,
    cos_lat: f64,
    sin_lon: f64,
    cos_lon: f64,
    sin_gmst: f64,
    cos_gmst: f64,
}

impl ObserverFrame {
    fn new(observer: ObserverLocation, at_utc: &NaiveDateTime) -> Self {
        let latitude = observer.latitude_deg.to_radians();
        let longitude = observer.longitude_deg.to_radians();
        let altitude_km = observer.altitude_m / 1_000.0;
        let (sin_lat, cos_lat) = latitude.sin_cos();
        let (sin_lon, cos_lon) = longitude.sin_cos();
        let sidereal_time =
            sgp4::iau_epoch_to_sidereal_time(sgp4::julian_years_since_j2000(at_utc));
        let (sin_gmst, cos_gmst) = sidereal_time.sin_cos();
        Self {
            observer_ecef: observer_ecef(altitude_km, sin_lat, cos_lat, sin_lon, cos_lon),
            sin_lat,
            cos_lat,
            sin_lon,
            cos_lon,
            sin_gmst,
            cos_gmst,
        }
    }

    /// One satellite's look angles at this instant, at any elevation.
    fn look(&self, satellite: &OrbitalSatellite, at_utc: &NaiveDateTime) -> Option<SatelliteLook> {
        let minutes = satellite
            .elements
            .datetime_to_minutes_since_epoch(at_utc)
            .ok()?;
        let prediction = satellite
            .constants
            .propagate_afspc_compatibility_mode(MinutesSinceEpoch(minutes.0))
            .ok()?;
        let [x, y, z] = prediction.position;
        let ecef = [
            x * self.cos_gmst + y * self.sin_gmst,
            -x * self.sin_gmst + y * self.cos_gmst,
            z,
        ];
        let relative = [
            ecef[0] - self.observer_ecef[0],
            ecef[1] - self.observer_ecef[1],
            ecef[2] - self.observer_ecef[2],
        ];
        let [east, north, up] = self.to_enu(relative);
        let range_km = east.hypot(north).hypot(up);
        if !range_km.is_finite() || range_km <= 0.0 {
            return None;
        }
        let mut azimuth_deg = east.atan2(north).to_degrees().rem_euclid(360.0);
        if azimuth_deg == 360.0 {
            azimuth_deg = 0.0;
        }
        let elevation_deg = (up / range_km).clamp(-1.0, 1.0).asin().to_degrees();
        if !elevation_deg.is_finite() {
            return None;
        }
        let [vx, vy, vz] = prediction.velocity;
        // The Earth-fixed velocity: the inertial one rotated like the position,
        // minus the transport term ω × r = (−ω·r_y, ω·r_x, 0). Without it the
        // heading is a few degrees off and the trail drifts sideways. The
        // observer is fixed in ECEF, so this is also the relative velocity.
        let velocity_ecef = [
            vx * self.cos_gmst + vy * self.sin_gmst + EARTH_ROTATION_RAD_S * ecef[1],
            -vx * self.sin_gmst + vy * self.cos_gmst - EARTH_ROTATION_RAD_S * ecef[0],
            vz,
        ];
        Some(SatelliteLook {
            name: satellite.name.clone(),
            azimuth_deg,
            elevation_deg,
            range_km,
            altitude_km: x.hypot(y).hypot(z) - WGS84_EQUATORIAL_RADIUS_KM,
            speed_km_s: vx.hypot(vy).hypot(vz),
            enu_km: [east, north, up],
            enu_velocity_km_s: self.to_enu(velocity_ecef),
            attitude: self.attitude([x, y, z], [vx, vy, vz]),
        })
    }

    /// The orbital (LVLH) frame from the inertial state, in ENU. `None` for a
    /// degenerate velocity, which a decayed TLE can produce.
    fn attitude(&self, position: [f64; 3], velocity: [f64; 3]) -> Option<SatelliteAttitude> {
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let size = |v: [f64; 3]| v[0].hypot(v[1]).hypot(v[2]);
        let cross_track = cross(position, velocity);
        if size(cross_track) < 1e-9 || size(position) == 0.0 {
            return None;
        }
        let radial = position.map(|value| value / size(position));
        let normal = cross_track.map(|value| value / size(cross_track));
        let along_track = cross(normal, radial);
        Some(SatelliteAttitude {
            radial: self.inertial_to_enu(radial),
            along_track: self.inertial_to_enu(along_track),
        })
    }

    /// A direction in inertial axes, turned to Earth-fixed, then to ENU.
    fn inertial_to_enu(&self, v: [f64; 3]) -> [f64; 3] {
        self.to_enu([
            v[0] * self.cos_gmst + v[1] * self.sin_gmst,
            -v[0] * self.sin_gmst + v[1] * self.cos_gmst,
            v[2],
        ])
    }

    /// An Earth-fixed vector in the observer's east/north/up axes.
    fn to_enu(&self, v: [f64; 3]) -> [f64; 3] {
        [
            -self.sin_lon * v[0] + self.cos_lon * v[1],
            -self.sin_lat * self.cos_lon * v[0] - self.sin_lat * self.sin_lon * v[1]
                + self.cos_lat * v[2],
            self.cos_lat * self.cos_lon * v[0]
                + self.cos_lat * self.sin_lon * v[1]
                + self.sin_lat * v[2],
        ]
    }
}

/// Earth's rotation rate, for the velocity transport term.
const EARTH_ROTATION_RAD_S: f64 = 7.292_115_9e-5;

/// Highest unobstructed candidate at the service elevation floor. If all
/// qualifying satellites are obstructed, preserve the Electron fallback and
/// return the highest one anyway.
pub fn serving_candidate(
    satellites: &[SatelliteLook],
    mut is_unobstructed: impl FnMut(&SatelliteLook) -> bool,
) -> Option<&SatelliteLook> {
    let mut qualifying: Vec<_> = satellites
        .iter()
        .filter(|satellite| satellite.elevation_deg >= SERVING_ELEVATION_FLOOR_DEG)
        .collect();
    qualifying.sort_by(|a, b| b.elevation_deg.total_cmp(&a.elevation_deg));
    qualifying
        .iter()
        .copied()
        .find(|satellite| is_unobstructed(satellite))
        .or_else(|| qualifying.first().copied())
}

fn observer_ecef(
    altitude_km: f64,
    sin_lat: f64,
    cos_lat: f64,
    sin_lon: f64,
    cos_lon: f64,
) -> [f64; 3] {
    let prime_vertical =
        WGS84_EQUATORIAL_RADIUS_KM / (1.0 - WGS84_ECCENTRICITY_SQUARED * sin_lat.powi(2)).sqrt();
    [
        (prime_vertical + altitude_km) * cos_lat * cos_lon,
        (prime_vertical + altitude_km) * cos_lat * sin_lon,
        (prime_vertical * (1.0 - WGS84_ECCENTRICITY_SQUARED) + altitude_km) * sin_lat,
    ]
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    const ISS_TLE: &str = "ISS (ZARYA)\n\
1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008\n";
    const VANGUARD_LINE_1: &str =
        "1 00005U 58002B   00179.78495062  .00000023  00000-0  28098-4 0  4753";
    const VANGUARD_LINE_2: &str =
        "2 00005  34.2682 348.7242 1859667 331.7664  19.3264 10.82419157413667";

    // SPEC-CONST-001: a malformed record does not discard its valid neighbors.
    #[test]
    fn catalog_keeps_valid_records_and_counts_rejected_records() {
        let text =
            format!("{ISS_TLE}Broken record\nnot line one\nnot line two\nTrailing name only\n");
        let catalog = TleCatalog::parse(&text).expect("one valid record");
        assert_eq!(catalog.satellite_count(), 1);
        assert_eq!(catalog.rejected_records(), 2);
        assert!(matches!(
            TleCatalog::parse("invalid"),
            Err(ConstellationError::EmptyCatalog)
        ));
    }

    // SPEC-CONST-002: SGP4 output becomes finite local look angles in the
    // north-zero, clockwise azimuth convention used by the sky overlay.
    #[test]
    fn propagation_produces_finite_observer_local_coordinates() {
        let catalog = TleCatalog::parse(ISS_TLE).expect("valid test TLE");
        let at = NaiveDate::from_ymd_opt(2020, 7, 12)
            .unwrap()
            .and_hms_opt(21, 15, 0)
            .unwrap();
        let mut visible = None;
        for latitude_deg in (-60..=60).step_by(10) {
            for longitude_deg in (-180..=180).step_by(10) {
                let tracker = catalog
                    .clone()
                    .tracker(ObserverLocation {
                        latitude_deg: f64::from(latitude_deg),
                        longitude_deg: f64::from(longitude_deg),
                        altitude_m: 0.0,
                    })
                    .expect("valid observer");
                if let Some(look) = tracker.look_angles_at(at).into_iter().next()
                    && visible
                        .as_ref()
                        .is_none_or(|(best, _): &(f64, SatelliteLook)| look.elevation_deg > *best)
                {
                    visible = Some((look.elevation_deg, look));
                }
            }
        }
        let look = &visible.expect("a ground observer sees this orbital pass").1;
        assert!(look.azimuth_deg.is_finite() && (0.0..360.0).contains(&look.azimuth_deg));
        assert!(look.elevation_deg.is_finite() && (-90.0..=90.0).contains(&look.elevation_deg));
        assert!(look.range_km.is_finite() && look.range_km > 0.0);
        assert!(look.altitude_km.is_finite() && (100.0..2_000.0).contains(&look.altitude_km));
    }

    // SPEC-TRAIL-001: the ENU velocity transports a sample forward to within a
    // few hundred metres of a fresh one a second later.
    #[test]
    fn enu_velocity_matches_a_later_sample() {
        let catalog = TleCatalog::parse(ISS_TLE).expect("valid test TLE");
        let at = NaiveDate::from_ymd_opt(2020, 7, 12)
            .unwrap()
            .and_hms_opt(21, 15, 0)
            .unwrap();
        let mut checked = 0;
        for latitude_deg in (-60..=60).step_by(20) {
            for longitude_deg in (-180..=180).step_by(20) {
                let tracker = catalog
                    .clone()
                    .tracker(ObserverLocation {
                        latitude_deg: f64::from(latitude_deg),
                        longitude_deg: f64::from(longitude_deg),
                        altitude_m: 0.0,
                    })
                    .expect("valid observer");
                let Some(now) = tracker.look_angles_at(at).into_iter().next() else {
                    continue;
                };
                let later = tracker
                    .look_angles_at(at + chrono::Duration::seconds(1))
                    .into_iter()
                    .next()
                    .expect("still in view a second later");
                assert!(now.has_velocity());
                let stepped: Vec<f64> = (0..3)
                    .map(|axis| now.enu_km[axis] + now.enu_velocity_km_s[axis])
                    .collect();
                let miss = (0..3)
                    .map(|axis| stepped[axis] - later.enu_km[axis])
                    .map(|d| d * d)
                    .sum::<f64>()
                    .sqrt();
                assert!(miss < 0.3, "transport missed by {miss} km");
                let (azimuth, elevation, range) = now.advanced(1.0);
                assert!(
                    (azimuth - later.azimuth_deg).abs() < 0.05,
                    "{azimuth} vs {}",
                    later.azimuth_deg
                );
                assert!((elevation - later.elevation_deg).abs() < 0.05);
                assert!((range - later.range_km).abs() < 0.3);
                let (azimuth, elevation, range) = now.advanced(0.0);
                assert!(
                    (azimuth - now.azimuth_deg).abs() < 1e-9
                        && (elevation - now.elevation_deg).abs() < 1e-9
                );
                assert!((range - now.range_km).abs() < 1e-9);
                checked += 1;
            }
        }
        assert!(checked > 0, "some observer sees the pass");
    }

    // SPEC-SATMODEL-002: the orbital frame is orthonormal, radial points from
    // Earth's centre through the craft, and along-track follows its flight.
    #[test]
    fn the_attitude_is_the_orbital_frame() {
        let catalog = TleCatalog::parse(ISS_TLE).expect("valid test TLE");
        let at = NaiveDate::from_ymd_opt(2020, 7, 12)
            .unwrap()
            .and_hms_opt(21, 15, 0)
            .unwrap();
        let angle = |a: [f64; 3], b: [f64; 3]| {
            let dot: f64 = (0..3).map(|axis| a[axis] * b[axis]).sum();
            let size = |v: [f64; 3]| v[0].hypot(v[1]).hypot(v[2]);
            (dot / (size(a) * size(b)))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        };
        let mut checked = 0;
        for latitude_deg in (-60..=60).step_by(20) {
            for longitude_deg in (-180..=180).step_by(20) {
                let tracker = catalog
                    .clone()
                    .tracker(ObserverLocation {
                        latitude_deg: f64::from(latitude_deg),
                        longitude_deg: f64::from(longitude_deg),
                        altitude_m: 0.0,
                    })
                    .expect("valid observer");
                let Some(look) = tracker.look_angles_at(at).into_iter().next() else {
                    continue;
                };
                let attitude = look
                    .attitude
                    .expect("a propagated velocity gives an attitude");
                let (radial, along) = (attitude.radial, attitude.along_track);
                assert!((angle(radial, along) - 90.0).abs() < 1e-6);
                assert!((radial[0].hypot(radial[1]).hypot(radial[2]) - 1.0).abs() < 1e-9);
                // From Earth's centre: the observer sits about one radius below.
                let from_centre = [look.enu_km[0], look.enu_km[1], look.enu_km[2] + 6_371.0];
                assert!(
                    angle(radial, from_centre) < 0.6,
                    "{}",
                    angle(radial, from_centre)
                );
                assert!(
                    angle(along, look.enu_velocity_km_s) < 5.0,
                    "{}",
                    angle(along, look.enu_velocity_km_s)
                );
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    // SPEC-CONST-007: the 30-minute forecast sweeps the whole catalog at each
    // offset and keeps the smallest serviceable count. Look angles carry the
    // orbital speed the callout shows.
    #[test]
    fn the_forecast_takes_the_minimum_over_the_next_half_hour() {
        let catalog = TleCatalog::parse(ISS_TLE).expect("valid test TLE");
        let at = NaiveDate::from_ymd_opt(2020, 7, 12)
            .unwrap()
            .and_hms_opt(21, 15, 0)
            .unwrap();
        let tracker = catalog
            .tracker(ObserverLocation {
                latitude_deg: 0.0,
                longitude_deg: 0.0,
                altitude_m: 0.0,
            })
            .expect("valid observer");
        assert_eq!(tracker.constellation_size(), 1);
        // One satellite cannot stay above 25° for half an hour from any site.
        assert_eq!(tracker.forecast_min_serviceable(at), 0);

        let frame = ObserverFrame::new(tracker.observer(), &at);
        let look = frame
            .look(&tracker.satellites[0], &at)
            .expect("propagates at any elevation");
        assert!((7.0..8.0).contains(&look.speed_km_s), "{}", look.speed_km_s);
    }

    // SPEC-CONST-002: the selected AFSPC SGP4 mode matches the published
    // Vanguard TEME state vector at epoch.
    #[test]
    fn sgp4_matches_the_epoch_reference_state() {
        let elements = Elements::from_tle(
            Some("VANGUARD 1".into()),
            VANGUARD_LINE_1.as_bytes(),
            VANGUARD_LINE_2.as_bytes(),
        )
        .expect("valid reference TLE");
        let constants = Constants::from_elements_afspc_compatibility_mode(&elements)
            .expect("valid reference orbit");
        let prediction = constants
            .propagate_afspc_compatibility_mode(MinutesSinceEpoch(0.0))
            .expect("epoch prediction");
        let expected = [7022.46529266, -1400.08296755, 0.03995155];
        for (actual, expected) in prediction.position.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    // SPEC-CONST-003: prefer the highest clear candidate, then fall back to the
    // highest above-floor satellite when obstruction rules exclude every one.
    #[test]
    fn serving_candidate_obeys_service_floor_and_clear_fallback() {
        let satellites = [
            satellite("below-floor", 24.9),
            satellite("higher-obstructed", 50.0),
            satellite("lower-clear", 35.0),
        ];
        assert_eq!(
            serving_candidate(&satellites, |satellite| satellite.name == "lower-clear")
                .map(|satellite| satellite.name.as_str()),
            Some("lower-clear")
        );
        assert_eq!(
            serving_candidate(&satellites, |_| false).map(|satellite| satellite.name.as_str()),
            Some("higher-obstructed")
        );
        assert!(serving_candidate(&satellites[..1], |_| true).is_none());
    }

    // SPEC-CONST-004: invalid or policy-disabled observer coordinates do not
    // start a device lookup; the model simply refuses to build a tracker.
    #[test]
    fn invalid_observer_coordinates_are_rejected_without_device_io() {
        let catalog = TleCatalog::parse(ISS_TLE).expect("valid test TLE");
        assert_eq!(
            catalog
                .tracker(ObserverLocation {
                    latitude_deg: 91.0,
                    longitude_deg: 0.0,
                    altitude_m: 0.0,
                })
                .err(),
            Some(ConstellationError::InvalidObserver)
        );
    }

    fn satellite(name: &str, elevation_deg: f64) -> SatelliteLook {
        SatelliteLook {
            name: name.into(),
            azimuth_deg: 0.0,
            elevation_deg,
            range_km: 1_000.0,
            altitude_km: 550.0,
            speed_km_s: 7.6,
            enu_km: [0.0; 3],
            enu_velocity_km_s: [0.0; 3],
            attitude: None,
        }
    }
}
