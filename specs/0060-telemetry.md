---
status: active
---

# 0060 — Telemetry

## Purpose

Port the dish's ring-buffer decoding and the accumulator that stitches polls
into a continuous series (`core/telemetry.ts`). The live panel, the recorder,
and persisted history must agree about what a sample is, when an outage began,
and which samples are new.

## Requirements

- **SPEC-TELEMETRY-001** — Unroll the ring by absolute counter: sample `n` is
  at index `n % array_length`; the newest sample is stamped at the poll time and
  earlier samples at one-second steps. A latency that is absent, zero, negative,
  or non-finite is `null`, never a reading.
- **SPEC-TELEMETRY-002** — `outages[]` timestamps are GPS epoch; add
  `315_964_800 - 18` seconds (the 18 leap seconds) to reach Unix. `eventLog`
  timestamps are already Unix nanoseconds.
- **SPEC-TELEMETRY-003** — When the event log has events it wins over
  `outages[]`; severity maps critical/warning and defaults to advisory.
- **SPEC-TELEMETRY-004** — Reduce causes to one token (trim, uppercase,
  whitespace/hyphen to `_`, strip `EVENT_REASON_`, `OUTAGE_`, `UT_ALERT_`).
  Unknown causes are `info`, except an explicit `OUTAGE_` token, which is an
  outage.
- **SPEC-TELEMETRY-005** — Select new samples by the dish counter when it is
  present and has not gone backwards; otherwise fall back to timestamps and
  absorb 500 ms of jitter.
- **SPEC-TELEMETRY-006** — The accumulator backfills once, appends only fresh
  samples, stamps the router reading onto those samples, and trims to a span
  measured back from the newest sample held.
- **SPEC-TELEMETRY-007** — A Starlink outage is eight consecutive samples with
  `dropRate >= 1`; a shorter run is a handover blip.
- **SPEC-TELEMETRY-008** — A router latency is a reading only when finite and
  positive. An absent five-minute drop rate means 100% success; a rate outside
  0–1 is unknown.
- **SPEC-TELEMETRY-009** — `get_status` decodes into typed fields, including
  proto3's string spelling of non-finite floats.

## Acceptance criteria

- **001** — Given the captured 900-sample history and a fixed poll time, when
  it is unrolled, then every timestamp, latency, throughput, power, and drop
  value matches the TypeScript window.
- **002** — Given one outage in each epoch, when both are decoded, then they
  start at the same Unix millisecond.
- **003** — Given a history with both lists populated, when events are decoded,
  then only the event-log entries appear.
- **004** — Given `EVENT_REASON_OUTAGE_NO_PINGS`, when its cause is canonicalised,
  then it is `NO_PINGS` and its kind is `outage`.
- **005** — Given a cursor after a reboot, when fresh samples are selected, then
  only samples more than 500 ms newer than the cursor's timestamp are returned.
- **006** — Given a seeded six-hour buffer, when a poll is ingested, then no old
  sample is duplicated and no sample older than six hours from the newest
  remains.
- **007** — Given seven dropped samples, when outage detection runs, then it is
  false; with eight it is true.
- **008** — Given a router drop rate of 0.25, when converted, then success is
  75%; given 1.5, it is unknown.
- **009** — Given the captured status reply, when decoded, then device identity,
  GPS, Ethernet speed, ready states, and uptime are typed values.
