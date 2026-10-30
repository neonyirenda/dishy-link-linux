---
status: active
---

# 0020 — Protoset decoding

## Purpose

Read responses through the schema the dish itself published, rather than a
hand-maintained struct. Firmware adds fields; a compiled struct silently drops
them, and a wrong field number is worse than a missing one.

## Requirements

- **SPEC-SCHEMA-001** — The bundled descriptor set must define
  `SpaceX.API.Device.Request` and `SpaceX.API.Device.Response`, and a schema
  built from fresh bytes must expose the same names.
- **SPEC-SCHEMA-002** — Oneof requests are encoded as a length-delimited tag
  (`field << 3 | 2`), a varint length, then the sub-message. Sub-messages over
  127 bytes must work.
- **SPEC-SCHEMA-003** — A captured `Device.Response` decodes to proto3 JSON
  matching the golden form: lowerCamelCase names, enums as strings, 64-bit
  integers as strings, default fields omitted. Single-precision `float` fields
  may differ in their last decimal digits, because the two runtimes print the
  shortest form of different widths; the contract comparison allows one part in
  a million for numeric values.
- **SPEC-SCHEMA-004** — A request expressed as proto3 JSON encodes to the same
  bytes as the hand-encoded oneof form.

## Acceptance criteria

`dishy-proto`'s unit tests cover 001, 002, and 004. `tests/contracts.rs` covers
003 using captured status, history, and obstruction responses.

## Why not generated Rust structs

Generated structs would be faster to read, but they must be regenerated from
the same protoset, so they add a build step without removing the schema as the
source of truth. Dynamic decoding also lets the cloud path decode gateway
responses with the exact same registry as the LAN path.
