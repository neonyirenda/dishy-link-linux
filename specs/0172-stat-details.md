---
status: complete
---

# 0172 — Dashboard tile details

## Purpose

Port the Electron drill-down a dashboard tile opens (`StatDetailPanel`,
`LatencyDetailPanel`, `LatencyQualityPanel`, `LatencyHistogram`,
`EnergyHistoryPanel`, `lib/statDetails.ts`, `core/latencySummary.ts`). Every
figure comes from samples and minute stores the recorder already holds; opening
a detail adds no dish or router request.

## Requirements

- **SPEC-DETAIL-001** — Clicking the download, upload, latency, power, or ping
  success tile opens its detail. The detail shows:
  - the average over the chosen window (15 min, 1 h, or 6 h) next to the
    current value, in that metric's format;
  - the window's chart;
  - the catalog's explainer for that metric.

  The latency detail overlays the dish line and the router line.

- **SPEC-DETAIL-002** — Averages count only finite readings, and a window with
  none averages to zero. Window coverage sums only steps of 5 s or less: a
  window with no readings is "nothing recorded", 95% or more is "full", and
  anything else reports the minutes covered.
- **SPEC-DETAIL-003** — The latency detail shows a distribution per series:
  2 ms bins from 0 to 100 ms plus one overflow bin, each as a percentage of
  that series' readings. A series with no readings is left out.
- **SPEC-DETAIL-004** — The ping-success detail adds a router chart from
  `popPingDropRate5m`, or, when the window holds no router readings, the
  catalog's note saying why.
- **SPEC-DETAIL-005** — The power detail shows the energy used over the window
  and the day/week/month energy history.
  - Both come from the recorder's energy summaries for 1 h and 6 h windows,
    falling back to the live samples.
  - The coverage note matches the Electron wording.
- **SPEC-DETAIL-006** — The latency detail's Quality view summarises a range
  (1 h to month) from the latency minute store:
  - interpolated p50/p95/p99, jitter (standard deviation), and spread
    (p99 − p50) for the dish, and drop % from the drop sums;
  - a 0–100 score weighting latency 40, jitter 30, tail 20, and loss 10, and
    an A–F grade;
  - per-bar p95, plus coverage against the range.

## Acceptance criteria

- Core tests port the percentile, score, grade, and range-summary cases.
- GTK tests cover averages, coverage wording, histogram bins, detail copy, and
  that opening a detail sends only recorder-local summary commands.
