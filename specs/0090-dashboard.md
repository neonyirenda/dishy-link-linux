---
status: active
---

# 0090 — Dashboard

## Purpose

The first screen: what the link is doing right now, and for the last fifteen
minutes, hour, or six hours. It ports `DashboardView` from the Electron
renderer to native GTK4 widgets.

The dashboard is a _reader_ of data the recorder already collects. It adds no
poll against the dish or router: the recorder's existing history and router
status cycle feeds it, exactly as it feeds the tray throughput readout.

## Requirements

- **SPEC-DASH-001** — The dashboard shows six stat tiles — download, upload,
  latency, power draw, ping success, and sky obstructed — with translated
  labels and units. Values come from the recorder's samples; a metric that only
  a status reply can supply reads as an em dash rather than zero.
- **SPEC-DASH-002** — Throughput, latency, and power charts draw the last 15
  minutes, 1 hour, or 6 hours. Changing the range re-cuts every chart but does
  not change the live stat tiles.
- **SPEC-DASH-003** — Chart buckets use the same reductions as the Electron
  charts: throughput is averaged, latency takes the maximum, and power is
  averaged. A gap longer than a chart bucket is drawn as a break, never as an
  interpolated line.
- **SPEC-DASH-004** — Before the recorder has any samples, the dashboard shows
  the searching hero. The first sample replaces it with the tiles and charts.
- **SPEC-DASH-005** — A feed with no sample for six seconds shows an offline
  banner. A minute with no ping readings reports 0% success, never 100%.
- **SPEC-DASH-006** — Formatting follows the shared TypeScript thresholds and
  wording: Gbps/Mbps/kbps, whole watts, translated range labels, and no
  user-facing literal in Rust.
- **SPEC-DASH-007** — The six statistic tiles use six columns on wide windows
  or three columns on narrower windows; intermediate column counts are not
  allowed.

## Acceptance criteria

`dashboard.rs` and `readings.rs` unit tests cover the window cut, bucket
reductions, stale/empty states, and formatting. The GTK integration test builds
the first view without touching the network. A focused layout test verifies the
stat grid is constrained to six or three columns.

## Deferred

The obstruction card and sky-obstruction tile value need a status/obstruction
reply, the terminal card needs device info from status, and the outage log needs
the recorder's event stream. Those arrive with the sky view, network, and
alerting milestones. This spec deliberately shows an em dash for the
status-only tile rather than inventing a number from history.
