//! The speed test's "Starlink" view (spec 0181): a port of upstream `SpeedBeam.tsx`.
//! Geometry and timing here are pure; `draw` paints a frame with Cairo.

/// Electron's SVG viewBox.
pub const VIEW_WIDTH: f64 = 260.0;
pub const VIEW_HEIGHT: f64 = 200.0;
const DISH_BOX: f64 = 46.0;
const DISH_GROUND: (f64, f64) = (130.0, 174.0);
pub const RING_RADII: [f64; 5] = [26.0, 50.0, 76.0, 104.0, 134.0];
const RING_FLATTEN: f64 = 0.24;
pub const ARC: [(f64, f64); 3] = [(-6.0, 42.0), (130.0, -16.0), (266.0, 42.0)];
const TRAVEL_MIN: f64 = -0.35;
const TRAVEL_MAX: f64 = 1.35;
const TRAVEL_SPAN: f64 = TRAVEL_MAX - TRAVEL_MIN;
const ORBIT_PERIOD_MS: f64 = 13_000.0;
pub const MUTED_OPACITY: f64 = 0.4;
const PACKET_PERIOD_MS: f64 = 2_600.0;
const PACKET_TRAIL: f64 = 0.16;

/// A kit's render and where it meets the ground and emits the beam, as
/// fractions of the art's box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DishArt {
    pub png: &'static [u8],
    pub ground_anchor: (f64, f64),
    pub beam_exit_anchor: (f64, f64),
}

/// The art for a `dish_model_for` id.
pub fn dish_art(model: &str) -> DishArt {
    macro_rules! art {
        ($file:literal, $ground:expr, $exit:expr) => {
            DishArt {
                png: include_bytes!(concat!("../assets/dishes/", $file)),
                ground_anchor: (0.5, $ground),
                beam_exit_anchor: $exit,
            }
        };
    }
    match model {
        "rev2Circular" => art!("rev2Circular.png", 0.7793, (0.5305, 0.3242)),
        "rev3Rectangular" => art!("rev3Rectangular.png", 0.7614, (0.5251, 0.3327)),
        "rev5Standard" => art!("rev5Standard.png", 0.6558, (0.5158, 0.4766)),
        "performanceGen1" => art!("performanceGen1.png", 0.7614, (0.5251, 0.3327)),
        "performanceGen2" => art!("performanceGen2.png", 0.6602, (0.5075, 0.4819)),
        "performanceGen3" | "aviation" => art!("performanceGen3.png", 0.6563, (0.4991, 0.5017)),
        "mini1" => art!("mini1.png", 0.6618, (0.5165, 0.4699)),
        "mini2" => art!("mini2.png", 0.6618, (0.5165, 0.4699)),
        // rev4Standard, unknown, and anything newer draw the Standard kit.
        _ => art!("rev4Standard.png", 0.6571, (0.5075, 0.4863)),
    }
}

/// The point at `t` along the orbit arc (a quadratic Bézier).
pub fn arc_point(t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let [p0, p1, p2] = ARC;
    (
        u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
        u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
    )
}

/// How central a satellite is on the visible arc; negative once off screen.
pub fn servability(t: f64) -> f64 {
    if (0.0..=1.0).contains(&t) {
        0.5 - (t - 0.5).abs()
    } else {
        -1.0
    }
}

/// Top-left of the box the art is drawn in.
pub fn dish_box(art: &DishArt) -> (f64, f64) {
    (
        DISH_GROUND.0 - art.ground_anchor.0 * DISH_BOX,
        DISH_GROUND.1 - art.ground_anchor.1 * DISH_BOX,
    )
}

/// Where the beam leaves the panel face.
pub fn beam_origin(art: &DishArt) -> (f64, f64) {
    let (x, y) = dish_box(art);
    (
        x + art.beam_exit_anchor.0 * DISH_BOX,
        y + art.beam_exit_anchor.1 * DISH_BOX,
    )
}

/// The streak's tail and head along the beam, and its opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packet {
    pub tail: (f64, f64),
    pub head: (f64, f64),
    pub opacity: f64,
}

/// One frame of the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct BeamFrame {
    pub satellites: [(f64, f64); 2],
    pub opacities: [f64; 2],
    pub target: (f64, f64),
    /// 0 when the beam is hidden.
    pub beam_opacity: f64,
    pub packet: Option<Packet>,
}

