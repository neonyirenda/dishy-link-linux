//! Every vertex buffer the sky view draws, built as plain floats.
//!
//! Pure data in, `Vec<f32>` out — no GL handles, no canvas, no frame loop. The
//! renderer owns GPU objects; this owns shape, and shape can be tested without
//! a display. Layouts are documented per builder and are what the attribute
//! pointers in `sky_render.rs` are cut to.

use crate::frame::grid_cell_to_enu;
use crate::survey::{CELL_UNMAPPED, Survey};

/// Light shared by every baked surface.
pub const LIGHT: [f64; 3] = [
    -0.309_426_373_877_638,
    0.928_279_121_632_914,
    -0.206_284_249_251_758_67,
];

/// The survey shell floats clear of the ground rather than sitting on it.
const DOME_LIFT: f64 = 0.18;
const ENVELOPE_SPOKES: usize = 72;
const ENVELOPE_REACH: isize = 3;

fn spoke_at(east: f64, north: f64) -> usize {
    let azimuth = (east.atan2(north) + std::f64::consts::TAU) % std::f64::consts::TAU;
    ((azimuth / std::f64::consts::TAU) * ENVELOPE_SPOKES as f64) as usize % ENVELOPE_SPOKES
}

/// How far out the dish's readings reach, one figure per compass direction, in
/// grid radius. Null when nothing has ever been observed.
pub fn observed_envelope(survey: &Survey) -> Option<Vec<f64>> {
    let grid_size = survey.grid_size;
    let mut deepest = vec![-1.0_f64; ENVELOPE_SPOKES];
    let mut any_reading = false;
    for row in 0..grid_size {
        for col in 0..grid_size {
            if survey.kinds[row * grid_size + col] == CELL_UNMAPPED {
                continue;
            }
            let (east, north) = grid_cell_to_enu(
                row,
                col,
                grid_size,
                survey.frame,
                survey.boresight_azimuth_deg,
            );
            let radial = (east * east + north * north).sqrt();
            if radial > 1.0 {
                continue;
            }
            any_reading = true;
            let spoke = spoke_at(east, north);
            if radial > deepest[spoke] {
                deepest[spoke] = radial;
            }
        }
    }
    any_reading.then(|| {
        let wrap = |index: isize| index.rem_euclid(ENVELOPE_SPOKES as isize) as usize;
        let mut reach = vec![0.0_f64; ENVELOPE_SPOKES];
        for spoke in 0..ENVELOPE_SPOKES {
            let mut furthest = -1.0_f64;
            for delta in -ENVELOPE_REACH..=ENVELOPE_REACH {
                furthest = furthest.max(deepest[wrap(spoke as isize + delta)]);
            }
            reach[spoke] = furthest;
        }
        // A sector wider than the reach carries a figure across from the nearest
        // direction either side rather than leaving a hole.
        for spoke in 0..ENVELOPE_SPOKES {
            if reach[spoke] >= 0.0 {
                continue;
            }
            let mut before = spoke as isize;
            let mut after = spoke as isize;
            while reach[wrap(before)] < 0.0 {
                before -= 1;
            }
            while reach[wrap(after)] < 0.0 {
                after += 1;
            }
            reach[spoke] = reach[wrap(before)].max(reach[wrap(after)]);
        }
        (0..ENVELOPE_SPOKES)
            .map(|spoke| {
                (reach[wrap(spoke as isize - 1)] + reach[spoke] + reach[wrap(spoke as isize + 1)])
                    / 3.0
            })
            .collect()
    })
}

/// The dome sits at radius 1. The constellation shell is pushed out from it,
/// as in upstream `skyScene.ts`.
const SATELLITE_SHELL: f64 = 2.0;

/// A satellite's display position: its true bearing, on a cosmetic radius
/// that grows a little with slant range so near passes read as nearer.
pub fn satellite_position(azimuth_deg: f64, elevation_deg: f64, range_km: f64) -> [f32; 3] {
    let radius = SATELLITE_SHELL * (1.12 + (range_km.max(550.0) / 550.0 - 1.0) * 0.26);
    let (sin_az, cos_az) = azimuth_deg.to_radians().sin_cos();
    let (sin_el, cos_el) = elevation_deg.to_radians().sin_cos();
    [
        (cos_el * sin_az * radius) as f32,
        (sin_el * radius) as f32,
        (-cos_el * cos_az * radius) as f32,
    ]
}

