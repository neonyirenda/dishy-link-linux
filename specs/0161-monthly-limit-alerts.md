---
status: verified
---

# 0161 — Monthly data-limit alerts

## Purpose

Make the watch-only monthly per-device caps visible in the shared Alerts
surface. Evaluation uses the existing odometer and limit store; it must not
introduce router/dish reads or enforcement writes.

## Requirements

- **SPEC-METER-ALERT-001** — A cap reaching or exceeding current-month RX plus
  TX usage opens one warning episode keyed by stable client key. Repeated
  observations do not create duplicate transitions.
- **SPEC-METER-ALERT-002** — A later observed total below the cap clears the
  episode. Removing the cap also clears it. Missing totals are not interpreted
  as zero and do not clear a firing episode.
- **SPEC-METER-ALERT-003** — Active state and history use the existing alert
  engine/store. The device display name is retained with an episode so active
  and historical entries remain understandable after a restart or record
  deletion. Older alert snapshots without this optional label remain readable.
- **SPEC-METER-ALERT-004** — Reached-cap notifications use the existing
  explicit opt-in preference and one-minute transition-key throttle. Each
  device has an independent key; clearing has a distinct key.
- **SPEC-METER-ALERT-005** — Evaluation runs only when existing demand-gated
  client totals are observed or the user changes a local cap/usage record. It
  adds no device poll, router write, or dish write.

## Acceptance criteria

- Core tests cover threshold edges, repeat suppression, below-cap recovery,
  cap removal, missing-total behavior, and label persistence across restart.
- GTK tests cover translated active/history copy and notification gating for
  the dynamic per-device alert.
- Rust formatting, clippy, workspace tests, changed-file formatting, and spec
  traceability pass.