/// The scene `elapsed_ms` into the loop. `linked` lights the beam; `active`
/// (a run in progress) adds the swell and the packet.
pub fn beam_frame(
    elapsed_ms: f64,
    linked: bool,
    active: bool,
    upload: bool,
    foot: (f64, f64),
) -> BeamFrame {
    // Half the travel range apart: one crosses mid-arc while the other is
    // off screen, and they share the frame only at the handoff.
    let phase = (elapsed_ms / ORBIT_PERIOD_MS).rem_euclid(1.0);
    let ts = [
        TRAVEL_MIN + (phase * TRAVEL_SPAN).rem_euclid(TRAVEL_SPAN),
        TRAVEL_MIN + (phase * TRAVEL_SPAN + TRAVEL_SPAN / 2.0).rem_euclid(TRAVEL_SPAN),
    ];
    let satellites = [arc_point(ts[0]), arc_point(ts[1])];
    let served = usize::from(servability(ts[0]) < servability(ts[1]));
    let opacities = [0, 1].map(|index| {
        if linked && index == served {
            1.0
        } else {
            MUTED_OPACITY
        }
    });
    let target = satellites[served];
    let beam_opacity = match (linked, active) {
        (false, _) => 0.0,
        (true, false) => 1.0,
        (true, true) => 0.82 + 0.18 * (elapsed_ms / 520.0).sin(),
    };
    let packet = (linked && active).then(|| {
        let head = elapsed_ms.rem_euclid(PACKET_PERIOD_MS) / PACKET_PERIOD_MS;
        let tail = (head - PACKET_TRAIL).max(0.0);
        let (from, to) = if upload {
            (foot, target)
        } else {
            (target, foot)
        };
        let along = |fraction: f64| {
            (
                from.0 + (to.0 - from.0) * fraction,
                from.1 + (to.1 - from.1) * fraction,
            )
        };
        Packet {
            tail: along(tail),
            head: along(head),
            opacity: (head * std::f64::consts::PI).sin(),
        }
    });
    BeamFrame {
        satellites,
        opacities,
        target,
        beam_opacity,
        packet,
    }
}

/// The scene's colours: ink for the wordmark, the beam's light, and the
/// rings' and orbit's strokes. Electron's `--beam`, `--hairline`, `--baseline`.
pub struct Palette {
    pub ink: (f64, f64, f64),
    pub beam: (f64, f64, f64),
    pub hairline: (f64, f64, f64),
    pub baseline: (f64, f64, f64),
}

impl Palette {
    pub fn for_ink(ink: (f64, f64, f64)) -> Self {
        let dark = (ink.0 + ink.1 + ink.2) / 3.0 > 0.5;
        if dark {
            Self {
                ink,
                beam: (1.0, 1.0, 1.0),
                hairline: rgb(0x23, 0x23, 0x23),
                baseline: rgb(0x3a, 0x3a, 0x3a),
            }
        } else {
            Self {
                ink,
                beam: rgb(0xd4, 0xd4, 0xd8),
                hairline: rgb(0xe7, 0xe5, 0xdc),
                baseline: rgb(0xc9, 0xc7, 0xbc),
            }
        }
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    (
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
    )
}

/// The decoded art, or `None` if the bundled PNG cannot be read.
pub fn art_surface(art: &DishArt) -> Option<gtk::cairo::ImageSurface> {
    gtk::cairo::ImageSurface::create_from_png(&mut &art.png[..]).ok()
}

/// A soft line: wide faint strokes under the line itself stand in for the SVG
/// blur filter, which Cairo lacks.
fn glow_line(
    cr: &gtk::cairo::Context,
    from: (f64, f64),
    to: (f64, f64),
    color: (f64, f64, f64),
    width: f64,
    alpha: f64,
) {
    for (spread, share) in [(3.2, 0.12), (2.2, 0.22), (1.4, 0.4)] {
        cr.set_source_rgba(color.0, color.1, color.2, alpha * share);
        cr.set_line_width(width * spread);
        cr.move_to(from.0, from.1);
        cr.line_to(to.0, to.1);
        let _ = cr.stroke();
    }
}

/// Paint one frame in viewBox units; the caller scales the context.
pub fn draw(
    cr: &gtk::cairo::Context,
    frame: &BeamFrame,
    art: &DishArt,
    surface: Option<&gtk::cairo::ImageSurface>,
    palette: &Palette,
) {
    use std::f64::consts::TAU;
    cr.set_line_cap(gtk::cairo::LineCap::Round);

    let (hairline, baseline, beam) = (palette.hairline, palette.baseline, palette.beam);
    cr.set_line_width(1.0);
    cr.set_source_rgb(hairline.0, hairline.1, hairline.2);
    for radius in RING_RADII {
        let _ = cr.save();
        cr.translate(DISH_GROUND.0, DISH_GROUND.1);
        cr.scale(radius, radius * RING_FLATTEN);
        cr.arc(0.0, 0.0, 1.0, 0.0, TAU);
        let _ = cr.restore();
        let _ = cr.stroke();
    }

    // A quadratic as Cairo's cubic: control points two-thirds of the way in.
    let [p0, p1, p2] = ARC;
    cr.set_source_rgb(baseline.0, baseline.1, baseline.2);
    cr.set_dash(&[3.0, 5.0], 0.0);
    cr.move_to(p0.0, p0.1);
    cr.curve_to(
        p0.0 + 2.0 / 3.0 * (p1.0 - p0.0),
        p0.1 + 2.0 / 3.0 * (p1.1 - p0.1),
        p2.0 + 2.0 / 3.0 * (p1.0 - p2.0),
        p2.1 + 2.0 / 3.0 * (p1.1 - p2.1),
        p2.0,
        p2.1,
    );
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);

