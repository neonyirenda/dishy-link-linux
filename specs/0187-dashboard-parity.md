---
status: complete
---

# 0187 — Dashboard parity with Electron

## Purpose

Bring the GTK dashboard up to Electron's main page (`DashboardView.tsx`,
`ObstructionCard.tsx`, `OutageLog.tsx`, `TopBar.tsx`). GTK showed only the
tiles, three charts, and the terminal card. Electron also has an Obstructions
card, an Events & outages card, a two-column layout, and a branded top bar.
Every figure comes from replies and recorder stores already in hand, so the
dashboard adds no dish or router request.

## Requirements

- **SPEC-DASHP-001** — The layout follows Electron's breakpoint at 1080 px:
  - when wide, six tiles sit in one row, then a left column (throughput,
    latency, power) beside a right column (Obstructions, then Events &
    outages), then the terminal card across the full width;
  - when narrow, three tiles per row, with the cards stacked in that order;
  - the content is capped at 1400 px.

  The tile columns follow the breakpoint itself, not a width notification
  that never fires.

- **SPEC-DASHP-002** — Each tile that opens a detail shows a trailing `›`.
  Sky obstructed, which opens nothing, shows none.
- **SPEC-DASHP-003** — The Obstructions card draws the live obstruction dome
  with the same survey as the Sky page, without the constellation:
  - its header has pause/resume rotation, show/hide unmapped, and "Live
    satellite view ›", which opens the Sky page;
  - below the dome are the key, the stats (sky obstructed, observed for), and a
    note: the clear-view note under 0.5% obstructed, otherwise the obstructed
    note;
  - until a map arrives it shows the waiting message.
- **SPEC-DASHP-004** — The Events & outages card lists the recorder's events
  newest first. Each row shows the start time, a kind mark (outage, degraded,
  or info), the title, and the duration, with "N events" in the header and the
  empty message when there are none. Activating a row opens the event detail.
  Events are read from the recorder when the dashboard is shown. This is a
  local read.
- **SPEC-DASHP-005** — The header carries the Dishylink logo and wordmark on
  the left. On the right, the status reads "online", "connecting", or "dish
  unreachable", then the country and the uptime, separated by " · ", followed
  by the alerts bell and the theme toggle.

## Acceptance criteria

- GTK tests cover the layout at both breakpoints, the tile chevrons, the
  obstruction note threshold and the open-sky action, the event rows and
  count, and the header status text.
- Hardware: the card's dome follows the dish like the Sky page's, and the
  events match the Alerts popover's.
