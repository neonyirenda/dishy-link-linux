//! Restoring the window without restoring it onto a monitor that has gone away.
//!
//! Electron's rule is exactly this: only reuse saved bounds when their centre
//! still lands on an attached display. The Rust port adds the clamp Electron
//! leaves to the window manager, so a display that shrank cannot leave half the
//! window unreachable.

use crate::preferences::WindowBounds;

pub const DEFAULT_WIDTH: i32 = 1500;
pub const DEFAULT_HEIGHT: i32 = 980;
pub const MIN_WIDTH: i32 = 800;
pub const MIN_HEIGHT: i32 = 700;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Display {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Display {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn contains(self, x: i64, y: i64) -> bool {
        x >= i64::from(self.x)
            && x < i64::from(self.x) + i64::from(self.width)
            && y >= i64::from(self.y)
            && y < i64::from(self.y) + i64::from(self.height)
    }
}

/// Clamp saved bounds to the display under their centre, or `None` when that
/// display no longer exists (or the saved size is unusable).
pub fn clamp_bounds(bounds: WindowBounds, displays: &[Display]) -> Option<WindowBounds> {
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }
    let center_x = i64::from(bounds.x) + i64::from(bounds.width) / 2;
    let center_y = i64::from(bounds.y) + i64::from(bounds.height) / 2;
    let display = displays.iter().copied().find(|display| {
        display.width > 0 && display.height > 0 && display.contains(center_x, center_y)
    })?;

    let width = bounds.width.max(MIN_WIDTH).min(display.width.max(1));
    let height = bounds.height.max(MIN_HEIGHT).min(display.height.max(1));
    let x = bounds.x.clamp(display.x, display.x + display.width - width);
    let y = bounds
        .y
        .clamp(display.y, display.y + display.height - height);
    Some(WindowBounds::new(x, y, width, height))
}

/// The size a window opens at when there is nothing usable to restore.
pub fn default_bounds() -> (i32, i32) {
    (DEFAULT_WIDTH, DEFAULT_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SHELL-005: saved bounds survive a restart, but only on a display
    // that is still attached; an unplugged monitor falls back to defaults.
    #[test]
    fn restores_bounds_on_an_attached_display() {
        let displays = [Display::new(0, 0, 1920, 1080)];
        let saved = WindowBounds::new(100, 80, 1400, 900);
        assert_eq!(clamp_bounds(saved, &displays), Some(saved));
    }

    #[test]
    fn drops_bounds_whose_centre_is_on_no_connected_display() {
        let displays = [Display::new(0, 0, 1920, 1080)];
        let saved = WindowBounds::new(2600, 120, 1400, 900);
        assert_eq!(clamp_bounds(saved, &displays), None);
        assert_eq!(
            clamp_bounds(WindowBounds::new(0, 0, 0, 900), &displays),
            None
        );
        assert_eq!(default_bounds(), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
    }

    #[test]
    fn clamps_oversized_and_off_edge_bounds_onto_the_display() {
        let displays = [Display::new(0, 0, 1920, 1080)];
        let clamped = clamp_bounds(WindowBounds::new(100, 0, 3000, 2000), &displays)
            .expect("centre is on the display");
        assert_eq!(clamped, WindowBounds::new(0, 0, 1920, 1080));
    }

    #[test]
    fn handles_monitors_with_negative_coordinates() {
        let displays = [
            Display::new(0, 0, 1920, 1080),
            Display::new(-2560, 0, 2560, 1440),
        ];
        let saved = WindowBounds::new(-2000, 300, 1000, 800);
        assert_eq!(clamp_bounds(saved, &displays), Some(saved));
    }
}
