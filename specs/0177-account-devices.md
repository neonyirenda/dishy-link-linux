---
status: complete
---

# 0177 — Account device list

## Purpose

Port the Electron account devices section (`DevicesSection.tsx`,
`deviceList.ts`, and the device helpers in `lib/starlinkCloud.ts`): every dish
and router on the signed-in service line, with each device's fields, from the
account data the cloud worker already fetches.

## Requirements

- **SPEC-ACCTDEV-001** — Each dish is listed, followed by its routers.
  - A dish is "STARLINK" plus the last six hex digits of its terminal id.
  - A router is Main Router, MESH (a repeater, or more than zero hops), or
    Router (no telemetry), plus its id's last twelve hex digits with leading
    zeros dropped.
  - Inactive dishes and their routers move to the end.
- **SPEC-ACCTDEV-002** — Status is "online" when telemetry is under 5 minutes
  old or the device answers on this LAN. A dish not connected for 30 days is
  "inactive", and so are its routers. Anything else is "offline".
- **SPEC-ACCTDEV-003** — A dish shows its Starlink ID, serial number, kit
  number, software version, uptime, last updated, time obstructed, and last
  connected.
- **SPEC-ACCTDEV-004** — A router shows its router ID, hardware name ("v3" →
  "Starlink Router 3"), software version, clients, uptime, connection (Direct,
  or Mesh with its hop count), bypassed, and last updated. A missing value
  reads "—".

## Acceptance criteria

- GTK tests cover names, statuses, grouping, and both field lists from a
  recorded account payload.
