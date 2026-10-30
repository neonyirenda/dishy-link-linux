# Contract corpus

Generated from the TypeScript reference implementation in the upstream
[Dishylink](https://github.com/DaveyHert/Dishylink) repository and frozen here,
so the Rust suite needs no Node install. To regenerate, run upstream's
`scripts/export-contracts.mts` and copy the output in.

```
grpc/*.request.bin     bytes the TS client puts on the wire
grpc/*.response.bin    captured response bytes, decoded by the TS schema
golden/*.json          exactly what the TS schema produces from those bytes
telemetry/*.json       telemetry maths run on the captured history by the TS code
```

`grpc/` and `golden/` come from `scripts/export-contracts.mts`.
`telemetry/` comes from `scripts/export-telemetry-contracts.mts`; it imports the
pure TS modules directly, so it needs no installed dependencies:

```bash
node --experimental-transform-types scripts/export-telemetry-contracts.mts
```

Rules:

- Never hand-edit a file here. Regenerate it.
- Changing a fixture is a behaviour change. Say why in the commit.
- No credentials, account identifiers, device IDs, or live session cookies.
  The sample responses in `schema/` are the only source.
- The Rust side must decode the same bytes to the same JSON. If it cannot, the
  port is wrong, not the fixture.
