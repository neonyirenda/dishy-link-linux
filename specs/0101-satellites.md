---
status: complete
---

# 0101 — Live constellation

## Purpose

The deferred half of the sky view: the Starlink constellation overhead, the
serving satellite, and the constellation-fed satellite counts. The model is
pure TLE/SGP4 maths in `dishy-sky`; the public catalogue and IP lookups live in
`dishy-core::external`; a GTK worker thread propagates while the sky page is
shown. Nothing here reads the dish or router.

## Requirements

- **SPEC-CONST-001** — Parse three-line element catalogs into valid orbital
  records, retaining valid entries when individual records are malformed and
  reporting rejected records.
- **SPEC-CONST-002** — Propagate records with SGP4 and convert TEME positions
  into observer-local east/north/up azimuth, elevation, and range. Coordinates
  must be finite and use the shared north-zero, clockwise azimuth convention.
- **SPEC-CONST-003** — Choose the highest clear satellite at or above the
  25-degree service floor; if none of those candidates is clear, choose the
  highest satellite above that floor. Return no candidate when none qualifies.
- **SPEC-CONST-004** — The model is pure with respect to device I/O. It accepts
  its observer location and element text as input and never reads the router or
  dish.

- **SPEC-CONST-005** — The catalogue feed fetches only with an observer location
  and while the sky page is shown, one request at a time, off the GTK main
  thread. Failures retry after 5 s, 15 s, 60 s, then every 300 s, and name the
  failing side (offline vs. source). A catalogue is reused for 6 h and stays in
  use while a refetch fails.
- **SPEC-CONST-006** — The observer location comes only from pasted coordinates
  (hemisphere letters honoured) or a city-level IP lookup, never from a device
  read. A deliberate clear is persisted apart from "never set", and the stored
  shape uses the TypeScript field names.
- **SPEC-CONST-007** — The 30-minute forecast is the smallest count of
  satellites at or above the service floor at 5, 10, … 30 minutes ahead, swept
  over the whole catalogue at most once a minute. Look angles carry orbital
  speed.
- **SPEC-CONST-008** — The serving guess is held for its 15 s handoff slot while
  it stays above the floor and clear; a replacement inherits the slot. "Clear"
  reads the dish's raw obstruction grid with the Electron rule (≤ 5 % unusable,
  unmeasured counts as clear, outside the mapped cone does not).
- **SPEC-CONST-009** — Satellites are drawn on their true bearing on a shell
  outside the dome, the serving one in amber, and a tap within 26 px selects the
  nearest for a callout (elevation, azimuth, altitude, distance, speed).

## Acceptance criteria

- `dishy-sky` tests cover catalog parsing/rejection, known SGP4 propagation,
  ENU azimuth/elevation, service floor, clear-candidate fallback, feed backoff,
  forecast, serving hold, and shell placement.
- `dishy-core` tests cover the catalogue and IP fetches against a mock server.
- On the Starlink LAN: with a saved location the sky shows the constellation and
  a serving marker; clearing it hides them; the recorder log shows no new
  device request.
- The workspace traceability test requires a citing test for each active
  requirement.
- Workspace formatting, tests, and clippy pass.

## Deferred further

None. Everything once deferred here has landed: device location (spec 0180),
the time-lapse scrubber (spec 0179), satellite trails (spec 0182), per-kit dish
meshes (spec 0183), and the 3-D satellite model (spec 0184).
