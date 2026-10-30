---
status: complete
---

# 0188 — Starlink line icons

## Purpose

Replace the generic Adwaita glyphs (a moon for the sky, a wifi arc in the tray)
with Electron's thin-line icon set (`src/assets/icons`), so the app reads as
Starlink rather than stock GNOME.

## Requirements

- **SPEC-ICON-001** — The navigation uses Electron's glyphs:
  - speedometer for Speed test, crosshair for Alignment, bars for Data usage,
    the wifi arcs for Network, user for Account, planet for the Sky view, and
    the gear for Settings;
  - Dashboard, which Electron reaches through its logo, gets a matching
    four-tile glyph.

  The header bell uses Electron's bell.

- **SPEC-ICON-002** — Each glyph is drawn from its SVG path data, filled or
  stroked as Electron draws it, in the widget's own foreground colour, so it
  follows the theme and stays crisp at any scale. Every path parses.
- **SPEC-ICON-003** — The tray shows a symbolic Dishylink mark instead of the
  wifi glyph. It is the logo's orbits, satellite, and receiver drawn as fills,
  so the panel recolours it. The app writes it to its cache directory and
  points the StatusNotifier icon theme path there, which works in dev runs and
  inside the Flatpak.

## Acceptance criteria

- GTK tests cover each tab's glyph, the parsing of every glyph's paths, and
  the tray's icon name and its written theme file.
