---
status: complete
---

# 0174 — Outage log and reports

## Purpose

Port the Electron outage log (`OutageLog.tsx`, `EventDetailModal.tsx`,
`BandSwitchDetail.tsx`, `core/postmortem.ts`, `collector/postmortemStore.mts`)
and the `starlinkOutage` system alert it depends on. Everything is derived from
the history replies, samples, and alert episodes the recorder already holds.

## Requirements

- **SPEC-OUTAGE-001** — `starlinkOutage` is a critical, notifying system alert.
  It is active while the newest eight one-second samples all dropped every
  ping, and it is evaluated after each history poll from the recorder's own
  samples.
- **SPEC-OUTAGE-002** — When a `starlinkOutage` or `dishUnreachable` episode
  closes, a report is written once, keyed `system:<source>:<episode start>`:
  - A `starlinkOutage` start is walked back over the contiguous run of
    fully-dropped samples.
  - The cause is the nearest dish event overlapping the outage (30 s
    tolerance), canonicalised.
  - The five minutes before the drop are taken from 1 s samples when at least
    60 are present, otherwise from minute buckets (throughput only).
  - The report says whether the heater ran in those five minutes, and lists
    the thermal alert episodes overlapping that window through to the end of
    the outage.
- **SPEC-OUTAGE-003** — Reports persist in `postmortems.ndjson` in the
  TypeScript shape, written atomically. A second report for the same id is
  refused, and reports older than 30 days are dropped.
- **SPEC-OUTAGE-004** — Reports are matched to dish outage events, each report
  to at most one event, nearest start first and within the 30 s tolerance. A
  report with no matching event is shown as its own row.
- **SPEC-OUTAGE-005** — The Alerts page has an Events list of dish events and
  unmatched reports, newest first. Each row opens a detail window:
  - kind, title, and time span;
  - a band switch's from and to bands;
  - the report's before-the-drop and thermal sections, with the coverage note;
  - "Copy as text" and "Copy as JSON" when there is a report, and the catalog's
    no-report note when there is not.

## Acceptance criteria

- Core tests cover the alert edge, start walk-back, cause matching, both
  before-drop sources, heater and thermal selection, store idempotence and
  retention, and report assignment.
- GTK tests cover row building, detail sections and text, and copy labels.
