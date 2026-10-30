---
status: complete
---

# 0175 — Device and node detail

## Purpose

Port the Electron device and node drill-down (`DeviceDetail.tsx`,
`deviceFacts.ts`, `DeviceThroughput.tsx`, `NodeDetail.tsx`,
`collector/clientWindow.mts`). The per-device throughput history is kept from
the rates the demand-gated roster reads already compute, so no poll is added.

## Requirements

- **SPEC-NETDETAIL-001** — Each roster read adds a sample per device to a
  window kept for 6 hours: key, MAC, time, and down/up Mbps rounded to three
  decimals. Samples older than the window are dropped, along with devices that
  have none left. The last 30 minutes persist to `client-samples.json` in the
  TypeScript shape and are restored on start.
- **SPEC-NETDETAIL-002** — The device detail lists, in the Electron order:
  - status (active, or idle for a duration), role, connected-to node,
    manufacturer, connection;
  - signal with its quality, SNR, channel width, MCS, spatial streams;
  - link rx/tx rates, IPv4, the first IPv6 address, MAC;
  - this month's data usage, or this connection's.

  A fact the router did not send is left out.

- **SPEC-NETDETAIL-003** — The device detail charts that device's down and up
  throughput over 15 min, 1 h, or 6 h. A gap longer than 150 s breaks the line.
  An empty window says nothing was recorded over that span.
- **SPEC-NETDETAIL-004** — The node detail lists role, signal, link rate,
  connection, interface, MAC, device id, IP, and hardware, then the devices
  connected through it. A paired node that is down says so.
- **SPEC-NETDETAIL-005** — Device rows and node tiles open their detail. The
  detail offers the device's rename and its pause, behind the spec 0170 gate.

## Acceptance criteria

- Core tests cover window ingest, rounding, expiry, and persistence.
- GTK tests cover the fact list, chart series, empty note, node facts, and
  that rows open details without a new router read.
