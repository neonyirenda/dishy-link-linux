---
status: active
---

# 0167 — Alignment panel

## Purpose

Port the alignment panel: SpaceX's own verdict on whether the dish points
inside its five-degree service cone, the two instruments that draw the dials,
and the facts that explain them. The math is the dish web app's algorithm,
ported 1:1 (`alignmentMath.ts`), and the panel consumes only the status reply
the recorder already polls.

## Requirements

- **SPEC-ALIGN-001** — The alignment reading is a pure function of a status
  reply. It performs no device read and no write, and it is the only source the
  panel uses for its figures.
- **SPEC-ALIGN-002** — The aligned judgment ports their `Nd`: great-circle
  separation under 5° against a target band, the floor of the band being
  `min(70, desired elevation)` with an absent or zero desired elevation
  meaning "not sent" and falling back to 70, the ceiling being 90° for a MOBILE
  install or a kit whose default plate tilt is under 8° and 75° otherwise. A
  dish may also read aligned while riding inside the band with the azimuth
  error projected onto the sky at the current elevation under the limit, and
  nothing is deemed aligned before the attitude filter has produced a value
  (`FILTER_CONVERGED` or `FILTER_UNCONVERGED`).
- **SPEC-ALIGN-003** — The kit ceiling comes from the prefix-based model
  resolution table (`lib/dishMesh.ts` `resolveDishModel`): hardware string
  plus the top-level actuator flag, ordered so `rev4_hp…` is not swallowed by
  `rev4`, with unknown hardware taking the Standard's 20° default tilt and
  falling into the 75° band.
- **SPEC-ALIGN-004** — The reported figures are the dish's own, not band
  arithmetic: boresight error is the great-circle angle to the *unclamped*
  desired elevation, the per-axis errors are signed current-minus-target with
  azimuth wrapped to (-180, 180], and the dial wedge limits are the band
  bracketed with 5° of slack, never outside 0–90.
- **SPEC-ALIGN-005** — The instruments draw the dials: the rotation dial is a
  dotted 5° ring with gaps around the cardinals, compass letters north-up, a
  tolerance wedge centered on the desired azimuth, and a dish pointer plus
  orange needle at the *actual* azimuth; the tilt dial is a y-flipped quarter
  arc from 0 to 90° with the wedge spanning the valid elevation band and the
  needle at the actual elevation. Each dial carries a health berry: green when
  in spec, red when the filter is valid and it is out, grey until the filter
  says anything.
- **SPEC-ALIGN-006** — The panel presents the verdict line (stale — with the
  time the last status was seen — or aligned / not aligned / not ready),
  the two instrument cards, the fact columns with their hints, the explainer,
  and a link that opens the satellite view. A cold start with no status shows
  the no-reading callout. All text comes from the shared translation catalog.
- **SPEC-ALIGN-007** — The page never polls, never writes. It reads the
  recorder's cached status, and staleness derives from when that status last
  arrived; opening the page, updating it, and refreshing do not send recorder
  commands or device messages.

## Acceptance criteria

- `dishy-core` unit tests port the separation, tolerance, wrap, band, kit
  ceiling, and unknown-kit cases from `alignmentMath.test.ts` and cite 001–004.
- The GTK integration test builds the page, checks the no-reading state, and
  checks the verdict and figures come from a cached status without any
  recorder command being sent.
- Workspace formatting, tests, and clippy pass; every active ID has a citing
  test.
