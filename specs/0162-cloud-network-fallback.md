---
status: verified
---

# 0162 — Authenticated network fallback

## Purpose

Keep the native Network and router-settings surfaces useful when the LAN router
cannot answer, using the existing authenticated Starlink device gateway. LAN
polling remains the fast path. Cloud reads are demand-gated, read-only, and
never use router `get_ping`.

## Requirements

- **SPEC-NETCLOUD-001** — While the Network page is visible and LAN roster
  reads have marked the router unreachable, the app may read Wi-Fi config
  through the authenticated gateway. It retries failed reads at no more than
  the cloud roster cadence and stops after a successful config read, when LAN
  returns, or when the page is hidden.
- **SPEC-NETCLOUD-002** — A cloud roster read is user-requested from the
  unreachable Network view. After the request, it refreshes at the cloud roster
  cadence only while that view remains visible and LAN is unreachable. The UI
  identifies cloud data as account-sourced and shows loading/failure feedback.
- **SPEC-NETCLOUD-003** — Cloud reads use schema-encoded `wifiGetClients` and
  `wifiGetConfig` calls through the trusted gateway, which chooses a fresh
  controller from account telemetry. They never create LAN clients, add router
  polls, or invoke `get_ping`.
- **SPEC-NETCLOUD-004** — Missing account sessions and cloud/network failures
  remain visible and actionable; a LAN recovery supersedes stale cloud results.
- **SPEC-NETCLOUD-005** — Cloud roster data is display-only: it does not update
  the local odometer or claim local per-device throughput/radio readings.

## Acceptance criteria

- Cloud tests verify both request fields use the fixed authenticated gateway,
  select a fresh controller, decode the corresponding replies, and preserve
  session/controller/transport errors.
- GTK tests verify cloud reads are requested only while the Network page is
  visible, the roster requires explicit user action, source/loading/error copy
  renders, and LAN recovery replaces cloud data.
- Workspace tests, clippy, formatting, changed-file formatting, and spec
  traceability pass.
