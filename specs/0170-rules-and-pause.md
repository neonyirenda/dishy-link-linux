---
status: complete
---

# 0170 — Rules tab, device pause, and rename

## Purpose

Bring the Electron Network "Rules" tab and the per-device pause and rename to
GTK, on top of the rule model in spec 0169. A pause is never automatic in this
port: it is sent only when a person asks, once, through the authenticated
Starlink account. A rule that is reached offers the pause; it never sends one.

## Requirements

- **SPEC-PAUSE-001** — A pause or unpause rewrites the router's client list
  with only the chosen client's `_permanent` weekly block added (covering the
  whole week) or removed.
  - Every other client entry and every other schedule is preserved.
  - A client missing from the saved list is appended from the live roster when
    its id and MAC are known. Otherwise the request is refused.
  - The target must be a `Router-` id.
- **SPEC-PAUSE-002** — A rename sets `givenName` on the chosen client only.
  An id the router has never reported is refused, not appended.
- **SPEC-PAUSE-003** — A pause control is offered only with a signed-in
  account, a known client id, an identified viewing device, and a row that is
  not the viewer's own: a paused device cannot unpause itself.
- **SPEC-PAUSE-004** — A pause, unpause, or rename is confirmed, sent once
  through the cloud worker, and reported as a toast. It is never sent on a
  timer, by a rule, or as a retry.
- **SPEC-PAUSE-005** — The Network page has a Rules tab:
  - It lists each device rule and each group: what it measures, its usage
    against the allowance, the countdown left, and whether its timetable is
    holding.
  - A dialog creates and edits device and group rules with the cycle,
    allowance, pooled or per-member mode, countdown, and timetable windows.
    Rules can also be restarted and deleted.
  - Every change goes through the recorder command channel. Storage failures
    are shown as failures.
- **SPEC-PAUSE-006** — A reached rule shows "Reached" on its card. With an
  account it offers the explicit pause; without one, it says the router is not
  paused. The `dataLimit:` alerts use the shared catalog's
  allowance and countdown wording, for a device or for a group.
- **SPEC-PAUSE-007** — A refused pause says why instead of hiding the control.
  With no account the row shows the account message; with an unidentified
  viewer, the self-device message; with both, the combined message; and the
  viewer's own row, once the rest of the gate holds, says it cannot be paused
  from here. The messages reuse the shared catalog, and their account and
  settings links raise the matching page. A refusal adds no write control,
  and a pause that reaches the host anyway — without a chosen self device, or
  aimed at the viewer's own device — is refused with a message and never
  sent.

## Acceptance criteria

- `dishy-cloud` tests cover pause, unpause, rename, missing clients, and target
  validation against recorded request shapes.
- GTK tests cover the pause gate, rule card summaries, dialog round-trips into
  typed recorder commands, and the alert copy.
- GTK tests cover each refusal message, the links raising the account and
  settings pages, the same message in the device detail, and the host
  refusing a self-targeted pause before it is sent.
- On the Starlink LAN, with a signed-in account: pausing a second device cuts
  its traffic and unpausing restores it, and the viewer's own row never offers a
  pause.
