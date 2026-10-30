//! The 3-D satellite (spec 0184): a port of Electron's `satelliteGeometry.ts`.
//! Centred on the origin, nose along +z, about 12 units nose to tail and 22
//! across the deployed wings.

/// Floats per vertex: `xyz + normal + rgb`.
pub const SATELLITE_FLOATS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatelliteDetail {
    Full,
    Distant,
}

type Vec3 = [f64; 3];

const BODY: Vec3 = [0.867, 0.867, 0.867];
const DARK: Vec3 = [0.267, 0.267, 0.267];
const PANEL: Vec3 = [0.8, 0.8, 0.8];

/// Segment counts: round sections, thin parts, and nose-cone rings.
struct Segments {
    body: usize,
    thin: usize,
    rings: usize,
}

impl SatelliteDetail {
    fn segments(self) -> Segments {
        match self {
            Self::Full => Segments {
                body: 32,
                thin: 16,
                rings: 16,
            },
            Self::Distant => Segments {
                body: 10,
                thin: 5,
                rings: 3,
            },
        }
    }
}

#[derive(Default)]
struct Builder {
    out: Vec<f32>,
}

impl Builder {
    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3, tint: Vec3) {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = normal[0].hypot(normal[1]).hypot(normal[2]);
        let length = if length > 0.0 { length } else { 1.0 };
        let normal = normal.map(|component| component / length);
        for point in [a, b, c] {
            for value in point.iter().chain(&normal).chain(&tint) {
                self.out.push(*value as f32);
            }
        }
    }

    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, tint: Vec3) {
        self.tri(a, b, c, tint);
        self.tri(a, c, d, tint);
    }

    /// A tube along z, optionally capped and offset sideways.
    #[allow(clippy::too_many_arguments)]
    fn tube(
        &mut self,
        z_back: f64,
        z_front: f64,
        r_back: f64,
        r_front: f64,
        segments: usize,
        tint: Vec3,
        (cap_back, cap_front): (bool, bool),
        (offset_x, offset_y): (f64, f64),
    ) {
        let at = |i: usize, r: f64, z: f64| -> Vec3 {
            let t = i as f64 / segments as f64 * std::f64::consts::PI * 2.0;
            [offset_x + t.cos() * r, offset_y + t.sin() * r, z]
        };
        for i in 0..segments {
            let j = i + 1;
            self.quad(
                at(i, r_back, z_back),
                at(i, r_front, z_front),
                at(j, r_front, z_front),
                at(j, r_back, z_back),
                tint,
            );
            if cap_front {
                self.tri(
                    [offset_x, offset_y, z_front],
                    at(j, r_front, z_front),
                    at(i, r_front, z_front),
                    tint,
                );
            }
            if cap_back {
                self.tri(
                    [offset_x, offset_y, z_back],
                    at(i, r_back, z_back),
                    at(j, r_back, z_back),
                    tint,
                );
            }
        }
    }

    /// The nose: a hemisphere capping the front, opening back toward -z.
    fn hemisphere(&mut self, z: f64, radius: f64, segments: usize, rings: usize, tint: Vec3) {
        use std::f64::consts::PI;
        let at = |ring: usize, seg: usize| -> Vec3 {
            let phi = ring as f64 / rings as f64 * (PI / 2.0);
            let theta = seg as f64 / segments as f64 * PI * 2.0;
            let r = phi.sin() * radius;
            [theta.cos() * r, theta.sin() * r, z + phi.cos() * radius]
        };
        for ring in 0..rings {
            for seg in 0..segments {
                let (a, c) = (at(ring, seg), at(ring, seg + 1));
                let (d, e) = (at(ring + 1, seg), at(ring + 1, seg + 1));
                if ring == 0 {
                    self.tri(a, e, d, tint); // the pole fan
                } else {
                    self.quad(a, c, e, d, tint);
                }
            }
        }
    }

    /// An axis-aligned box: the solar boards and their raised sections.
    #[allow(clippy::too_many_arguments)]
    fn cuboid(&mut self, cx: f64, cy: f64, cz: f64, w: f64, h: f64, d: f64, tint: Vec3) {
        let (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
        let (y0, y1) = (cy - h / 2.0, cy + h / 2.0);
        let (z0, z1) = (cz - d / 2.0, cz + d / 2.0);
        self.quad([x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1], tint); // +z
        self.quad([x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0], tint); // -z
        self.quad([x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0], tint); // -x
        self.quad([x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1], tint); // +x
        self.quad([x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0], tint); // +y
        self.quad([x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1], tint); // -y
    }

    /// A strut between two points, as a thin open tube.
    fn strut(&mut self, from: Vec3, to: Vec3, radius: f64, segments: usize, tint: Vec3) {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let length = d[0].hypot(d[1]).hypot(d[2]);
        let length = if length > 0.0 { length } else { 1.0 };
        let dir = d.map(|component| component / length);
        let reference = if dir[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let p = [
            reference[1] * dir[2] - reference[2] * dir[1],
            reference[2] * dir[0] - reference[0] * dir[2],
            reference[0] * dir[1] - reference[1] * dir[0],
        ];
        let p_length = p[0].hypot(p[1]).hypot(p[2]);
        let p_length = if p_length > 0.0 { p_length } else { 1.0 };
        let u = p.map(|component| component / p_length);
        let v = [
            dir[1] * u[2] - dir[2] * u[1],
            dir[2] * u[0] - dir[0] * u[2],
            dir[0] * u[1] - dir[1] * u[0],
        ];
        let ring = |o: Vec3, i: usize| -> Vec3 {
            let t = i as f64 / segments as f64 * std::f64::consts::PI * 2.0;
            let (c, s) = (t.cos() * radius, t.sin() * radius);
            [
                o[0] + u[0] * c + v[0] * s,
                o[1] + u[1] * c + v[1] * s,
                o[2] + u[2] * c + v[2] * s,
            ]
        };
        for i in 0..segments {
            self.quad(
                ring(from, i),
                ring(to, i),
                ring(to, i + 1),
                ring(from, i + 1),
                tint,
            );
        }
    }

    /// One solar wing: the arm, the board, and four raised sections a face.
    fn solar_wing(&mut self, side: f64, z: f64, segments: &Segments) {
        self.strut(
            [0.0, 0.0, z],
            [side * 3.0, 0.0, z],
            0.2,
            segments.thin,
            BODY,
        );
        let (width, height, thickness) = (8.0, 2.5, 0.2);
        let cx = side * (3.0 + width / 2.0);
        self.cuboid(cx, 0.0, z, width, height, thickness, BODY);
        let sections = 4;
        for i in 0..sections {
            let x = cx - width / 2.0 + width / sections as f64 * (i as f64 + 0.5);
            for face in [1.0, -1.0] {
                self.cuboid(
                    x,
                    0.0,
                    z + face * (thickness / 2.0 + 0.05),
                    1.6,
                    2.1,
                    0.1,
                    PANEL,
                );
            }
        }
    }
}

/// The craft as triangles, `SATELLITE_FLOATS` per vertex.
pub fn build_satellite(detail: SatelliteDetail) -> Vec<f32> {
    use std::f64::consts::PI;
    let seg = detail.segments();
    let mut b = Builder::default();
    let none = (false, false);
    let centred = (0.0, 0.0);

    // The body, front to back.
    b.hemisphere(4.0, 2.0, seg.body, seg.rings, BODY);
    b.tube(2.0, 4.0, 2.5, 2.0, seg.body, BODY, none, centred); // front cone
    b.tube(1.0, 2.0, 2.5, 2.5, seg.body, BODY, none, centred); // front barrel
    b.tube(0.0, 1.0, 1.5, 1.5, seg.body, DARK, (true, false), centred); // recessed core
    for i in 0..6 {
        // Struts bridging the gap.
        let t = i as f64 / 6.0 * PI * 2.0;
        b.tube(
            0.0,
            1.0,
            0.15,
            0.15,
            seg.thin,
            BODY,
            (true, true),
            (t.cos() * 2.1, t.sin() * 2.1),
        );
    }
    b.tube(-2.25, 0.0, 2.5, 2.5, seg.body, BODY, none, centred); // rear barrel
    b.tube(-3.75, -2.25, 1.5, 2.5, seg.body, BODY, none, centred); // rear taper
    b.tube(
        -4.25,
        -3.75,
        1.5,
        1.5,
        seg.body,
        BODY,
        (true, false),
        centred,
    );

    // The solar wings, on the rear barrel.
    b.solar_wing(1.0, -1.0, &seg);
    b.solar_wing(-1.0, -1.0, &seg);

    // The rear antenna: a dish opening backwards, its feed on three struts.
    b.tube(-5.75, -4.25, 2.5, 0.5, seg.body, BODY, none, centred); // open cone
    b.tube(
        -4.3,
        -4.25,
        0.5,
        0.5,
        seg.body,
        BODY,
        (true, false),
        centred,
    );
    b.tube(
        -6.35,
        -5.15,
        0.2,
        0.2,
        seg.thin,
        BODY,
        (true, true),
        centred,
    );
    for i in 0..3 {
        let t = i as f64 / 3.0 * PI * 2.0;
        b.strut(
            [t.cos() * 2.4, t.sin() * 2.4, -5.75],
            [0.0, 0.0, -6.25],
            0.05,
            seg.thin,
            BODY,
        );
    }
    b.out
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SATMODEL-001: both detail levels match Electron's buildSatellite.
    #[test]
    fn the_craft_matches_the_electron_golden() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../../contracts/sky/satellite-mesh.json"
        ))
        .unwrap();
        for case in cases {
            let detail = match case["detail"].as_str().unwrap() {
                "full" => SatelliteDetail::Full,
                _ => SatelliteDetail::Distant,
            };
            let mesh = build_satellite(detail);
            let triangles = case["triangleCount"].as_u64().unwrap() as usize;
            assert_eq!(mesh.len(), triangles * 3 * SATELLITE_FLOATS, "{detail:?}");
            for vertex in case["vertices"].as_array().unwrap() {
                let index = vertex["index"].as_u64().unwrap() as usize;
                let ours = &mesh[index * SATELLITE_FLOATS..(index + 1) * SATELLITE_FLOATS];
                for (offset, key) in [(0, "position"), (3, "normal"), (6, "color")] {
                    for (axis, theirs) in vertex[key].as_array().unwrap().iter().enumerate() {
                        let theirs = theirs.as_f64().unwrap() as f32;
                        let a = ours[offset + axis];
                        assert!(
                            (a - theirs).abs() < 1e-5,
                            "{detail:?} vertex {index} {key}[{axis}]: {a} vs {theirs}"
                        );
                    }
                }
            }
        }
    }
}
