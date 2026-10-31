//! Electron's thin-line icons (spec 0188), drawn from their SVG path data in
//! the widget's own colour so they follow the theme and stay crisp.

use gtk::prelude::*;

/// How a path is painted: filled (the 256-unit outline glyphs) or stroked at a
/// width in the glyph's own units (the 24-unit line glyphs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Fill,
    Stroke(f64),
}

#[derive(Debug)]
pub struct Glyph {
    pub name: &'static str,
    /// The side of the square viewBox the paths are drawn in.
    pub size: f64,
    pub parts: &'static [(Paint, &'static str)],
}

pub const SPEEDOMETER: Glyph = Glyph {
    name: "speedometer",
    size: 24.0,
    parts: &[
        (Paint::Stroke(2.0), "m12 14 4-4"),
        (Paint::Stroke(1.5), "M3.34 19a10 10 0 1 1 17.32 0"),
    ],
};

pub const CROSSHAIR: Glyph = Glyph {
    name: "crosshair",
    size: 256.0,
    parts: &[(
        Paint::Fill,
        "M128,24A104,104,0,1,0,232,128,104.11,104.11,0,0,0,128,24Zm8,191.63V184a8,8,0,0,0-16,0v31.63A88.13,88.13,0,0,1,40.37,136H72a8,8,0,0,0,0-16H40.37A88.13,88.13,0,0,1,120,40.37V72a8,8,0,0,0,16,0V40.37A88.13,88.13,0,0,1,215.63,120H184a8,8,0,0,0,0,16h31.63A88.13,88.13,0,0,1,136,215.63Z",
    )],
};

/// Three rounded bars; Electron strokes them at 14 in a 247-unit box.
pub const CHART: Glyph = Glyph {
    name: "chart",
    size: 247.0,
    parts: &[(
        Paint::Stroke(14.0),
        "M211.4936 7H211.4936A27.5576 27.5576 0 0 1 239.0512 34.5576V206.8324A27.5576 27.5576 0 0 1 211.4936 234.39H211.4936A27.5576 27.5576 0 0 1 183.936 206.8324V34.5576A27.5576 27.5576 0 0 1 211.4936 7ZM34.5576 79.8018H34.5576A27.5576 27.5576 0 0 1 62.1152 107.3594V206.8322A27.5576 27.5576 0 0 1 34.5576 234.3898H34.5576A27.5576 27.5576 0 0 1 7 206.8322V107.3594A27.5576 27.5576 0 0 1 34.5576 79.8018ZM123.0254 124.088H123.0254A27.5576 27.5576 0 0 1 150.583 151.6456V206.8324A27.5576 27.5576 0 0 1 123.0254 234.39H123.0254A27.5576 27.5576 0 0 1 95.4678 206.8324V151.6456A27.5576 27.5576 0 0 1 123.0254 124.088Z",
    )],
};

pub const NETWORK: Glyph = Glyph {
    name: "network",
    size: 256.0,
    parts: &[(
        Paint::Fill,
        "M237.08,87A172,172,0,0,0,18.92,87,8,8,0,0,0,29.08,99.37a156,156,0,0,1,197.84,0A8,8,0,0,0,237.08,87ZM205,122.77a124,124,0,0,0-153.94,0A8,8,0,0,0,61,135.31a108,108,0,0,1,134.06,0,8,8,0,0,0,11.24-1.3A8,8,0,0,0,205,122.77Zm-32.26,35.76a76.05,76.05,0,0,0-89.42,0,8,8,0,0,0,9.42,12.94,60,60,0,0,1,70.58,0,8,8,0,1,0,9.42-12.94Z",
    )],
};

pub const USER: Glyph = Glyph {
    name: "user",
    size: 256.0,
    parts: &[(
        Paint::Fill,
        "M230.92,212c-15.23-26.33-38.7-45.21-66.09-54.16a72,72,0,1,0-73.66,0C63.78,166.78,40.31,185.66,25.08,212a8,8,0,1,0,13.85,8c18.84-32.56,52.14-52,89.07-52s70.23,19.44,89.07,52a8,8,0,1,0,13.85-8ZM72,96a56,56,0,1,1,56,56A56.06,56.06,0,0,1,72,96Z",
    )],
};

