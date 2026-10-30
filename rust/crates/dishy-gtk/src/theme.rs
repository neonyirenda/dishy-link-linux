//! The Starlink dark glass (spec 0190): bundled typefaces, the theme CSS, and
//! the tinted backdrop behind the window's content.

use std::path::{Path, PathBuf};

/// The link as the backdrop's glow shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMood {
    Online,
    Degraded,
    Offline,
}

/// Unreachable devices are offline; an active warning or critical alert while
/// reachable is degraded.
pub fn mood_for(online: bool, alerting: bool) -> LinkMood {
    match (online, alerting) {
        (false, _) => LinkMood::Offline,
        (true, true) => LinkMood::Degraded,
        (true, false) => LinkMood::Online,
    }
}

/// The glow colour for a mood.
pub fn mood_tint(mood: LinkMood) -> (f64, f64, f64) {
    match mood {
        // The earth-limb blue of the Starlink site's hero.
        LinkMood::Online => (0.22, 0.42, 0.85),
        // Electron's --accent amber.
        LinkMood::Degraded => (0.88, 0.64, 0.13),
        // Electron's --status-critical.
        LinkMood::Offline => (0.96, 0.25, 0.37),
    }
}

/// `from` to `to` by `t` in 0..=1, for the cross-fade.
pub fn blend(from: (f64, f64, f64), to: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: f64, b: f64| a + (b - a) * t;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// The backdrop base's opacity: solid, or thin enough for a compositor blur.
pub fn base_alpha(see_through: bool) -> f64 {
    // Dark enough to stay dark glass over a bright desktop.
    if see_through { 0.72 } else { 1.0 }
}

/// The bundled faces: Electron's interface and figure typefaces, SIL OFL.
const FONTS: [(&str, &[u8]); 7] = [
    (
        "Barlow-Regular.ttf",
        include_bytes!("../assets/fonts/Barlow-Regular.ttf"),
    ),
    (
        "Barlow-Medium.ttf",
        include_bytes!("../assets/fonts/Barlow-Medium.ttf"),
    ),
    (
        "Barlow-SemiBold.ttf",
        include_bytes!("../assets/fonts/Barlow-SemiBold.ttf"),
    ),
    (
        "Barlow-Bold.ttf",
        include_bytes!("../assets/fonts/Barlow-Bold.ttf"),
    ),
    (
        "IBMPlexMono-Regular.ttf",
        include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    ),
    (
        "IBMPlexMono-Medium.ttf",
        include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
    ),
    (
        "IBMPlexMono-SemiBold.ttf",
        include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf"),
    ),
];

/// Write the faces under `cache`, skipping any already there intact. Pango
/// loads fonts from files, so the embedded bytes need a home on disk.
pub fn write_fonts(cache: &Path) -> Vec<PathBuf> {
    if let Err(error) = std::fs::create_dir_all(cache) {
        tracing::warn!("font cache unavailable: {error}");
        return Vec::new();
    }
    FONTS
        .iter()
        .filter_map(|(name, bytes)| {
            let path = cache.join(name);
            let current =
                std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64);
            if !current && let Err(error) = std::fs::write(&path, bytes) {
                tracing::warn!("could not write {name}: {error}");
                return None;
            }
            Some(path)
        })
        .collect()
}

/// Write the faces and add them to GTK's font map. Needs GTK initialised.
pub fn register_fonts(cache: &Path) -> Vec<PathBuf> {
    let written = write_fonts(cache);
    use gtk::prelude::*;
    let Some(font_map) = gtk::Label::new(None).pango_context().font_map() else {
        return written;
    };
    for path in &written {
        if let Err(error) = font_map.add_font_file(path) {
            tracing::warn!("could not register {}: {error}", path.display());
        }
    }
    written
}

/// Electron's dark tokens as GTK CSS. Everything glass is scoped to
/// `window.dishy-glass`, which the window carries only in dark mode.
pub const THEME_CSS: &str = r#"
window {
  font-family: "Barlow", sans-serif;
}
.monospace,
.dishy-mono {
  font-family: "IBM Plex Mono", monospace;
}
window.dishy-glass,
window.dishy-glass .background,
window.dishy-glass headerbar,
window.dishy-glass toolbarview,
window.dishy-glass stack,
window.dishy-glass scrolledwindow,
window.dishy-glass viewport,
window.dishy-glass statuspage,
window.dishy-glass preferencespage,
window.dishy-glass .view {
  background: transparent;
  background-color: transparent;
  box-shadow: none;
}
window.dishy-glass headerbar {
  border-bottom: none;
}
window.dishy-glass .card,
window.dishy-glass .dishy-card,
window.dishy-glass .boxed-list,
window.dishy-glass list.boxed-list {
  background-color: rgba(255, 255, 255, 0.045);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 12px;
  box-shadow: none;
}
window.dishy-glass list.boxed-list > row {
  background-color: transparent;
}
window.dishy-glass .dishy-toolbar-dock,
window.dishy-glass .dishy-toolbar-rail {
  background-color: rgba(10, 12, 16, 0.72);
  border: 1px solid rgba(255, 255, 255, 0.09);
}
window.dishy-glass popover > contents {
  background-color: rgba(14, 16, 20, 0.96);
  border: 1px solid rgba(255, 255, 255, 0.08);
}
"#;

