---
status: active
---

# 0070 — Minute buckets

## Purpose

Fold per-second telemetry into per-minute latency, energy, and traffic rows
(`core/latencyBuckets.ts`, `core/energyBuckets.ts`). A day or year of history
must be summarisable without keeping every 1 Hz sample, and the recorder and the
UI must agree on what a minute holds.

## Requirements

- **SPEC-BUCKET-001** — Latency uses 16 bins: 10 ms steps to 100 ms, then 150,
  200, 300, 500, 1000 ms, then an overflow bin for 1000 ms and above.
- **SPEC-BUCKET-002** — A latency minute counts every sample for coverage, but
  only finite readings above zero enter the per-source histogram and sums.
  Drop rate is summed whenever it is finite, including outage seconds with no
  ping.
- **SPEC-BUCKET-003** — Re-seeing the in-progress latency minute replaces its
  row; it never accumulates.
- **SPEC-BUCKET-004** — An energy minute sums power (watt-seconds), downlink
  bits, uplink bits, and sample count. Adding a delta for the same minute adds
  each field exactly.
- **SPEC-BUCKET-005** — Expired minutes fold into calendar months through a
  caller-supplied month boundary, summing histograms, sums, drops, energy, and
  traffic; a later fold merges into the row's existing totals.

## Acceptance criteria

- **001** — Given 9.999 ms, 100 ms, and 1000 ms, when binned, then they land in
  bins 0, 10, and 15.
- **002** — Given the captured window, when folded, then the minute rows match
  the TypeScript output, including router readings and drop sums.
- **003** — Given a base row and a delta for the same minute, when replaced, then
  the result equals the delta.
- **004** — Given the captured window, when energy is folded, then watt-seconds
  and traffic match the TypeScript output; given two partial rows for one
  minute, their addition is field-by-field exact.
- **005** — Given minutes in two months, when folded with that calendar, then two
  rows exist with summed coverage; merging a later fold keeps the earlier
  totals.
