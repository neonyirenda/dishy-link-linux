---
status: verified
---

# 0166 — Native toolbar style

## Purpose

Expose the shared App-level Dock/Rail navigation preference in GTK and switch
the shell layout live without rebuilding or fetching device data.

## Requirements

- **SPEC-TOOLBAR-001** — App settings offers localized Dock and Rail choices;
  Dock is the default.
- **SPEC-TOOLBAR-002** — The selected value is persisted in host preferences as
  `toolbarStyle` (`dock` or `rail`), and invalid values fall back to Dock.
- **SPEC-TOOLBAR-003** — Dock is a floating, icon-only capsule centered at the
  bottom of the window. It uses the Electron app's destination iconography,
  tooltips, and an active-page indicator.
- **SPEC-TOOLBAR-004** — Rail is a floating capsule centered vertically at the
  left edge. It starts icon-only and expands to show translated labels while
  hovered. Both layouts navigate the same stack and reflect its active page.
- **SPEC-TOOLBAR-005** — Switching layouts preserves the selected page and
  changes only local presentation; it issues no router/dish reads or writes.
- **SPEC-TOOLBAR-006** — The header theme control shows a moon for Dark, a sun
  for Light, and an `A` for Automatic. The adjacent header status uses one
  red/green connectivity indicator plus country and uptime from already fetched
  replies; it shows no placeholder and starts no poll.
- **SPEC-TOOLBAR-007** — Navigation icons use available symbolic assets. Rail
  icons remain horizontally centered, equidistant from both borders, while
  collapsed and align with labels while expanded; all icons remain vertically
  centered in either state. Dock and rail surfaces use theme colors in both
  light and dark modes without a bright shadow halo.
- **SPEC-TOOLBAR-008** — The bottom dock reserves page space while visible so it
  cannot cover content or prevent scrolling to the end of a page. Rail mode
  removes that clearance.
- **SPEC-TOOLBAR-009** — The single connectivity indicator is green only when
  cached dish and router reachability are both positive; otherwise it is red.
  It adds no device requests.
- **SPEC-TOOLBAR-010** — The dock floats over the content, as in Electron.
  The content area runs to the bottom of the window, so the sky view fills it.
  Scrolling pages pad their end by 96 px so their last row clears the dock,
  and the sky's info panel lifts 96 px above the bottom edge.

## Acceptance criteria

- Preference tests cover default, valid round-trip, and invalid-value fallback.
- GTK shell tests cover icon/tooltips, active-page state, rail alignment and
  expansion, dock clearance, and selected-page preservation when switching
  layouts.
- The settings test verifies selection intent and silent state seeding.
- The GTK shell test covers header theme/status controls and icon availability.
- Workspace tests, formatting, catalog coverage, and spec traceability pass.