pub const PLANET: Glyph = Glyph {
    name: "planet",
    size: 256.0,
    parts: &[(
        Paint::Fill,
        "M245.11,60.68c-7.65-13.19-27.84-16.16-58.5-8.66A95.93,95.93,0,0,0,32,128a98,98,0,0,0,.78,12.31C5.09,169,5.49,186,10.9,195.32,16,204.16,26.64,208,40.64,208a124.11,124.11,0,0,0,28.79-4A95.93,95.93,0,0,0,224,128a97.08,97.08,0,0,0-.77-12.25c12.5-13,20.82-25.35,23.65-35.92C248.83,72.51,248.24,66.07,245.11,60.68ZM128,48a80.11,80.11,0,0,1,78,62.2c-17.06,16.06-40.15,32.53-62.07,45.13C116.38,171.14,92.48,181,73.42,186.4A79.94,79.94,0,0,1,128,48ZM24.74,187.29c-1.46-2.51-.65-7.24,2.22-13a79.05,79.05,0,0,1,10.29-15.05,96,96,0,0,0,18,31.32C38,193.46,27.24,191.61,24.74,187.29ZM128,208a79.45,79.45,0,0,1-38.56-9.94,370,370,0,0,0,62.43-28.86c21.58-12.39,40.68-25.82,56.07-39.08A80.07,80.07,0,0,1,128,208ZM231.42,75.69c-1.7,6.31-6.19,13.53-12.63,21.13a95.69,95.69,0,0,0-18-31.35c14.21-2.35,27.37-2.17,30.5,3.24C232.19,70.28,232.24,72.63,231.42,75.69Z",
    )],
};

pub const SETTINGS: Glyph = Glyph {
    name: "settings",
    size: 256.0,
    parts: &[(
        Paint::Fill,
        "M128,80a48,48,0,1,0,48,48A48.05,48.05,0,0,0,128,80Zm0,80a32,32,0,1,1,32-32A32,32,0,0,1,128,160Zm109.94-52.79a8,8,0,0,0-3.89-5.4l-29.83-17-.12-33.62a8,8,0,0,0-2.83-6.08,111.91,111.91,0,0,0-36.72-20.67,8,8,0,0,0-6.46.59L128,41.85,97.88,25a8,8,0,0,0-6.47-.6A112.1,112.1,0,0,0,54.73,45.15a8,8,0,0,0-2.83,6.07l-.15,33.65-29.83,17a8,8,0,0,0-3.89,5.4,106.47,106.47,0,0,0,0,41.56,8,8,0,0,0,3.89,5.4l29.83,17,.12,33.62a8,8,0,0,0,2.83,6.08,111.91,111.91,0,0,0,36.72,20.67,8,8,0,0,0,6.46-.59L128,214.15,158.12,231a7.91,7.91,0,0,0,3.9,1,8.09,8.09,0,0,0,2.57-.42,112.1,112.1,0,0,0,36.68-20.73,8,8,0,0,0,2.83-6.07l.15-33.65,29.83-17a8,8,0,0,0,3.89-5.4A106.47,106.47,0,0,0,237.94,107.21Zm-15,34.91-28.57,16.25a8,8,0,0,0-3,3c-.58,1-1.19,2.06-1.81,3.06a7.94,7.94,0,0,0-1.22,4.21l-.15,32.25a95.89,95.89,0,0,1-25.37,14.3L134,199.13a8,8,0,0,0-3.91-1h-.19c-1.21,0-2.43,0-3.64,0a8.08,8.08,0,0,0-4.1,1l-28.84,16.1A96,96,0,0,1,67.88,201l-.11-32.2a8,8,0,0,0-1.22-4.22c-.62-1-1.23-2-1.8-3.06a8.09,8.09,0,0,0-3-3.06l-28.6-16.29a90.49,90.49,0,0,1,0-28.26L61.67,97.63a8,8,0,0,0,3-3c.58-1,1.19-2.06,1.81-3.06a7.94,7.94,0,0,0,1.22-4.21l.15-32.25a95.89,95.89,0,0,1,25.37-14.3L122,56.87a8,8,0,0,0,4.1,1c1.21,0,2.43,0,3.64,0a8.08,8.08,0,0,0,4.1-1l28.84-16.1A96,96,0,0,1,188.12,55l.11,32.2a8,8,0,0,0,1.22,4.22c.62,1,1.23,2,1.8,3.06a8.09,8.09,0,0,0,3,3.06l28.6,16.29A90.49,90.49,0,0,1,222.9,142.12Z",
    )],
};

