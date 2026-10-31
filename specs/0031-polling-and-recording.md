---
status: active
---

# 0031 — Polling and recording

## Purpose

Port `collector/historian.mts`: a 5 s poll that unrolls the dish's 900-sample
ring buffer and writes append-only JSON Lines stores, with honest gaps.

## Requirements

- **SPEC-RECORDER-001** — Poll the dish history at 5 s. Any router client read
  is demand-gated by the view that needs it and is no faster than the existing
  historian.
- **SPEC-RECORDER-002** — Unroll history using the absolute sample counter;
  never duplicate or reorder samples across a wrap.
- **SPEC-RECORDER-003** — Convert outage timestamps from the GPS epoch to Unix,
  accounting for the 18 leap seconds.
- **SPEC-RECORDER-004** — Store one JSON object per line in the existing
  `.ndjson` format, with the existing field names.
- **SPEC-RECORDER-005** — A torn final line is discarded, not fatal.
- **SPEC-RECORDER-006** — Coverage is `sampled / expected`; gaps are never
  filled with last-known values.
- **SPEC-RECORDER-007** — One process owns a data directory, enforced by the
  same lock protocol as the TypeScript collector.
- **SPEC-RECORDER-008** — The app and `dishy record` default to the same data
  directory, `$XDG_CONFIG_HOME/Dishylink/data` (`~/.config/Dishylink/data`
  when unset). `HISTORIAN_DATA_DIR`, then `--data-dir` for the CLI, override
  it. Both resolve it through one function in `dishy-core`.

## Acceptance criteria

- Fixture data directories written by the TS historian read identically.
- A process killed mid-append leaves a readable store.
- Tests cover wrap, gap, and duplicate cases in the unroller.
