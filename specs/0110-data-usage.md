---
status: active
---

# 0110 — Local data usage

## Purpose

The self-measured traffic page: the dish's own throughput telemetry, folded
into per-minute volume and summarised over the same ranges the Electron app
offers. This is not Starlink's billing meter; it counts what this recorder saw,
and it says so when coverage is incomplete.

The page adds no poll. It reads the per-minute traffic the history recorder
already writes and the in-progress minute it holds in memory.

## Requirements

- **SPEC-USAGE-001** — A usage summary sums `downlinkBits` and `uplinkBits` from
  the recorder's per-minute buckets. One gigabyte is 8×10⁹ bits, matching the
  TypeScript summary, and coverage is `sampledSeconds / expectedSeconds` capped
  at one.
- **SPEC-USAGE-002** — The page offers `1h`, `6h`, `12h`, `today`, `day`,
  `week`, and `month`, with the same windows and bar grouping as
  `core/energySummary.ts`: 5-minute, 30-minute, hourly, calendar-day,
  calendar-week, and calendar-month bars, using local midnight/Monday/January
  boundaries.
- **SPEC-USAGE-003** — A slot with no recorded minutes is absent, not zero: its
  down/up values are null and no bar is drawn for it. Slots at the edges are
  clipped to the requested window.
- **SPEC-USAGE-004** — The GTK page reads summaries through the recorder's
  command channel, so only the one recorder process touches the stores. It
  refreshes on open, on range change, and at most every 30 seconds while the
  page is visible.
- **SPEC-USAGE-005** — The `1h` through `month` range selector is centered
  within the page rather than pinned to its left edge.

## Acceptance criteria

A TypeScript-generated fixture covers every range over a synthetic minute
series with gaps. `dishy-core` tests assert the totals, coverage, slot keys, and
null slots against it. The GTK page has a pure formatting test and is wired into
the page stack.

## Related

- Per-device usage totals and the device list are specified in `0111-device-usage.md`.

## Deferred

- The cloud billing source is part of the account work, never a local estimate (M9).
