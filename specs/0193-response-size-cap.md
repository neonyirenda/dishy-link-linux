---
status: complete
---

# 0193 — Bounded HTTP replies

## Purpose

Every HTTP reply the app reads used to be buffered whole. The dish and router
answer over plain HTTP on the LAN, so anything that can answer for
`192.168.100.1` or the router's address (a spoofed gateway, a hostile device on
an open network) could stream an unbounded body and exhaust the app's memory.
The cloud and public-internet reads had the same shape. Real replies are small:
the largest is the CelesTrak catalogue at about 2 MB.

## Requirements

- **SPEC-BODY-001** — Every reply body read from the dish, the router, the
  Starlink cloud, CelesTrak, or the IP-location service is read through one
  capped reader. A body over the cap fails the call with an error that names
  the cap. A declared `Content-Length` over the cap is refused before the body
  is read. A chunked body stops being read as soon as it passes the cap.
- **SPEC-BODY-002** — The cap is 16 MiB for every source. That is eight times
  the largest real reply, so no legitimate reply comes near it.

## Acceptance criteria

- Tests cover a body under the cap, a declared length over the cap, and a
  chunked body that passes the cap.
- The LAN transport, the cloud transport, and the external client all read
  through the shared reader.
