---
status: complete
---

# 0190 — Dark glass and the Starlink typeface

## Purpose

Give the GTK app the Starlink website's dark look instead of stock Adwaita.
It follows the in-window glass of Amberol and Tambourine: a soft, tinted
backdrop behind translucent cards. When a compositor blur such as Blur my
Shell is running, the window can also become see-through glass. The app
bundles Electron's typefaces.

## Requirements

- **SPEC-GLASS-001** — The app bundles Barlow (400, 500, 600, 700) and IBM
  Plex Mono (400, 500, 600), both SIL OFL, with their licences.
  - At startup it writes them once to its cache directory and registers them
    with Pango.
  - The interface is set in Barlow, and figures marked monospace in Plex
    Mono.
- **SPEC-GLASS-002** — In dark mode the window is dark glass:
  - a backdrop draws a black base, an earth-limb horizon glow, faint stars,
    and Electron's dot grid;
  - the header, pages, and scrolled areas are transparent over it;
  - cards are translucent (4.5% white) with a 1 px 8% white hairline and a
    12 px radius.

  Light mode keeps Adwaita light.

- **SPEC-GLASS-003** — The glow is tinted by the link:
  - a cool blue while online with no warning;
  - amber while online with an active warning or critical alert;
  - red while the devices are unreachable.

  The tint cross-fades over about 600 ms.

- **SPEC-GLASS-004** — A "See-through glass" setting, off by default, thins
  the backdrop's base so a compositor blur behind the window shows through.
  It is off by default because without a blur the desktop shows sharp behind
  the window. The choice persists as `seeThroughGlass`.
- **SPEC-GLASS-005** — The theme CSS loads without parser errors.
- **SPEC-GLASS-006** — Text stays legible in light mode. View stylesheets
  never fix text to white; muted text is a translucent `currentColor`, so it
  follows the theme on cards and stays light inside the sky view's dark
  panels, which set their own light colour. The only fixed light text colours
  belong to elements that paint their own dark or coloured background (the sky
  panel, the notification badge) or draw a swatch (the dome's "clear" key).

## Acceptance criteria

- GTK tests cover font registration, the window's glass class following the
  colour scheme, the tint for each link state, the see-through preference
  and its effect on the backdrop, a clean CSS parse, and no fixed white text
  outside the allowed dark surfaces.
