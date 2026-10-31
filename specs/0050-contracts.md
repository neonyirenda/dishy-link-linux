---
status: active
---

# 0050 — Contract corpus

## Purpose

The decoder and the telemetry maths must match known-good behaviour, not
merely compile. The corpus is the mechanism: wire bytes captured from a real
dish and their expected decoded form. It was first exported from the upstream
[Dishylink](https://github.com/DaveyHert/Dishylink) reference implementation
and is frozen here.

## Requirements

- **SPEC-CONTRACT-001** — Each captured response frame decodes, through the
  bundled schema, to exactly the golden JSON committed beside it. Numeric
  comparison follows the single-precision tolerance defined in spec 0020.
- **SPEC-CONTRACT-002** — The committed request frames are byte-identical to
  the frames the Rust client builds.

## Layout

```
crates/dishy-core/fixtures/grpc/*.request.bin    request frames
crates/dishy-core/fixtures/grpc/*.response.bin   captured response frames
crates/dishy-core/fixtures/golden/*.json         expected decodes
crates/dishy-core/fixtures/telemetry/*.json      expected telemetry maths
crates/dishy-core/fixtures/recorder/             recorder stores and their compactions
crates/dishy-sky/fixtures/*.json                 expected dish and satellite meshes
```

A changed fixture is a behaviour change and needs the same review as code.

## What belongs in the corpus

- One response per RPC the app reads.
- Requests whose encoding is non-obvious (oneof selections, config writes,
  stow/unstow).
- Cross-language maths: latency buckets, energy buckets, alert transitions.
- No credentials, no account identifiers, no live session cookie.
