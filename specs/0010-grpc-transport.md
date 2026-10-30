---
status: active
---

# 0010 — grpc-web transport

## Purpose

Speak the dish's and router's LAN API correctly, including the two quirks that
make a plain HTTP client fail: the CORS allowlist and the `Referer` guard. This
is the port of `core/grpcWeb.ts`.

## Requirements

- **SPEC-TRANSPORT-001** — A unary call sends exactly one message frame:
  a 1-byte flag (`0x00`), a 4-byte big-endian length, then the message. The
  response parser must handle message frames and trailers frames, taking the
  last message frame.
- **SPEC-TRANSPORT-002** — Requests carry `content-type:
  application/grpc-web+proto`, `x-grpc-web: 1`, and `accept:
  application/grpc-web+proto`, and must not carry `Origin` or `Referer`.
- **SPEC-TRANSPORT-003** — Trailers are ASCII `key: value\r\n` pairs.
  `grpc-status` and `grpc-message` are parsed, and the message is
  percent-decoded.
- **SPEC-TRANSPORT-004** — A non-zero status, whether in trailers or in HTTP
  headers, becomes a typed `GrpcWebError::Status`. HTTP 401/403 map to
  `UNAUTHENTICATED`; other non-2xx responses map to `UNKNOWN`.
- **SPEC-TRANSPORT-005** — Every call reports request and response wire bytes
  exactly once, including the headers we set, the URL, and a status line.
- **SPEC-TRANSPORT-006** — A per-call deadline is honoured and surfaces as a
  transport error, never as a successful empty response.

## Non-goals

- Streaming calls. Every RPC used by the app is unary.
- Transparent retries. A retry policy belongs to the poller, which knows
  whether the device is expected to answer.

## Acceptance criteria

`rust/crates/dishy-core/tests/transport.rs` proves each requirement against a
fake HTTP server, including byte-identical request framing and the absence of
`Origin`/`Referer`.
