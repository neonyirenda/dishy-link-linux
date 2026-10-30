---
status: superseded
---

# 0160 — Per-device monthly data limits

> **Superseded by [0169](0169-meter-rules.md).** A monthly limit is now a
> device's own monthly rule. Behaviour below is preserved: limits migrate into
> rules and keep their alerts.

## Purpose

The first native network-metering slice is a local, watch-only monthly cap on
one device. It provides clear usage feedback without claiming to enforce a
Starlink router block. The TypeScript app's wider rule system—groups, pooled
allowances, schedules, countdowns, arbitrary cycles, and pause enforcement—is
out of scope for this specification.

## Requirements

- **SPEC-METER-001** — A monthly limit is keyed by the same stable client key as
  the device-usage odometer and stores one positive byte cap per device. Invalid,
  zero, or overflowing limits are refused without changing stored state.
- **SPEC-METER-002** — Progress is current-month received plus transmitted bytes
  from the existing odometer. Reaching or exceeding the cap is displayed as
  reached. Saving a cap applies it to the whole current month; the next local
  calendar month starts its progress at zero.
- **SPEC-METER-003** — Limits survive recorder restart in a versioned snapshot
  written by atomic temporary-file replacement. Removing a device usage record
  removes its limit; clearing all device records clears all limits.
- **SPEC-METER-004** — Data Usage exposes a translated edit/remove control per
  device and shows its monthly progress and reached state. User changes travel
  through the recorder command channel; storage failures are reported rather
  than shown as successful changes.
- **SPEC-METER-005** — Evaluation reuses the existing demand-gated client roster
  and odometer data. This slice adds no router or dish reads and never pauses,
  blocks, or otherwise writes to a device.

## Acceptance criteria

- Core tests cover valid and invalid limits, current-month progress and reached
  state, atomic persistence/restart, and cleanup with device-record removal.
- GTK tests cover translated limit controls, display state, and typed recorder
  command dispatch.
- Rust spec traceability, workspace tests, clippy, formatting, and changed-file
  formatting pass.
