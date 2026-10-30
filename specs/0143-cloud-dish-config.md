---
status: active
---

# 0143 — Cloud dish configuration writes

## Purpose

Port the dish settings that current firmware accepts only through the
authenticated Starlink device gateway. The host chooses the target from fresh
account telemetry and sends only the setting fields explicitly changed by the
user.

## Requirements

- **SPEC-DISHCFG-001** — A typed update can change snow-melt mode, location
  requests, level mode, power-save schedule/mode, update reboot hour, or
  three-day deferral. Each changed field carries its matching apply flag;
  omitted fields do not appear in the request.
- **SPEC-DISHCFG-002** — Empty or out-of-range updates are rejected before
  account/network access; update reboot hour is limited to 3, 9, 15, or 21.
- **SPEC-DISHCFG-003** — The dish target is selected from authenticated account
  telemetry, not caller-supplied IDs; gateway writes are one-shot and use the
  same trusted schema/frame path as router controls.
- **SPEC-DISHCFG-004** — GTK exposes snow melt, sleep schedule, update window,
  and three-day deferral controls. Each gesture sends typed intent to the
  account worker, disables the controls while pending, and reports the write
  result; the GTK page never creates a network client.
- **SPEC-DISHCFG-005** — While Settings is visible, the recorder reads dish
  configuration on a one-minute cadence and supplies it to GTK. Controls remain
  disabled until a real LAN reply arrives, and selectors reflect that reply
  instead of assuming defaults.

## Acceptance criteria

- Unit tests assert exact apply-flag payloads, partial updates, and invalid
  values.
- Fake transport tests assert fresh dish target selection and schema framing.
- The GTK shell test selects a snow-melt mode and verifies an exact typed
  partial update reaches the host action channel.
- A GTK test supplies a dish config snapshot before enabling the control path.
- Traceability and the full workspace test/clippy/format gates pass.
