---
status: complete
---

# 0142 — Cloud router configuration writes

## Purpose

Port the router configuration writes that the Electron app sends
through Starlink's authenticated device gateway. GTK supplies typed intent only;
the cloud crate validates it, selects a fresh controller, encodes the protobuf,
and dispatches once.

## Requirements

- **SPEC-CLOUDCFG-001** — Custom DNS accepts at most four IPv4/IPv6 literals,
  canonicalizes and de-duplicates them in order, and permits an empty list to
  restore Starlink DNS. Invalid input is rejected before any gateway call.
- **SPEC-CLOUDCFG-002** — Bypass changes and factory reset are encoded with the
  correct `WifiSetConfig` apply flag or factory-reset oneof and target only a
  fresh controller. A write is never automatically replayed after an ambiguous
  failure.
- **SPEC-CLOUDCFG-003** — Router settings expose four validated DNS fields and
  send only a typed update through the host cloud worker. The page never owns a
  cookie/client, and it reports both success and failure to the user.
- **SPEC-CLOUDCFG-004** — Subnet changes are limited to the official presets and
  WPA2 passphrase length; the current networks are read first, and the write
  preserves SSIDs/config while removing router-assigned identifiers and
  replacing masked Wi-Fi passwords without changing external-auth passwords.
- **SPEC-CLOUDCFG-005** — Mesh renames require a known node and non-empty name;
  the full mesh map is preserved except for the selected name and router-owned
  incarnation values, which are omitted from the write.
- **SPEC-CLOUDCFG-006** — The bypass control reuses already-fetched account
  telemetry, asks for explicit confirmation with Cancel as the default, and
  defaults an unknown state to the safe recovery action (turn bypass off).
- **SPEC-CLOUDCFG-007** — The subnet form exposes only supported address-range
  presets, requires the current Wi-Fi passphrase, validates before dispatch,
  and sends a typed request through the cloud worker. The UI warns that clients
  may disconnect while the router applies the new range.
- **SPEC-CLOUDCFG-008** — Router factory reset stays disabled until either the
  LAN router answers or the account is connected. When LAN is unavailable, the
  explicit destructive confirmation routes the reset through the cloud gateway.

## Acceptance criteria

- Unit tests assert canonical DNS normalization, input refusal, and exact JSON
  request shapes.
- Fake transport tests assert the gateway target, frame, and no replay after an
  ambiguous failure.
- The GTK smoke test submits a DNS setting through the settings action channel
  and confirms the result is presented as a toast.
- The GTK smoke test submits a subnet selection and passphrase through the same
  typed action channel; invalid passphrases do not dispatch.
- Settings tests verify the bypass target is inverted from known state and is
  off when the current state is unknown.
- The GTK test verifies an unreachable router's factory reset is account-gated,
  still requires a Cancel-default confirmation, and sends the typed cloud action.
- A GTK test seeds mesh-node choices from the cached Network snapshot, validates
  and trims the name, and dispatches the typed cloud update without requesting
  another router read.
- All requirements are traced to Rust tests; workspace tests and clippy pass.
