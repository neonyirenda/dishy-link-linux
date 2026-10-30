---
status: complete
---

# 0181 — Speed-test beam view

## Purpose

Port Electron's "Starlink" speed-test view (`SpeedBeam.tsx`, `BeamScene.tsx`,
`lib/dishPngArt.ts`). It is an illustrated scene: the user's own kit on its
ground rings, two satellites crossing a dashed orbit, and a beam carrying a
data packet. It replaces the deferral noted in spec 0168.

## Requirements

- **SPEC-BEAM-001** — The speed-test page has a view switch, "Starlink" then
  "Gauge", labelled with the catalog's view label. It opens on Starlink, as
  Electron does, and is disabled while a run is in progress.
- **SPEC-BEAM-002** — The scene uses Electron's 260 × 200 geometry:
  - ground rings of horizontal radii 26, 50, 76, 104, and 134, flattened to
    0.24, centred where the dish stands;
  - an orbit drawn as a quadratic arc through (−6, 42), (130, −16), and
    (266, 42);
  - two satellites half a 13 s loop apart, travelling t from −0.35 to 1.35;
  - the beam goes to whichever satellite sits more centrally on the visible
    arc. That satellite is fully lit and the other sits at 0.4.
- **SPEC-BEAM-003** — The beam and packet:
  - the beam is lit while a test runs or the mode is not idle;
  - while a test runs it swells between 0.64 and 1, and a packet runs along it
    every 2.6 s with a trailing streak of 0.16 of the beam. The packet travels
    down to the dish during download and up during upload;
  - when not lit, the beam and packet are hidden.
- **SPEC-BEAM-004** — The dish is the user's kit, using Electron's per-model
  art. The art sits on the rings by its ground anchor, and the beam starts at
  its exit anchor. An unknown model draws the Standard kit.
- **SPEC-BEAM-005** — Below the scene, the reading shows with one decimal next
  to "Mbps", the same eased value and pending rule as the gauge.

## Acceptance criteria

- GTK tests cover the arc, the handoff between satellites, the swell and
  packet direction, the art anchors, and the view switch.
