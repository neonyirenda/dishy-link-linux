---
status: active
---

# 0030 — Hardware safety

## Purpose

The dish and router are small embedded devices on the network the app is
supposed to help diagnose. A careless poll can take that network down. These
are invariants, not features.

## Requirements

- **SPEC-SAFETY-001** — No code path may call the router's `get_ping` (field
  1009). Ping success comes from `popPingDropRate5m` in the `get_status` reply
  the app already fetches.
- **SPEC-SAFETY-002** — One recorder owns every poll against the dish and
  router. The approved cadences are 5 s for history, 2 s for status, and 30 s
  for the obstruction map. The GUI never calls either box itself, and no poll is
  added beyond these without a new approved requirement.
- **SPEC-SAFETY-003** — The network page's router reads are approved and are
  demand-gated: the client roster every 5 s, Wi-Fi config once per answering
  stretch and after a reconnect, and radio temperatures every 15 s, only while
  that page is visible. Closing the page stops them. Together with the history
  cycle's router status, these are the only router polls.
- **SPEC-SAFETY-004** — Maintenance controls are one-shot writes, not polls.
  They are user-initiated, confirmed before sending, serialized through the
  recorder thread, and never retried automatically. The GUI still holds no dish
  or router client.
- **SPEC-SAFETY-005** — The dish configuration read is performed only while
  Settings is visible and no faster than once per minute. It uses the
  recorder-owned dish client and does not alter the approved status, history,
  or obstruction cadences.

## Acceptance criteria

- A source guard in `tests/spec_traceability.rs` fails if any Rust source
  outside that guard names `get_ping`.
- `dishy-core` tests assert the approved cadences, including the demand-gated
  dish configuration refresh, that a due poll fires no more
  often than its interval, and that network polls are demand-gated. The GTK
  crate contains no dish or router client.
- Maintenance controls carry their target and request field in `dishy-core`,
  are sent only from a confirmation result, and are tested through the recorder
  command channel rather than a GUI client.

## Context

On 2026-07-20 the router watchdog rebooted within ~15 minutes of every trial at
2 s, 5 s, and 30 s cadence. See `LOCAL-API.md` and `CLAUDE.md` in the repository
root. The status and obstruction polls were added only after explicit approval
for the sky view; the status reply replaces the app's separate thermal fetch,
and the obstruction grid is read at the cadence the Electron sky view uses.

## Planned requirements (not yet active)

- The recorder must never invent samples across a gap; coverage is reported as
  a fraction.
