---
status: complete
---

# 0169 — Metering rules

## Purpose

Port the Electron rule model (`core/dataMeter.ts`, `core/schedule.ts`,
`core/deviceGroup.ts`) so a device, or a group of devices, can carry an
allowance, a countdown, and a weekly timetable on one rule. Rules replace the
monthly limits of spec 0160 as the single metering model. In this port rules
are **watch-only**: reaching one is announced and shown, and the router is
never paused automatically. Spec 0170 covers the explicit pause.

## Requirements

- **SPEC-RULE-001** — A rule's cycle is daily, weekly (on a weekday), monthly
  (on a day, clamped to short months), custom (every N days from a start),
  billing (monthly on the account's day), or once (never rolls). Boundaries are
  local calendar midnights.
- **SPEC-RULE-002** — Usage is the device's lifetime counter minus the rule's
  anchor, floored at zero. Each reading updates the last-seen counters. A cycle
  rolls even for an offline device: it re-anchors to the last-seen counters and
  clears the cycle's latch.
- **SPEC-RULE-003** — A rule measures up to three things, and whichever runs out
  first holds the device:
  - an allowance in bytes (zero means none);
  - a countdown of at most 24 h, on its own clock;
  - a timetable segment, on its own boundary.

  Running out of the allowance or the countdown is announced once per latch,
  and the announcement retires after 60 s. A timetable closing is never
  announced.

- **SPEC-RULE-004** — A timetable is `allow` or `block` over windows. Each
  window has weekdays, start and end minutes, and an optional inclusive run of
  dates:
  - A window ending at or before its start runs past midnight.
  - A day the timetable never names is unrestricted.
  - The current segment ends at the next instant the answer changes, and never
    more than a day ahead.
  - Boundaries stay on wall-clock time across daylight-saving shifts.
  - The query-parameter form round-trips. A malformed timetable leaves the rule
    unscheduled.
- **SPEC-RULE-005** — Editing a rule keeps its anchors.
  - A changed cycle moves only the boundaries.
  - Setting a countdown starts it from now.
  - A changed timetable re-seeds its segment.
  - An allowance raised above the charged usage re-arms.
  - Restarting a rule starts its allowance and countdown over from now, but not
    its timetable.
- **SPEC-RULE-006** — A group projects one rule per member and keeps each
  member's anchors across edits.
  - A pooled member is charged the group's summed usage.
  - A group countdown runs on one clock for all members.
  - A member that leaves loses its group rule. A device's own rule stands beside
    its group rules.
  - Identity merges move rules and members to the surviving key. When two rules
    collide, the one with the newest terms wins. Merged members are not counted
    twice, and absence never drops anything.
- **SPEC-RULE-007** — Rules and groups persist in `meters.json` and
  `device-groups.json`, in the TypeScript snapshot shape, by atomic replacement.
  - A never-rolling cycle's end, written by JSON as `null`, reads back as never.
  - Rules written by this host carry `autoPause: false`. A stored `true` is
    preserved but never enforced here.
  - Existing monthly limits migrate once into monthly, watch-only rules that
    keep the month's usage. After that, `monthly-limits.json` is no longer
    written.
- **SPEC-RULE-009** — A device's own monthly allowance with no countdown or
  timetable is that device's monthly limit. It keeps spec 0161's alert
  (`monthlyDataLimit:<key>`, active while over the cap). Every other rule
  announces through `dataLimit:<subject>`, keyed to the group when a pooled
  allowance or group countdown crossed. That alert opens when the rule is
  reached and closes when the announcement retires. No rule raises both alerts.
- **SPEC-RULE-008** — Evaluation reuses the demand-gated roster and odometer
  readings the recorder already holds. It adds no router or dish read, and it
  never writes to a device.

## Acceptance criteria

- `dishy-core` unit tests port the TypeScript cases for cycles, schedules,
  evaluation, edits, restarts, groups, and key resolution, all asserted in
  local wall-clock terms so they pass in any timezone.
- Store tests cover a TypeScript-written snapshot read, a round-trip, `null`
  period ends, and limit migration.
- Workspace tests, clippy, formatting, and spec traceability pass.

## Supersedes

Spec 0160 (watch-only monthly limits). Its limits migrate into rules.
