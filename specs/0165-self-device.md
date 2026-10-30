---
status: verified
---

# 0165 — Native self-device identity

## Purpose

Let the user identify the computer running Dishylink in the router roster,
using the same locally persisted `selfClientId` preference as the shared host
settings.

## Requirements

- **SPEC-SELF-001** — The App settings page offers None and the current client
  devices from the latest cached network snapshot. It performs no network read.
- **SPEC-SELF-002** — Selection is persisted in the host preferences JSON as
  `selfClientId`; clearing stores null. Existing preference validation remains
  independent.
- **SPEC-SELF-003** — The selected device is marked “This device” in the
  Network page. A saved ID absent from the current roster remains selected and
  is shown as not connected; an empty/unavailable roster does not clear it.
- **SPEC-SELF-004** — Building settings, refreshing cached snapshots, and
  selecting/clearing an ID do not issue router/dish calls or add pause/write
  controls.

## Acceptance criteria

- Preference tests cover persisted IDs and clear behavior.
- GTK tests cover an empty roster, a selected missing ID, options from a cached
  roster, selection/clear action intent, and the marked network row.
- Tests assert that applying the cached data does not send recorder commands.
- All requirements are cited by implementation/tests; workspace checks pass.