/// How many points span a trail and how far apart in time they sit (spec 0182).
pub const TRAIL_POINTS: usize = 12;
pub const TRAIL_INTERVAL_S: f64 = 0.55;
/// The furthest a sample is carried forward: one and a half 1 Hz samples.
pub const MAX_ADVANCE_S: f64 = 1.5;
const TRAIL_HEAD_WIDTH: f64 = 0.004;
const TRAIL_TAIL_FLARE: f64 = 0.8;
const TRAIL_HEAD_GLOW: f64 = 0.3;
/// Floats per ribbon vertex: `xyz + across + glow`.
pub const TRAIL_FLOATS: usize = 5;

/// Where a satellite is drawn `since_sample_s` after its sample.
pub fn drawn_position(look: &crate::SatelliteLook, since_sample_s: f64) -> [f32; 3] {
    let (azimuth, elevation, range) = look.advanced(since_sample_s.clamp(0.0, MAX_ADVANCE_S));
    satellite_position(azimuth, elevation, range)
}

/// A trail, oldest point first, ending at the drawn position. Derived from the
/// sample each frame, never accumulated, so a stalled loop leaves nothing stale.
pub fn trail_points(look: &crate::SatelliteLook, since_sample_s: f64) -> Vec<[f32; 3]> {
    let head = drawn_position(look, since_sample_s);
    if !look.has_velocity() {
        return vec![head];
    }
    let now = since_sample_s.clamp(0.0, MAX_ADVANCE_S);
    let mut points: Vec<[f32; 3]> = (1..TRAIL_POINTS)
        .rev()
        .map(|step| {
            let (azimuth, elevation, range) = look.advanced(now - step as f64 * TRAIL_INTERVAL_S);
            satellite_position(azimuth, elevation, range)
        })
        .collect();
    points.push(head);
    points
}

/// The trails as one camera-facing ribbon of triangles, `TRAIL_FLOATS` per
/// vertex.
pub fn build_trail_ribbon(trails: &[Vec<[f32; 3]>], eye: [f32; 3]) -> Vec<f32> {
    let eye = eye.map(f64::from);
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let length = |v: [f64; 3]| v[0].hypot(v[1]).hypot(v[2]);
    let unit = |v: [f64; 3]| {
        let size = length(v);
        if size > 0.0 {
            v.map(|component| component / size)
        } else {
            v
        }
    };
    // Across the path and at right angles to the line of sight.
    let side = |point: [f64; 3], direction: [f64; 3]| {
        let to_eye = sub(eye, point);
        unit([
            direction[1] * to_eye[2] - direction[2] * to_eye[1],
            direction[2] * to_eye[0] - direction[0] * to_eye[2],
            direction[0] * to_eye[1] - direction[1] * to_eye[0],
        ])
    };
    let mut out = Vec::new();
    for trail in trails {
        let count = trail.len();
        if count < 2 {
            continue;
        }
        for index in 1..count {
            let a = trail[index - 1].map(f64::from);
            let b = trail[index].map(f64::from);
            let direction = unit(sub(b, a));
            // 0 at the oldest tail, 1 at the head.
            let section = |point: [f64; 3], fraction: f64| {
                let width = TRAIL_HEAD_WIDTH
                    * length(sub(eye, point))
                    * (1.0 + (1.0 - fraction) * TRAIL_TAIL_FLARE);
                (
                    point,
                    side(point, direction),
                    width,
                    fraction * fraction * TRAIL_HEAD_GLOW,
                )
            };
            let start = section(a, (index - 1) as f64 / (count - 1) as f64);
            let end = section(b, index as f64 / (count - 1) as f64);
            let mut push = |(point, side, width, glow): ([f64; 3], [f64; 3], f64, f64),
                            across: f64| {
                out.extend_from_slice(&[
                    (point[0] + side[0] * width * across) as f32,
                    (point[1] + side[1] * width * across) as f32,
                    (point[2] + side[2] * width * across) as f32,
                    across as f32,
                    glow as f32,
                ]);
            };
            // Two triangles bridging the cross-sections.
            push(start, -1.0);
            push(end, -1.0);
            push(end, 1.0);
            push(start, -1.0);
            push(end, 1.0);
            push(start, 1.0);
        }
    }
    out
}

/// The craft's model scale: Electron's `0.032 × shell / 22` (spec 0184).
pub const SATELLITE_MODEL_SCALE: f64 = 0.032 * SATELLITE_SHELL / 22.0;

