//! Orbit camera and pointer maths for the sky view.
//!
//! Owns where the viewer is and how input moves them: yaw/pitch/zoom, flick
//! inertia after a drag, auto-rotation, and the eased reset. The renderer asks
//! for an eye position once per frame and otherwise leaves it alone.

use crate::math::Vec3;

/// One turn every two minutes: present when you look at it, never the thing
/// moving in the corner of your eye.
const AUTO_ROTATE_RAD_PER_SEC: f64 = std::f64::consts::TAU / 120.0;
/// Rotation yields for this long after a drag.
const RESUME_AFTER_DRAG_MS: f64 = 1_000.0;
const RESET_SECONDS: f64 = 2.4;
const INITIAL: (f64, f64, f64) = (2.6, 0.34, 4.3);
const PITCH_MIN: f64 = 0.03;
const PITCH_MAX: f64 = 1.35;
const DISTANCE_MIN: f64 = 0.45;
const DISTANCE_MAX: f64 = 5.5;
const TARGET_Y_FAR: f64 = 0.42;
const TARGET_Y_NEAR: f64 = 0.13;
const FOCUS_FAR: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub eye: Vec3,
    pub target: Vec3,
}

#[derive(Debug, Clone, Copy)]
struct ResetGlide {
    from_pitch: f64,
    to_pitch: f64,
    from_distance: f64,
    to_distance: f64,
    progress: f64,
}

#[derive(Debug, Clone)]
pub struct SkyCamera {
    yaw: f64,
    pitch: f64,
    distance: f64,
    model_floor: f64,
    home_pitch: f64,
    home_distance: f64,
    reset: Option<ResetGlide>,
    auto_rotate: bool,
    dragging: bool,
    velocity: f64,
    last_interaction_ms: f64,
}

impl Default for SkyCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl SkyCamera {
    pub fn new() -> Self {
        let (yaw, pitch, distance) = INITIAL;
        Self {
            yaw,
            pitch,
            distance,
            model_floor: 0.0,
            home_pitch: pitch,
            home_distance: distance,
            reset: None,
            auto_rotate: true,
            dragging: false,
            velocity: 0.0,
            last_interaction_ms: f64::NEG_INFINITY,
        }
    }

    pub fn yaw(&self) -> f64 {
        self.yaw
    }

    pub fn pitch(&self) -> f64 {
        self.pitch
    }

    pub fn distance(&self) -> f64 {
        self.distance
    }

    pub fn is_rotating(&self) -> bool {
        self.auto_rotate
    }

    pub fn drag_start(&mut self, now_ms: f64) {
        self.dragging = true;
        self.reset = None;
        self.velocity = 0.0;
        self.last_interaction_ms = now_ms;
    }

    pub fn drag_by(&mut self, dx: f64, dy: f64, now_ms: f64) {
        if !self.dragging {
            return;
        }
        self.yaw -= dx * 0.005;
        self.velocity = -dx * 0.005;
        self.pitch = (self.pitch + dy * 0.004).clamp(PITCH_MIN, PITCH_MAX);
        self.last_interaction_ms = now_ms;
    }

    pub fn drag_end(&mut self) {
        self.dragging = false;
    }

    pub fn zoom(&mut self, delta: f64, now_ms: f64) {
        self.reset = None;
        self.distance = (self.distance + delta * 0.0035).clamp(self.floor(), DISTANCE_MAX);
        self.last_interaction_ms = now_ms;
    }

    /// Returns the new rotation state, so a button can render the right icon.
    pub fn toggle_rotation(&mut self) -> bool {
        self.auto_rotate = !self.auto_rotate;
        if self.auto_rotate {
            self.last_interaction_ms = f64::NEG_INFINITY;
        }
        self.auto_rotate
    }

    pub fn set_min_distance(&mut self, distance: f64) {
        self.model_floor = distance;
        if self.distance < self.floor() {
            self.distance = self.floor();
        }
    }

    /// Ease tilt and distance back to the opening framing, leaving the heading
    /// untouched. Pressing it again mid-glide re-eases from the current spot.
    pub fn reset_view(&mut self) {
        self.reset = Some(ResetGlide {
            from_pitch: self.pitch,
            to_pitch: self.home_pitch,
            from_distance: self.distance,
            to_distance: self.home_distance,
            progress: 0.0,
        });
    }

