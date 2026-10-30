---
status: active
---

# 0120 — Network

## Purpose

The router's own view of the local network: the devices attached to it, the
mesh nodes paired with it, and the radios' temperatures. This is a read-only
port of the Network panel's Devices and Nodes tabs. Watch-only monthly data
limits are covered separately in 0160; naming, pausing, full rules, the cloud
roster fallback details are covered by 0162; naming, pausing, full rules, and
per-device throughput charts stay in later milestones.

The page reuses the router client the recorder already owns. While it is open it
asks for the client roster every 5 s, the Wi-Fi config once when the router first
answers, and radio temperatures every 15 s. It adds no other poll, and it never
calls the dish.

## Requirements

- **SPEC-NET-001** — While the network page is visible, the recorder polls the
  router's client roster at 5 s, re-reads the Wi-Fi config on the first answer
  and after a reconnect, and reads radio temperatures at 15 s. No network poll
  happens while the page is closed, and the page itself never opens a client.
- **SPEC-NET-002** — A roster entry is keyed by its MAC and the router's
  per-device id (falling back to IP and then the MAC), so two devices behind one
  vendor-masked address remain distinct. Names resolve from the live entry, the
  Wi-Fi config's saved name, then the hostname, then a translated unnamed label.
- **SPEC-NET-003** — Per-device rates come from the change in the router's
  receive/transmit byte counters over the actual elapsed time. Before a second
  reading, or when a counter is absent or resets, the router's 15-second average
  is used and never a fabricated delta.
- **SPEC-NET-004** — A device with no counter block is shown as having no
  counters rather than as zero traffic. Ethernet and Wi-Fi bands are labelled
  from the interface enum; signal quality is derived from dBm.
- **SPEC-NET-005** — The Nodes roster joins live infrastructure entries with
  `wifiConfig.meshConfigs`, so a paired mesh node that is currently down is
  still listed as disconnected. Radio temperatures show the fallback sensor and
  the duty cycle.
- **SPEC-NET-006** — Two consecutive failed roster reads mark the router
  unreachable; the page says so instead of showing stale devices, and a later
  answer clears it. A failed config read is retried at the roster cadence.
- **SPEC-NET-007** — The Connected/Nodes segmented selector is centered above
  the network content.

## Acceptance criteria

`dishy-core` tests cover entry keys, name resolution, counter deltas, fallbacks,
band labels, node joining, and radio parsing. A GTK test builds the page, feeds
it a synthetic snapshot, and checks the device list and unreachable branch. The
hardware check opens the page against a real router.

## Deferred

- Client rename and explicit pause are covered by 0170, and metering rules
  (groups, schedules, countdowns) by 0169. Automatic pause enforcement is
  intentionally not ported. Mesh-node renaming is covered by the account-backed
  router configuration in 0142.
- Cloud network fallback behavior is covered separately by 0162.
- Per-device throughput charts and node/device detail (see the 0199 audit).
  Monthly per-device usage persistence and its demand-gated roster reads are
  covered by 0111.