/// The craft's `[right, up, forward]` in scene axes (x east, y up, z south):
/// nadir-pointing with its nose along track. Without an attitude, up points
/// away from the centre and forward is level, so the basis is never broken.
pub fn satellite_basis(look: &crate::SatelliteLook, position: [f32; 3]) -> [[f32; 3]; 3] {
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let unit = |v: [f64; 3]| {
        let size = v[0].hypot(v[1]).hypot(v[2]);
        if size > 0.0 {
            v.map(|value| value / size)
        } else {
            v
        }
    };
    // ENU to scene axes is a relabelling: east, up, south.
    let scene = |enu: [f64; 3]| [enu[0], enu[2], -enu[1]];
    let (up, forward) = match look.attitude {
        Some(attitude) => (scene(attitude.radial), unit(scene(attitude.along_track))),
        None => {
            let up = unit(position.map(f64::from));
            let reference = if up[1].abs() > 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            (up, unit(cross(reference, up)))
        }
    };
    let right = unit(cross(forward, up));
    [right, up, forward].map(|axis| axis.map(|value| value as f32))
}

/// Dome points, `xyz + kind` per vertex, drawn as GL points.
pub fn build_dome_points(survey: &Survey, trim_unmapped: bool) -> Vec<f32> {
    let grid_size = survey.grid_size;
    let max_theta = survey.max_theta_deg.to_radians();
    let envelope = trim_unmapped.then(|| observed_envelope(survey)).flatten();
    let mut out = Vec::with_capacity(grid_size * grid_size * 4);
    for row in 0..grid_size {
        for col in 0..grid_size {
            let (east, north) = grid_cell_to_enu(
                row,
                col,
                grid_size,
                survey.frame,
                survey.boresight_azimuth_deg,
            );
            let radial = (east * east + north * north).sqrt();
            if radial > 1.0 {
                continue;
            }
            let kind = survey.kinds[row * grid_size + col];
            if let Some(envelope) = &envelope
                && kind == CELL_UNMAPPED
                && radial > envelope[spoke_at(east, north)]
            {
                continue;
            }
            let zenith = radial * max_theta;
            let spread = if radial == 0.0 {
                0.0
            } else {
                zenith.sin() / radial
            };
            out.extend_from_slice(&[
                (east * spread) as f32,
                (zenith.cos() + DOME_LIFT) as f32,
                (-north * spread) as f32,
                f32::from(kind),
            ]);
        }
    }
    out
}

/// Stars, `xyz + size` per point, on a far shell behind the dome.
pub fn build_stars() -> Vec<f32> {
    let mut seed = 42_u64;
    let mut rand = move || {
        seed = (seed * 16_807) % 2_147_483_647;
        seed as f64 / 2_147_483_647.0
    };
    let mut out = Vec::with_capacity(700 * 4);
    for _ in 0..700 {
        let azimuth = rand() * std::f64::consts::TAU;
        let elevation = (rand() * 0.98 + 0.02).asin();
        let radius = 40.0;
        out.extend_from_slice(&[
            (radius * elevation.cos() * azimuth.sin()) as f32,
            (radius * elevation.sin()) as f32,
            (-radius * elevation.cos() * azimuth.cos()) as f32,
            (0.6 + rand() * 1.8) as f32,
        ]);
    }
    out
}

