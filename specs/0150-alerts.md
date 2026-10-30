---
status: complete
---

# 0150 — Native alert state, history, and notifications

## Purpose

Port the shared alert behavior to the GTK application. Alert observations come
only from dish/router status replies the recorder already fetches; this work
does not add a hardware poll or change an approved cadence.

## Requirements

- **SPEC-ALERT-001** — Known dish and router alert flags resolve to translated
  clear/firing states with source and severity. State transitions are emitted
  only when a flag changes, and carry the timestamp of the observation.
- **SPEC-ALERT-002** — An unanswered status poll raises an unreachability alert
  without clearing that device's last known flags. A later reply clears the
  unreachability alert. Router silence is not an alert when the dish reports the
  router absent or bypassed. A cycle that did not poll a device is not an
  observation about it.
- **SPEC-ALERT-003** — Alert transitions are stored as durable episodes in the
  recorder's data directory, restored across restart, and retained for 48 hours
  after closure. Open episodes remain until a clear observation.
- **SPEC-ALERT-004** — Alert observations reuse the current dish status poll and
  router status read; they add no device request and do not call `get_ping`.
- **SPEC-ALERT-005** — The GTK alerts surface shows active alerts, cleared
  history, and the current known checks. Offline devices show their last known
  checks as stale rather than as live all-clear.
- **SPEC-ALERT-006** — Desktop notifications are sent only when the saved user
  preference explicitly enables them, only for alert definitions marked
  notifiable, and at most once per transition key per minute. A clear
  notification has a distinct key from its firing notification.
- **SPEC-ALERT-007** — A bell in the application header opens an alerts
  popover, not a navigation page. Centered Active / History / Status tabs
  select one view at a time. Its badge shows the current active-alert count and
  is hidden when that count is zero.

## Acceptance criteria

- Core tests cover edge transitions, source separation, missing observations,
  device silence/recovery, router presence suppression, restart restoration,
  retention, and newest-first history.
- Runner tests cover router-presence interpretation from the existing dish
  status reply; the existing cadence and forbidden-`get_ping` regression tests
  ensure alert support adds no poll.
- GTK tests verify the tab-driven active/history/status popover, that
  opening it does not change the current page, translations, notification
  gating, independent firing/clear throttle keys, and the header badge behavior.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  workspace tests, and spec traceability pass.

## Deferred

- Alert sounds and notification-channel capability explanations.
- Rules for muting or suppressing individual alert types.