    let origin = beam_origin(art);
    if frame.beam_opacity > 0.0 {
        let alpha = frame.beam_opacity;
        glow_line(cr, origin, frame.target, beam, 4.2, 0.8 * alpha);
        cr.set_source_rgba(beam.0, beam.1, beam.2, 0.98 * alpha);
        cr.set_line_width(1.8);
        cr.move_to(origin.0, origin.1);
        cr.line_to(frame.target.0, frame.target.1);
        let _ = cr.stroke();
        if let Some(packet) = frame.packet {
            glow_line(
                cr,
                packet.tail,
                packet.head,
                beam,
                4.6,
                packet.opacity * alpha,
            );
        }
    }

    for (centre, opacity) in frame.satellites.iter().zip(frame.opacities) {
        let halo =
            gtk::cairo::RadialGradient::new(centre.0, centre.1, 0.0, centre.0, centre.1, 14.0);
        halo.add_color_stop_rgba(0.0, beam.0, beam.1, beam.2, 0.55 * opacity);
        halo.add_color_stop_rgba(0.35, beam.0, beam.1, beam.2, 0.12 * opacity);
        halo.add_color_stop_rgba(1.0, beam.0, beam.1, beam.2, 0.0);
        let _ = cr.set_source(&halo);
        cr.arc(centre.0, centre.1, 14.0, 0.0, TAU);
        let _ = cr.fill();
        cr.set_source_rgba(beam.0, beam.1, beam.2, opacity);
        cr.arc(centre.0, centre.1, 5.5, 0.0, TAU);
        let _ = cr.fill();
    }

    // The dish over the beam's base, so the beam emerges from behind it.
    if let Some(surface) = surface {
        let (x, y) = dish_box(art);
        let _ = cr.save();
        cr.translate(x, y);
        cr.scale(
            DISH_BOX / f64::from(surface.width().max(1)),
            DISH_BOX / f64::from(surface.height().max(1)),
        );
        let _ = cr.set_source_surface(surface, 0.0, 0.0);
        let _ = cr.paint();
        let _ = cr.restore();
    }

