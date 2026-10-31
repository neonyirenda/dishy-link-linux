---
status: draft
---

# 0001 — Architecture

## Purpose

Define the crate boundaries and the direction dependencies may point, so the
Rust app keeps the separation the TypeScript app already has: pure logic, host
integration, and presentation are not the same layer.

## Requirements

- **SPEC-ARCH-001** — `dishy-core` must not depend on GTK, libadwaita, or any
  display library. It owns transport, protocol, polling, telemetry maths, and
  recording.
- **SPEC-ARCH-002** — `dishy-gtk` must not reach the network directly. Every
  device or cloud call goes through `dishy-core` or `dishy-cloud`.
- **SPEC-ARCH-003** — `dishy-cli` and `dishy-gtk` must embed the same recorder
  implementation, not two copies of it.
- **SPEC-ARCH-004** — The protobuf schema is loaded from
  `crates/dishy-proto/dish.protoset`; no generated Rust struct may be the only definition
  of a device field.
- **SPEC-ARCH-005** — The recorder's on-disk format must remain readable by the
  TypeScript historian and vice versa.

## Acceptance criteria

- `cargo tree -p dishy-core` contains no `gtk4`, `libadwaita`, or `glib` crate.
- A source scan finds no `reqwest` use in `dishy-gtk`.
- The recorder's store tests read a fixture data directory produced by the TS
  historian and write rows the TS reader accepts.

## Why this shape

The sky renderer, charts, and tray are the parts most likely to change with the
desktop environment. The polling, decode, and storage code is the part that can
cost a network outage if it changes carelessly. Putting a crate boundary between
them keeps the dangerous part testable on a machine with no display.