    pub fn view(&mut self, now_ms: f64, dt_seconds: f64) -> CameraView {
        if !self.dragging {
            self.yaw += self.velocity;
            self.velocity *= 0.94;
            if self.auto_rotate && now_ms - self.last_interaction_ms > RESUME_AFTER_DRAG_MS {
                self.yaw -= dt_seconds * AUTO_ROTATE_RAD_PER_SEC;
            }
        }
        if let Some(reset) = self.reset.as_mut() {
            reset.progress = (reset.progress + dt_seconds / RESET_SECONDS).min(1.0);
            let eased = 0.5 - 0.5 * (std::f64::consts::PI * reset.progress).cos();
            self.pitch = reset.from_pitch + (reset.to_pitch - reset.from_pitch) * eased;
            self.distance = reset.from_distance + (reset.to_distance - reset.from_distance) * eased;
            if reset.progress >= 1.0 {
                self.reset = None;
            }
        }
        let zoomed_out =
            ((self.distance - DISTANCE_MIN) / (FOCUS_FAR - DISTANCE_MIN)).clamp(0.0, 1.0);
        let target = [
            0.0,
            TARGET_Y_NEAR + (TARGET_Y_FAR - TARGET_Y_NEAR) * zoomed_out,
            0.0,
        ];
        CameraView {
            target,
            eye: [
                target[0] + self.distance * self.pitch.cos() * self.yaw.sin(),
                target[1] + self.distance * self.pitch.sin(),
                target[2] + self.distance * self.pitch.cos() * self.yaw.cos(),
            ],
        }
    }

    fn floor(&self) -> f64 {
        DISTANCE_MIN.max(self.model_floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SKY-004: drag turns the camera and pitch stops short of the poles.
    #[test]
    fn drag_turns_and_clamps_pitch() {
        let mut camera = SkyCamera::new();
        let yaw = camera.yaw();
        camera.drag_start(0.0);
        camera.drag_by(40.0, 1_000.0, 10.0);
        assert!(camera.yaw() < yaw);
        assert_eq!(camera.pitch(), PITCH_MAX);

        camera.drag_by(0.0, -10_000.0, 20.0);
        assert_eq!(camera.pitch(), PITCH_MIN);
        camera.drag_end();
    }

    // SPEC-SKY-004: the wheel is bounded, including a model clearance floor.
    #[test]
    fn zoom_clamps_between_the_floor_and_the_widest_orbit() {
        let mut camera = SkyCamera::new();
        camera.zoom(-10_000.0, 0.0);
        assert_eq!(camera.distance(), DISTANCE_MIN);
        camera.zoom(10_000.0, 0.0);
        assert_eq!(camera.distance(), DISTANCE_MAX);

        camera.set_min_distance(2.5);
        assert_eq!(camera.distance(), DISTANCE_MAX);
        camera.zoom(-10_000.0, 0.0);
        assert_eq!(camera.distance(), 2.5);
    }

    // SPEC-SKY-004: auto-rotation resumes after the idle delay, and the pause
    // button stops it immediately.
    #[test]
    fn rotation_yields_to_a_drag_then_resumes() {
        let mut camera = SkyCamera::new();
        camera.drag_start(0.0);
        camera.drag_by(10.0, 0.0, 0.0);
        camera.drag_end();
        let held = camera.yaw();
        camera.view(500.0, 0.5);
        let after_inertia = camera.yaw();
        assert!((after_inertia - held).abs() < 0.05);

        camera.view(2_000.0, 1.0);
        assert!((camera.yaw() - after_inertia).abs() > 0.01);

        assert!(!camera.toggle_rotation());
        camera.velocity = 0.0;
        let paused = camera.yaw();
        camera.view(20_000.0, 1.0);
        assert_eq!(camera.yaw(), paused);
        assert!(camera.toggle_rotation());
    }

    // SPEC-SKY-004: reset eases pitch and distance home and leaves yaw alone.
    #[test]
    fn reset_returns_tilt_and_zoom_not_heading() {
        let mut camera = SkyCamera::new();
        let home_pitch = camera.pitch();
        let home_distance = camera.distance();
        camera.drag_start(0.0);
        camera.drag_by(100.0, 600.0, 0.0);
        camera.drag_end();
        camera.zoom(400.0, 0.0);
        let heading = camera.yaw();
        camera.reset_view();
        for step in 0..40 {
            camera.view(step as f64 * 100.0, 0.1);
        }
        assert!((camera.pitch() - home_pitch).abs() < 0.001);
        assert!((camera.distance() - home_distance).abs() < 0.001);
        assert_ne!(camera.yaw(), heading);
    }

    #[test]
    fn the_eye_sits_at_the_requested_distance_from_the_target() {
        let mut camera = SkyCamera::new();
        let view = camera.view(0.0, 0.0);
        let dx = view.eye[0] - view.target[0];
        let dy = view.eye[1] - view.target[1];
        let dz = view.eye[2] - view.target[2];
        assert!(((dx * dx + dy * dy + dz * dz).sqrt() - camera.distance()).abs() < 0.000_1);
    }
}