pub const BELL: Glyph = Glyph {
    name: "bell",
    size: 24.0,
    parts: &[
        (
            Paint::Stroke(2.0),
            "M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9",
        ),
        (Paint::Stroke(2.0), "M13.7 21a2 2 0 0 1-3.4 0"),
    ],
};

/// Four rounded tiles, for the Dashboard tab Electron reaches by its logo.
pub const DASHBOARD: Glyph = Glyph {
    name: "dashboard",
    size: 24.0,
    parts: &[(
        Paint::Stroke(1.5),
        "M5.5 3.75H9A1.75 1.75 0 0 1 10.75 5.5V9A1.75 1.75 0 0 1 9 10.75H5.5A1.75 1.75 0 0 1 3.75 9V5.5A1.75 1.75 0 0 1 5.5 3.75ZM15 3.75H18.5A1.75 1.75 0 0 1 20.25 5.5V9A1.75 1.75 0 0 1 18.5 10.75H15A1.75 1.75 0 0 1 13.25 9V5.5A1.75 1.75 0 0 1 15 3.75ZM5.5 13.25H9A1.75 1.75 0 0 1 10.75 15V18.5A1.75 1.75 0 0 1 9 20.25H5.5A1.75 1.75 0 0 1 3.75 18.5V15A1.75 1.75 0 0 1 5.5 13.25ZM15 13.25H18.5A1.75 1.75 0 0 1 20.25 15V18.5A1.75 1.75 0 0 1 18.5 20.25H15A1.75 1.75 0 0 1 13.25 18.5V15A1.75 1.75 0 0 1 15 13.25Z",
    )],
};

pub const ALL: [&Glyph; 9] = [
    &SPEEDOMETER,
    &CROSSHAIR,
    &CHART,
    &NETWORK,
    &USER,
    &PLANET,
    &SETTINGS,
    &BELL,
    &DASHBOARD,
];

/// The glyph for a navigation page id.
pub fn for_page(page: &str) -> &'static Glyph {
    match page {
        "satellite" => &PLANET,
        "datausage" => &CHART,
        "network" => &NETWORK,
        "speedtest" => &SPEEDOMETER,
        "alignment" => &CROSSHAIR,
        "account" => &USER,
        "settings" => &SETTINGS,
        _ => &DASHBOARD,
    }
}

/// A square drawing of `glyph`, `pixels` wide, in the widget's colour.
pub fn line_icon(glyph: &'static Glyph, pixels: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(pixels)
        .content_height(pixels)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    area.set_widget_name(&format!("line-icon-{}", glyph.name));
    area.add_css_class("dishy-line-icon");
    // Parsed once; a path that fails to parse (none do, per the tests) is
    // simply skipped rather than taking the icon down.
    let parts: Vec<(Paint, gtk::gsk::Path)> = glyph
        .parts
        .iter()
        .filter_map(|(paint, data)| Some((*paint, gtk::gsk::Path::parse(data).ok()?)))
        .collect();
    let size = glyph.size;
    area.set_draw_func(move |area, cr, width, height| {
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let side = f64::from(width.min(height));
        cr.translate(
            (f64::from(width) - side) / 2.0,
            (f64::from(height) - side) / 2.0,
        );
        cr.scale(side / size, side / size);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        for (paint, path) in &parts {
            cr.new_path();
            path.to_cairo(cr);
            match paint {
                Paint::Fill => {
                    let _ = cr.fill();
                }
                Paint::Stroke(width) => {
                    cr.set_line_width(*width);
                    let _ = cr.stroke();
                }
            }
        }
    });
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-ICON-002: every glyph's paths parse as SVG path data.
    #[test]
    fn every_glyph_parses() {
        for glyph in ALL {
            for (_, data) in glyph.parts {
                assert!(
                    gtk::gsk::Path::parse(data).is_ok(),
                    "{} does not parse",
                    glyph.name
                );
            }
        }
    }

    // SPEC-ICON-001: each page maps to Electron's glyph.
    #[test]
    fn pages_use_electrons_glyphs() {
        for (page, glyph) in [
            ("dashboard", "dashboard"),
            ("satellite", "planet"),
            ("datausage", "chart"),
            ("network", "network"),
            ("speedtest", "speedometer"),
            ("alignment", "crosshair"),
            ("account", "user"),
            ("settings", "settings"),
        ] {
            assert_eq!(for_page(page).name, glyph, "{page}");
        }
    }
}