/// Load the theme above the per-view styles, and register the typefaces.
pub fn install() {
    use std::sync::Once;
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        register_fonts(&gtk::glib::user_cache_dir().join("dishylink/fonts"));
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(THEME_CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    });
}

/// How long the glow takes to cross-fade to a new mood.
const FADE_US: i64 = 600_000;

struct GlassState {
    from: std::cell::Cell<(f64, f64, f64)>,
    to: std::cell::Cell<(f64, f64, f64)>,
    started_us: std::cell::Cell<i64>,
    mood: std::cell::Cell<LinkMood>,
    see_through: std::cell::Cell<bool>,
}

impl GlassState {
    fn tint_at(&self, now_us: i64) -> (f64, f64, f64) {
        let t = (now_us - self.started_us.get()) as f64 / FADE_US as f64;
        blend(self.from.get(), self.to.get(), t)
    }
}

/// The dark glass behind the window's content: a black base, the earth-limb
/// glow tinted by the link, faint stars, and Electron's dot grid.
#[derive(Clone)]
pub struct GlassBackdrop {
    area: gtk::DrawingArea,
    state: std::rc::Rc<GlassState>,
}

impl Default for GlassBackdrop {
    fn default() -> Self {
        Self::new()
    }
}

impl GlassBackdrop {
    pub fn new() -> Self {
        use gtk::prelude::*;
        let tint = mood_tint(LinkMood::Online);
        let state = std::rc::Rc::new(GlassState {
            from: std::cell::Cell::new(tint),
            to: std::cell::Cell::new(tint),
            started_us: std::cell::Cell::new(0),
            mood: std::cell::Cell::new(LinkMood::Online),
            see_through: std::cell::Cell::new(false),
        });
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        area.set_widget_name("glass-backdrop");
        area.set_can_target(false);
        let drawing = state.clone();
        area.set_draw_func(move |_, cr, width, height| {
            let tint = drawing.tint_at(gtk::glib::monotonic_time());
            paint_backdrop(
                cr,
                f64::from(width),
                f64::from(height),
                tint,
                base_alpha(drawing.see_through.get()),
            );
        });
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Cross-fade the glow to the mood's tint.
    pub fn set_mood(&self, mood: LinkMood) {
        use gtk::prelude::*;
        if self.state.mood.replace(mood) == mood {
            return;
        }
        let now = gtk::glib::monotonic_time();
        self.state.from.set(self.state.tint_at(now));
        self.state.to.set(mood_tint(mood));
        self.state.started_us.set(now);
        let state = self.state.clone();
        self.area.add_tick_callback(move |area, _| {
            area.queue_draw();
            if gtk::glib::monotonic_time() - state.started_us.get() >= FADE_US {
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
    }

    pub fn mood(&self) -> LinkMood {
        self.state.mood.get()
    }

    pub fn set_see_through(&self, on: bool) {
        use gtk::prelude::*;
        self.state.see_through.set(on);
        self.area.queue_draw();
    }

    pub fn base_alpha(&self) -> f64 {
        base_alpha(self.state.see_through.get())
    }
}

fn paint_backdrop(
    cr: &gtk::cairo::Context,
    width: f64,
    height: f64,
    tint: (f64, f64, f64),
    alpha: f64,
) {
    use gtk::cairo::{Operator, RadialGradient};
    use std::f64::consts::TAU;
    let (r, g, b) = tint;
    cr.set_operator(Operator::Source);
    cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
    let _ = cr.paint();
    cr.set_operator(Operator::Over);

    // A faint ambient wash from the top left, so the glass is never flat.
    let ambient = RadialGradient::new(
        width * 0.15,
        -height * 0.1,
        0.0,
        width * 0.15,
        -height * 0.1,
        width * 0.7,
    );
    ambient.add_color_stop_rgba(0.0, r, g, b, 0.07);
    ambient.add_color_stop_rgba(1.0, r, g, b, 0.0);
    let _ = cr.set_source(&ambient);
    let _ = cr.paint();

    // Electron's dot grid.
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
    let mut y = 12.0;
    while y < height {
        let mut x = 12.0;
        while x < width {
            cr.arc(x, y, 0.7, 0.0, TAU);
            let _ = cr.fill();
            x += 24.0;
        }
        y += 24.0;
    }

    // Faint stars, placed by a fixed generator so they never move.
    let mut seed: u32 = 0x5eed_1e57;
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1_u32 << 24)
    };
    for _ in 0..90 {
        let (x, y, size, glow) = (
            next() * width,
            next() * height * 0.72,
            0.5 + next() * 0.7,
            0.15 + next() * 0.4,
        );
        cr.set_source_rgba(1.0, 1.0, 1.0, glow);
        cr.arc(x, y, size, 0.0, TAU);
        let _ = cr.fill();
    }

    // The earth's limb rising from the bottom edge, its atmosphere glowing.
    let radius = width.max(height) * 1.25;
    let (cx, cy) = (width / 2.0, height * 0.82 + radius);
    let rim = RadialGradient::new(cx, cy, radius * 0.96, cx, cy, radius * 1.1);
    rim.add_color_stop_rgba(0.0, r, g, b, 0.0);
    rim.add_color_stop_rgba(0.2857, r, g, b, 0.42);
    rim.add_color_stop_rgba(1.0, r, g, b, 0.0);
    let _ = cr.set_source(&rim);
    let _ = cr.paint();
    cr.arc(cx, cy, radius, 0.0, TAU);
    cr.set_source_rgba(0.01, 0.015, 0.03, alpha);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(r, g, b, 0.55);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-GLASS-003: reachability and alerts choose the mood; each mood has
    // its own tint, and the cross-fade runs from one to the other.
    #[test]
    fn the_glow_follows_the_link() {
        assert_eq!(mood_for(true, false), LinkMood::Online);
        assert_eq!(mood_for(true, true), LinkMood::Degraded);
        assert_eq!(mood_for(false, false), LinkMood::Offline);
        assert_eq!(mood_for(false, true), LinkMood::Offline);
        let (online, degraded, offline) = (
            mood_tint(LinkMood::Online),
            mood_tint(LinkMood::Degraded),
            mood_tint(LinkMood::Offline),
        );
        assert!(online.2 > online.0, "online is cool blue");
        assert!(
            degraded.0 > degraded.2 && degraded.1 > degraded.2,
            "degraded is amber"
        );
        assert!(
            offline.0 > offline.1 && offline.0 > offline.2,
            "offline is red"
        );
        assert_eq!(blend(online, offline, 0.0), online);
        assert_eq!(blend(online, offline, 1.0), offline);
        let middle = blend((0.0, 0.0, 0.0), (1.0, 0.5, 0.0), 0.5);
        assert_eq!(middle, (0.5, 0.25, 0.0));
        assert_eq!(blend(online, offline, 7.0), offline, "clamped");
    }

    // SPEC-GLASS-004: see-through thins the base so a blur shows through.
    #[test]
    fn see_through_thins_the_base() {
        assert_eq!(base_alpha(false), 1.0);
        assert!(base_alpha(true) < 0.8 && base_alpha(true) > 0.5);
    }

    // SPEC-GLASS-001: the seven faces are written once and their licences
    // ship beside them.
    #[test]
    fn fonts_are_written_to_the_cache() {
        let directory = tempfile::tempdir().expect("temp dir");
        let written = write_fonts(directory.path());
        assert_eq!(written.len(), 7);
        for path in &written {
            assert!(path.exists() && path.extension().is_some_and(|ext| ext == "ttf"));
        }
        let again = write_fonts(directory.path());
        assert_eq!(again, written, "a second start reuses the files");
    }

    // SPEC-GLASS-006: a fixed white text colour is invisible on light-mode
    // cards. Only selectors that paint their own dark or coloured surface, or
    // a swatch, may fix one.
    #[test]
    fn view_stylesheets_never_fix_text_to_white() {
        const OWN_SURFACE: [&str; 4] = [
            ".dishy-sky-panel",
            ".dishy-sky-callout",
            ".dishy-notification-badge",
            ".dishy-sky-clear",
        ];
        let light = |value: &str| {
            let value = value.trim().trim_end_matches(';').to_ascii_lowercase();
            let near_white_hex = value.strip_prefix('#').is_some_and(|hex| {
                let channels: Vec<u8> = match hex.len() {
                    3 => hex
                        .chars()
                        .filter_map(|c| u8::from_str_radix(&c.to_string().repeat(2), 16).ok())
                        .collect(),
                    6 => (0..6)
                        .step_by(2)
                        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                        .collect(),
                    _ => Vec::new(),
                };
                channels.len() == 3 && channels.iter().all(|&channel| channel >= 0xe0)
            });
            value.starts_with("rgba(255, 255, 255") || near_white_hex || value == "white"
        };
        let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&source_dir).expect("src directory") {
            let path = entry.expect("source entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source text");
            for rule in text.split('}') {
                let Some((selector, body)) = rule.rsplit_once('{') else {
                    continue;
                };
                let selector = selector.lines().last().unwrap_or("").trim();
                if !selector.starts_with('.') || OWN_SURFACE.contains(&selector) {
                    continue;
                }
                for declaration in body.split(';') {
                    if let Some((property, value)) = declaration.split_once(':')
                        && property.trim() == "color"
                        && light(value)
                    {
                        offenders.push(format!(
                            "{}: {selector}",
                            path.file_name().unwrap().to_string_lossy()
                        ));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "fixed white text: {offenders:#?}");
    }
}