/// A faceted ground, flat-shaded by baking the light into per-vertex colour.
pub fn build_terrain() -> Vec<f32> {
    const N: usize = 46;
    const EXTENT: f64 = 9.0;
    let mut seed = 7_u64;
    let mut rand = move || {
        seed = (seed * 16_807) % 2_147_483_647;
        seed as f64 / 2_147_483_647.0
    };
    let mut heights = Vec::with_capacity((N + 1) * (N + 1));
    for i in 0..=N {
        for j in 0..=N {
            let x = (i as f64 / N as f64 - 0.5) * 2.0 * EXTENT;
            let z = (j as f64 / N as f64 - 0.5) * 2.0 * EXTENT;
            let distance = (x * x + z * z).sqrt();
            let amplitude = ((distance - 0.9) / 2.5).clamp(0.0, 1.0) * (0.12 + 0.35 * rand());
            heights.push(amplitude * (rand() - 0.35));
        }
    }
    let at = |i: usize, j: usize| -> [f64; 3] {
        [
            (i as f64 / N as f64 - 0.5) * 2.0 * EXTENT,
            heights[i * (N + 1) + j],
            (j as f64 / N as f64 - 0.5) * 2.0 * EXTENT,
        ]
    };
    let mut out = Vec::with_capacity(N * N * 6 * 6);
    for i in 0..N {
        for j in 0..N {
            let quad = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            for triangle in [[0, 1, 2], [0, 2, 3]] {
                let [a, b, c] = triangle.map(|index| quad[index]);
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let normal = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let length =
                    (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                        .sqrt()
                        .max(f64::EPSILON);
                let light = ((normal[0] / length) * LIGHT[0]
                    + (normal[1] / length) * LIGHT[1]
                    + (normal[2] / length) * LIGHT[2])
                    .max(0.0);
                let mid = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                let fade = (mid[0].hypot(mid[2]) / EXTENT).min(1.0);
                let base = 0.32 + light * 0.42;
                let color = [
                    base * (0.72 - 0.28 * fade),
                    base * (0.78 - 0.26 * fade),
                    base * (0.88 - 0.18 * fade),
                ];
                for point in [a, b, c] {
                    out.extend_from_slice(&[point[0] as f32, point[1] as f32, point[2] as f32]);
                    out.extend_from_slice(&[color[0] as f32, color[1] as f32, color[2] as f32]);
                }
            }
        }
    }
    out
}

const CARDINALS: [(&str, f64); 4] = [
    ("N", 0.0),
    ("E", std::f64::consts::FRAC_PI_2),
    ("S", std::f64::consts::PI),
    ("W", 3.0 * std::f64::consts::FRAC_PI_2),
];

/// Stroke paths for the cardinal letters, in a unit box, as line pairs.
type GlyphStroke = ([f64; 2], [f64; 2]);
type Glyph = (&'static str, [GlyphStroke; 5], usize);
const GLYPHS: [Glyph; 4] = [
    (
        "N",
        [
            ([0.0, 0.0], [0.0, 1.0]),
            ([0.0, 1.0], [1.0, 0.0]),
            ([1.0, 0.0], [1.0, 1.0]),
            ([0.0, 0.0], [0.0, 0.0]),
            ([0.0, 0.0], [0.0, 0.0]),
        ],
        3,
    ),
    (
        "E",
        [
            ([1.0, 1.0], [0.0, 1.0]),
            ([0.0, 1.0], [0.0, 0.0]),
            ([0.0, 0.0], [1.0, 0.0]),
            ([0.0, 0.5], [0.62, 0.5]),
            ([0.0, 0.0], [0.0, 0.0]),
        ],
        4,
    ),
    (
        "S",
        [
            ([1.0, 1.0], [0.0, 1.0]),
            ([0.0, 1.0], [0.0, 0.5]),
            ([0.0, 0.5], [1.0, 0.5]),
            ([1.0, 0.5], [1.0, 0.0]),
            ([1.0, 0.0], [0.0, 0.0]),
        ],
        5,
    ),
    (
        "W",
        [
            ([0.0, 1.0], [0.25, 0.0]),
            ([0.25, 0.0], [0.5, 0.7]),
            ([0.5, 0.7], [0.75, 0.0]),
            ([0.75, 0.0], [1.0, 1.0]),
            ([0.0, 0.0], [0.0, 0.0]),
        ],
        4,
    ),
];

/// The dashed horizon ring and radial ticks, as triangles with baked colour.
pub fn build_compass() -> Vec<f32> {
    const LIFT: f64 = 0.06;
    const TICKS: usize = 48;
    const INNER: f64 = 0.99;
    const OUTER: f64 = 1.05;
    const HALF: f64 = 0.006 / 2.0;
    const TINT: f64 = 0.45;
    let mut out = Vec::with_capacity(TICKS * 18);
    let mut push = |x: f64, z: f64| {
        out.extend_from_slice(&[
            x as f32,
            LIFT as f32,
            z as f32,
            (TINT * 0.86) as f32,
            (TINT * 0.9) as f32,
            TINT as f32,
        ]);
    };
    for index in 0..TICKS {
        let angle = index as f64 / TICKS as f64 * std::f64::consts::TAU;
        let dx = angle.sin();
        let dz = -angle.cos();
        let tx = angle.cos();
        let tz = angle.sin();
        let inner = (dx * INNER, dz * INNER);
        let outer = (dx * OUTER, dz * OUTER);
        let a1 = (inner.0 - tx * HALF, inner.1 - tz * HALF);
        let a2 = (inner.0 + tx * HALF, inner.1 + tz * HALF);
        let b1 = (outer.0 - tx * HALF, outer.1 - tz * HALF);
        let b2 = (outer.0 + tx * HALF, outer.1 + tz * HALF);
        push(a1.0, a1.1);
        push(a2.0, a2.1);
        push(b2.0, b2.1);
        push(a1.0, a1.1);
        push(b2.0, b2.1);
        push(b1.0, b1.1);
    }
    out
}

/// The N/E/S/W letters as filled ribbons, laid flat and reading outward.
pub fn build_compass_labels() -> Vec<f32> {
    const LIFT: f64 = 0.06;
    const SIZE: f64 = 0.13;
    const WIDTH: f64 = 0.015;
    let mut out = Vec::new();
    let mut push = |x: f64, z: f64, tint: f64| {
        out.extend_from_slice(&[
            x as f32,
            LIFT as f32,
            z as f32,
            (tint * 0.86) as f32,
            (tint * 0.9) as f32,
            tint as f32,
        ]);
    };
    for (label, azimuth) in CARDINALS {
        let centre_x = azimuth.sin() * 1.16;
        let centre_z = -azimuth.cos() * 1.16;
        let right_x = azimuth.cos();
        let right_z = azimuth.sin();
        let up_x = azimuth.sin();
        let up_z = -azimuth.cos();
        let tint = if label == "N" { 0.8 } else { 0.46 };
        let world = |gx: f64, gy: f64| {
            let dx = (gx - 0.5) * SIZE;
            let dy = (gy - 0.5) * SIZE;
            (
                centre_x + right_x * dx + up_x * dy,
                centre_z + right_z * dx + up_z * dy,
            )
        };
        let glyph = GLYPHS
            .iter()
            .find(|(name, _, _)| *name == label)
            .expect("cardinal glyph");
        for (from, to) in glyph.1.iter().take(glyph.2) {
            let a = world(from[0], from[1]);
            let b = world(to[0], to[1]);
            let mut dx = b.0 - a.0;
            let mut dz = b.1 - a.1;
            let length = dx.hypot(dz).max(f64::EPSILON);
            dx /= length;
            dz /= length;
            let half = WIDTH / 2.0;
            let px = -dz * half;
            let pz = dx * half;
            let ex = dx * half;
            let ez = dz * half;
            let corners = [
                (a.0 - ex - px, a.1 - ez - pz),
                (a.0 - ex + px, a.1 - ez + pz),
                (b.0 + ex + px, b.1 + ez + pz),
                (a.0 - ex - px, a.1 - ez - pz),
                (b.0 + ex + px, b.1 + ez + pz),
                (b.0 + ex - px, b.1 + ez - pz),
            ];
            for (x, z) in corners {
                push(x, z, tint);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct DishMesh {
    /// `xyz + rgb` per vertex.
    pub data: Vec<f32>,
    /// Where a beam would leave the panel, in world space.
    pub origin: [f32; 3],
    pub radius: f32,
}

/// A procedural terminal: a rectangular panel on a small mast, aimed along the
/// live boresight. Per-kit meshes are a fidelity follow-up (spec 0100).
pub fn build_dish(survey: &Survey, scale: f64) -> DishMesh {
    let model = crate::dish_models::mesh_for_model(&survey.dish_model);
    // Stored units to scene units: the panel's long axis spans 0.5 × scale.
    let mm = 0.5 * scale / model.long_axis_mm;
    let azimuth = survey.boresight_azimuth_deg.to_radians();
    let elevation = survey.boresight_elevation_deg.to_radians();
    let (sa, ca) = azimuth.sin_cos();
    let (se, ce) = elevation.sin_cos();
    // w is the boresight, v along the face, u across. World is x east, y up,
    // z south, so the frame is right-handed and outward faces stay lit.
    let w = [ce * sa, se, -ce * ca];
    let v = [-se * sa, ce, se * ca];
    let u = [-ca, 0.0, -sa];
    let place = |a: f64, b: f64, c: f64| -> [f64; 3] {
        [
            a * u[0] + b * v[0] + c * w[0],
            a * u[1] + b * v[1] + c * w[1],
            a * u[2] + b * v[2] + c * w[2],
        ]
    };
    // A hinged mount: the head tilts to the boresight, the foot stays flat and
    // only turns in azimuth, and both meet at the pivot.
    let pivot = model.joint.map_or([0.0; 3], |joint| {
        joint.pivot.map(|value| value / model.scale * mm)
    });
    let hinge = place(pivot[0], pivot[1], pivot[2]);
    let plant = |a: f64, b: f64, c: f64| -> [f64; 3] {
        let (p, q, r) = (a - pivot[0], b - pivot[1], c - pivot[2]);
        [
            p * u[0] - q * sa + hinge[0],
            r + hinge[1],
            p * u[2] + q * ca + hinge[2],
        ]
    };
    let foot_from = model.joint.map(|joint| joint.base_vertex);

    let points: Vec<[f64; 3]> = model
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .map(|(index, xyz)| {
            let [a, b, c] = xyz.map(|value| f64::from(value) / model.scale * mm);
            if foot_from.is_some_and(|first| index >= first) {
                plant(a, b, c)
            } else {
                place(a, b, c)
            }
        })
        .collect();

    let mut data = Vec::with_capacity(model.indices.len() * 6);
    for &(first, count, tint) in &model.parts {
        for triangle in first..first + count {
            let corner = |k: usize| points[usize::from(model.indices[triangle * 3 + k])];
            let (a, b, c) = (corner(0), corner(1), corner(2));
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let normal = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let length = normal[0].hypot(normal[1]).hypot(normal[2]);
            let length = if length > 0.0 { length } else { 1.0 };
            let lambert = ((normal[0] * LIGHT[0] + normal[1] * LIGHT[1] + normal[2] * LIGHT[2])
                / length)
                .max(0.0);
            let shade = (0.3 + 0.7 * lambert) * tint;
            for point in [a, b, c] {
                data.extend_from_slice(&[
                    point[0] as f32,
                    point[1] as f32,
                    point[2] as f32,
                    (shade * 0.94) as f32,
                    (shade * 0.96) as f32,
                    shade as f32,
                ]);
            }
        }
    }
    // Stand the model on the ground.
    let low = data
        .iter()
        .skip(1)
        .step_by(6)
        .fold(f32::INFINITY, |low, y| low.min(*y));
    let low = if low.is_finite() { low } else { 0.0 };
    for y in data.iter_mut().skip(1).step_by(6) {
        *y -= low;
    }
    // Only a body with an inside to fall into holds the camera off.
    let radius = if model.keep_camera_outside {
        data.as_chunks::<6>()
            .0
            .iter()
            .map(|vertex| vertex[0].hypot(vertex[1]).hypot(vertex[2]))
            .fold(0.0, f32::max)
    } else {
        0.0
    };
    let [bx, by, bz] = model.beam_origin_mm.unwrap_or([0.0, 0.0, 14.0]);
    let exit = if foot_from == Some(0) {
        plant(bx * mm, by * mm, bz * mm)
    } else {
        place(bx * mm, by * mm, bz * mm)
    };
    DishMesh {
        data,
        origin: [exit[0] as f32, exit[1] as f32 - low, exit[2] as f32],
        radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::ObstructionFrame;
    use crate::survey::DomePointKind;

    fn moving_look() -> crate::SatelliteLook {
        crate::SatelliteLook {
            name: "STARLINK-1".into(),
            azimuth_deg: 90.0,
            elevation_deg: 45.0,
            range_km: 700.0,
            altitude_km: 550.0,
            speed_km_s: 7.6,
            enu_km: [494.974_746_830_583_5, 0.0, 494.974_746_830_583_5],
            enu_velocity_km_s: [0.0, 7.0, 0.0],
            attitude: None,
        }
    }

    // SPEC-TRAIL-002: the drawn position is the sample advanced, capped at
    // 1.5 s; a look without a velocity stays where it was sampled.
    #[test]
    fn satellites_advance_with_a_cap() {
        let look = moving_look();
        let (azimuth, elevation, range) = look.advanced(1.0);
        assert_eq!(
            drawn_position(&look, 1.0),
            satellite_position(azimuth, elevation, range)
        );
        assert_eq!(
            drawn_position(&look, 9.0),
            drawn_position(&look, MAX_ADVANCE_S)
        );
        assert_eq!(drawn_position(&look, -1.0), drawn_position(&look, 0.0));
        let still = crate::SatelliteLook {
            enu_velocity_km_s: [0.0; 3],
            ..look
        };
        assert_eq!(
            drawn_position(&still, 1.0),
            satellite_position(90.0, 45.0, 700.0)
        );
    }

    // SPEC-SATMODEL-003: the basis maps the orbital frame into scene axes and
    // stays orthonormal without one.
    #[test]
    fn the_craft_basis_follows_its_orbit() {
        let unit = |v: [f32; 3]| (v[0].hypot(v[1]).hypot(v[2]) - 1.0).abs() < 1e-5;
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let flying_north = crate::SatelliteLook {
            attitude: Some(crate::SatelliteAttitude {
                radial: [0.0, 0.0, 1.0],
                along_track: [0.0, 1.0, 0.0],
            }),
            ..moving_look()
        };
        let [right, up, forward] = satellite_basis(&flying_north, [0.0, 2.0, 0.0]);
        assert_eq!(up, [0.0, 1.0, 0.0]);
        assert_eq!(forward, [0.0, 0.0, -1.0], "north is -z");
        assert_eq!(right, [1.0, 0.0, 0.0], "right-handed: east");

        let position = [1.0, 1.0, -0.5];
        let [right, up, forward] = satellite_basis(&moving_look(), position);
        for axis in [right, up, forward] {
            assert!(unit(axis), "{axis:?}");
        }
        assert!(
            dot(right, up).abs() < 1e-6
                && dot(up, forward).abs() < 1e-6
                && dot(right, forward).abs() < 1e-6
        );
        let length = 1.5_f32;
        assert!(
            (0..3).all(|axis| (up[axis] - position[axis] / length).abs() < 1e-6),
            "up is outward"
        );
        // Straight overhead still gives a level forward.
        let [_, up, forward] = satellite_basis(&moving_look(), [0.0, 2.0, 0.0]);
        assert_eq!(up, [0.0, 1.0, 0.0]);
        assert!(unit(forward) && forward[1].abs() < 1e-6);
    }

    // SPEC-TRAIL-003: twelve points 550 ms apart, oldest first, ending at the
    // drawn head; no velocity, no trail.
    #[test]
    fn trails_step_back_along_the_velocity() {
        let look = moving_look();
        let trail = trail_points(&look, 0.4);
        assert_eq!(trail.len(), TRAIL_POINTS);
        assert_eq!(*trail.last().unwrap(), drawn_position(&look, 0.4));
        let (azimuth, elevation, range) = look.advanced(0.4 - 11.0 * TRAIL_INTERVAL_S);
        assert_eq!(trail[0], satellite_position(azimuth, elevation, range));
        let still = crate::SatelliteLook {
            enu_velocity_km_s: [0.0; 3],
            ..look
        };
        assert_eq!(trail_points(&still, 0.4).len(), 1);
    }

    // SPEC-TRAIL-004: two triangles per segment, the half-width scaling with
    // distance and flaring to the tail, glow squared toward the head, and the
    // ribbon turned to face the eye.
    #[test]
    fn the_ribbon_faces_the_eye_and_fades_to_the_tail() {
        // A straight trail along +x at z = 0, seen from far up +z.
        let trail = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let eye = [1.0, 0.0, 10.0];
        let ribbon = build_trail_ribbon(&[trail, vec![[5.0, 5.0, 5.0]]], eye);
        assert_eq!(
            ribbon.len(),
            2 * 6 * TRAIL_FLOATS,
            "single-point trails draw nothing"
        );
        let vertex = |index: usize| &ribbon[index * TRAIL_FLOATS..(index + 1) * TRAIL_FLOATS];
        // First vertex: the tail (fraction 0), across −1, no glow.
        let tail = vertex(0);
        assert_eq!((tail[3], tail[4]), (-1.0, 0.0));
        // Facing the eye: the offset is across the trail in y, never in z.
        assert!(tail[2].abs() < 1e-6, "{tail:?}");
        let distance = (1.0_f32 + 100.0).sqrt();
        let expected = 0.004 * distance * 1.8;
        assert!(
            (tail[1].abs() - expected).abs() < 1e-5,
            "{} vs {expected}",
            tail[1]
        );
        // The head of the last segment: fraction 1, full glow 0.3, no flare.
        let head = vertex(6 + 1);
        assert!((head[4] - 0.3).abs() < 1e-6);
        let head_distance = (1.0_f32 + 100.0).sqrt();
        assert!((head[1].abs() - 0.004 * head_distance).abs() < 1e-5);
        // Midpoint glow is 0.3 × 0.5².
        assert!((vertex(1)[4] - 0.075).abs() < 1e-6);
    }

    fn survey() -> Survey {
        let mut kinds = vec![DomePointKind::Unmapped.code(); 11 * 11];
        for row in 3..8 {
            for col in 3..8 {
                kinds[row * 11 + col] = DomePointKind::Clear.code();
            }
        }
        kinds[5 * 11 + 5] = DomePointKind::Obstructed.code();
        Survey {
            grid_size: 11,
            max_theta_deg: 80.0,
            kinds,
            boresight_azimuth_deg: 0.0,
            boresight_elevation_deg: 70.0,
            frame: ObstructionFrame::Earth,
            dish_model: "rev4Standard".into(),
        }
    }

    // SPEC-SKY-003: the envelope only measures observed cells, and trimming
    // never removes a reading.
    #[test]
    fn trimming_only_drops_unmapped_sky() {
        let survey = survey();
        let envelope = observed_envelope(&survey).expect("observed cells");
        assert_eq!(envelope.len(), ENVELOPE_SPOKES);

        let untrimmed = build_dome_points(&survey, false);
        let trimmed = build_dome_points(&survey, true);
        assert!(trimmed.len() < untrimmed.len());
        let (trimmed_points, _) = trimmed.as_chunks::<4>();
        let (untrimmed_points, _) = untrimmed.as_chunks::<4>();
        let observed_in_trimmed = trimmed_points
            .iter()
            .filter(|point| point[3] != f32::from(CELL_UNMAPPED))
            .count();
        let observed_untrimmed = untrimmed_points
            .iter()
            .filter(|point| point[3] != f32::from(CELL_UNMAPPED))
            .count();
        assert_eq!(observed_in_trimmed, observed_untrimmed);
    }

    #[test]
    fn an_unobserved_survey_has_no_envelope_and_keeps_every_point() {
        let mut survey = survey();
        survey.kinds.fill(DomePointKind::Unmapped.code());
        assert!(observed_envelope(&survey).is_none());
        assert_eq!(
            build_dome_points(&survey, true).len(),
            build_dome_points(&survey, false).len()
        );
    }

    // SPEC-SKY-005: the generated buffers are complete and finite.
    // SPEC-CONST-009: satellites keep the dome's north-zero, clockwise bearing
    // and sit outside it.
    #[test]
    fn satellites_sit_on_their_bearing_outside_the_dome() {
        let north = satellite_position(0.0, 0.0, 550.0);
        assert!(north[2] < -2.0 && north[0].abs() < 1e-5 && north[1].abs() < 1e-5);
        let east = satellite_position(90.0, 0.0, 550.0);
        assert!(east[0] > 2.0 && east[2].abs() < 1e-5);
        let zenith = satellite_position(0.0, 90.0, 550.0);
        assert!((zenith[1] - 2.24).abs() < 1e-5);
        let far = satellite_position(0.0, 90.0, 1_100.0);
        assert!(far[1] > zenith[1]);
    }

    #[test]
    fn generated_geometry_is_finite_and_the_expected_shape() {
        assert_eq!(build_stars().len(), 700 * 4);
        assert!(build_terrain().len().is_multiple_of(18));
        assert!(build_compass().len().is_multiple_of(18));
        assert!(!build_compass_labels().is_empty());
        assert!(
            build_dome_points(&survey(), false)
                .iter()
                .all(|v| v.is_finite())
        );

        let dish = build_dish(&survey(), 1.0);
        assert!(dish.data.len().is_multiple_of(18));
        assert!(dish.origin.iter().all(|v| v.is_finite()));
        // Only a body the camera could fall into reports a radius (spec 0183).
        assert_eq!(dish.radius, 0.0);
        let ground = dish
            .data
            .iter()
            .skip(1)
            .step_by(6)
            .fold(f32::INFINITY, |low, y| low.min(*y));
        assert!(ground.abs() < 0.000_1);
    }

    // SPEC-DISHMESH-002, SPEC-DISHMESH-003: the rigid, hinged, and keep-outside
    // kits match Electron's buildDish output for the same aim.
    #[test]
    fn dish_meshes_match_the_electron_golden() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../fixtures/dish-mesh.json")).unwrap();
        assert_eq!(cases.len(), 3);
        for case in cases {
            let model = case["model"].as_str().unwrap();
            let mut aimed = survey();
            aimed.dish_model = model.into();
            aimed.boresight_azimuth_deg = case["boresightAzimuthDeg"].as_f64().unwrap();
            aimed.boresight_elevation_deg = case["boresightElevationDeg"].as_f64().unwrap();
            let dish = build_dish(&aimed, 1.0);
            assert_eq!(
                dish.data.len() as u64,
                case["floats"].as_u64().unwrap(),
                "{model}"
            );
            let close = |ours: &[f32], theirs: &serde_json::Value, what: &str| {
                for (index, (a, b)) in ours.iter().zip(theirs.as_array().unwrap()).enumerate() {
                    let b = b.as_f64().unwrap() as f32;
                    assert!((a - b).abs() < 1e-5, "{model} {what}[{index}]: {a} vs {b}");
                }
            };
            close(&dish.data[..18], &case["head"], "head");
            close(&dish.data[dish.data.len() - 18..], &case["tail"], "tail");
            close(&dish.origin, &case["origin"], "origin");
            assert!(
                (dish.radius - case["radius"].as_f64().unwrap() as f32).abs() < 1e-5,
                "{model} radius"
            );
        }
    }

    // SPEC-SKY-004: a 90° boresight puts the dish face up, and a 0° one puts it
    // on the horizon.
    #[test]
    fn the_dish_aims_with_the_boresight() {
        let mut up = survey();
        up.boresight_elevation_deg = 90.0;
        let mut side = survey();
        side.boresight_elevation_deg = 0.0;
        let height = |dish: &DishMesh| {
            dish.data
                .iter()
                .skip(1)
                .step_by(6)
                .fold(0.0_f32, |high, y| high.max(*y))
        };
        let up = build_dish(&up, 1.0);
        let side = build_dish(&side, 1.0);
        // Face up the panel lies low; aimed at the horizon it stands on edge.
        assert!(
            height(&up) < 0.5 * height(&side),
            "{} vs {}",
            height(&up),
            height(&side)
        );
    }
}
