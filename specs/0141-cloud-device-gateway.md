---
status: active
---

# 0141 — Authenticated Starlink device gateway

## Purpose

Provide the trusted, testable grpc-web path that cloud-backed router/dish reads
and writes share. It is separate from the LAN recorder and never trusts
renderer-supplied protobuf bytes or URLs.

## Requirements

- **SPEC-CLOUDGW-001** — Device calls use the fixed Starlink `Device/Handle`
  endpoint, authenticated with the host session, grpc-web framing, and the
  protocol's status/trailer semantics.
- **SPEC-CLOUDGW-002** — Router calls target a controller selected from a fresh
  account telemetry response (`WifiHopsFromController == 0`); a mesh-only result
  is not accepted as a controller.
- **SPEC-CLOUDGW-003** — Session expiry, controller absence, device unreachable,
  HTTP/transport failure, and malformed frames remain distinct outcomes.
- **SPEC-CLOUDGW-004** — Requests are schema-encoded in the trusted cloud crate;
  credentials and arbitrary renderer-controlled URLs never leave that boundary.
- **SPEC-CLOUDGW-005** — A gateway call is not replayed after an ambiguous
  transport/device failure. A response explicitly rejected as unauthenticated
  (grpc status 16) may refresh the session and retry once; other statuses do not.

## Acceptance criteria

- Fake transport tests assert the fixed endpoint, cookie, grpc-web headers and
  frame; verify controller selection from telemetry; and cover every error class.
- The cloud crate remains independent of GTK, the recorder, and LAN clients.
- Workspace tests and clippy with warnings denied pass.