    // The wordmark, set wide in the clear band left of the beam.
    let ink = palette.ink;
    cr.select_font_face(
        "sans-serif",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Bold,
    );
    cr.set_font_size(12.0);
    cr.set_source_rgb(ink.0, ink.1, ink.2);
    let tracking = 12.0 * 0.22;
    let word = "STARLINK";
    let advances: Vec<f64> = word
        .chars()
        .map(|letter| {
            cr.text_extents(&letter.to_string())
                .map_or(0.0, |extents| extents.x_advance())
        })
        .collect();
    let width: f64 = advances.iter().sum::<f64>() + tracking * (advances.len() as f64 - 1.0);
    let mut x = 78.0 - width / 2.0;
    for (letter, advance) in word.chars().zip(advances) {
        cr.move_to(x, 112.0);
        let _ = cr.show_text(&letter.to_string());
        x += advance + tracking;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    // SPEC-BEAM-002: the orbit is Electron's arc, and the beam follows the
    // satellite more central on screen, handing off as they cross.
    #[test]
    fn the_orbit_and_handoff_match_electron() {
        assert!(close(arc_point(0.0), (-6.0, 42.0)));
        assert!(close(arc_point(0.5), (130.0, 13.0)));
        assert!(close(arc_point(1.0), (266.0, 42.0)));
        assert_eq!(servability(0.5), 0.5);
        assert_eq!(servability(1.2), -1.0);

        // At the start of the loop satellite 0 is off the left edge (t −0.35)
        // and satellite 1 at t 0.5, mid-arc: it serves.
        let start = beam_frame(0.0, true, true, false, (130.0, 160.0));
        assert_eq!(start.opacities, [MUTED_OPACITY, 1.0]);
        assert!(close(start.target, arc_point(0.5)));
        assert!(close(start.satellites[1], arc_point(0.5)));
        // Half a loop later they have swapped roles.
        let half = beam_frame(6_500.0, true, true, false, (130.0, 160.0));
        assert_eq!(half.opacities, [1.0, MUTED_OPACITY]);
        // Unlinked, neither is lit.
        assert_eq!(
            beam_frame(0.0, false, false, false, (0.0, 0.0)).opacities,
            [MUTED_OPACITY; 2]
        );
    }

    // SPEC-BEAM-003: the swell and packet run only during a test; the packet
    // heads to the dish on download and away from it on upload.
    #[test]
    fn the_beam_swells_and_carries_a_packet_while_testing() {
        let foot = (130.0, 160.0);
        let hidden = beam_frame(1_000.0, false, false, false, foot);
        assert_eq!((hidden.beam_opacity, hidden.packet), (0.0, None));
        let lit_after_run = beam_frame(1_000.0, true, false, false, foot);
        assert_eq!(
            (lit_after_run.beam_opacity, lit_after_run.packet),
            (1.0, None)
        );

        for ms in (0..13_000).step_by(97) {
            let frame = beam_frame(f64::from(ms), true, true, false, foot);
            assert!(
                (0.64 - 1e-9..=1.0 + 1e-9).contains(&frame.beam_opacity),
                "{}",
                frame.beam_opacity
            );
        }

        // A quarter-way packet (650 ms into 2.6 s): head 0.25, tail 0.09.
        let down = beam_frame(650.0, true, true, false, foot);
        let packet = down.packet.expect("packet while testing");
        let along = |fraction: f64, from: (f64, f64), to: (f64, f64)| {
            (
                from.0 + (to.0 - from.0) * fraction,
                from.1 + (to.1 - from.1) * fraction,
            )
        };
        assert!(close(packet.head, along(0.25, down.target, foot)));
        assert!(close(packet.tail, along(0.09, down.target, foot)));
        assert!((packet.opacity - (0.25 * std::f64::consts::PI).sin()).abs() < 1e-9);
        let up = beam_frame(650.0, true, true, true, foot).packet.unwrap();
        assert!(close(up.head, along(0.25, foot, down.target)));
    }

    // SPEC-BEAM-004: each kit has its art and anchors; the beam starts on the
    // panel and the art stands on the rings.
    #[test]
    fn dish_art_seats_on_the_rings() {
        let standard = dish_art("rev4Standard");
        assert_eq!(standard.ground_anchor, (0.5, 0.6571));
        assert_eq!(standard.beam_exit_anchor, (0.5075, 0.4863));
        assert_eq!(dish_art("unknown"), standard);
        assert_eq!(dish_art("something new"), standard);
        assert_eq!(dish_art("mini2").beam_exit_anchor, (0.5165, 0.4699));
        assert_eq!(dish_art("aviation").png, dish_art("performanceGen3").png);
        assert!(standard.png.starts_with(b"\x89PNG"));

        let (x, y) = dish_box(&standard);
        assert!(close((x + 0.5 * 46.0, y + 0.6571 * 46.0), (130.0, 174.0)));
        let origin = beam_origin(&standard);
        assert!(close(origin, (x + 0.5075 * 46.0, y + 0.4863 * 46.0)));
    }
}
