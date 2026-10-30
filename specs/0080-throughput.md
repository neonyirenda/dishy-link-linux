---
status: active
---

# 0080 — Throughput

## Purpose

Turn the router's per-device byte counters into exact throughput
(`core/throughputTracker.ts`). The router's own 15 s and 1 min averages lag a
speed test; subtracting counters at the moments the router updates them does
not.

## Requirements

- **SPEC-THROUGHPUT-001** — A device's first reading seeds its counters and
  returns the router's published average as a placeholder.
- **SPEC-THROUGHPUT-002** — When a counter moves, the delta is one refresh
  interval per observed poll gap (at least one), divided by 1005 ms per interval
  rather than the measured gap. The result is the device's current rate.
- **SPEC-THROUGHPUT-003** — A counter that goes backwards, or a polling gap over
  15 s, reseeds and returns the published average; no rate is invented across
  an unobserved stretch.
- **SPEC-THROUGHPUT-004** — A counter unchanged for two refresh periods reports
  zero, so an idle device does not hold its last rate forever.
- **SPEC-THROUGHPUT-005** — Between edges the last measured rate is held; absent
  counters return the published average. State for departed devices can be
  dropped.

## Acceptance criteria

- **001** — Given a first poll with fallback 1/2 Mbps, when rates are read, then
  the result is 1/2 Mbps.
- **002** — Given 1000 bytes over a two-interval poll gap, when rates are read,
  then downlink is `1000 * 8 / 1e6 / 2.01` Mbps.
- **003** — Given a poll gap of 15,001 ms, when rates are read, then the result
  is the fallback.
- **004** — Given an unchanged counter at least 2010 ms after its last change,
  when rates are read, then both rates are zero.
- **005** — Given the captured eleven-poll sequence, when replayed, then every
  rate and fallback matches the TypeScript tracker; given a missing counter,
  the fallback is returned.
