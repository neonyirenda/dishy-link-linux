---
status: active
---

# 0130 — Settings and maintenance controls

## Purpose

The native settings page: the device maintenance actions that still work over
the LAN, plus the host preferences the GTK app owns. Dish configuration writes
(snow melt, sleep schedule, update window, deferral) are deliberately not here:
current firmware refuses LAN config writes, so they belong with the Starlink
account work in M9, where they can take the cloud path the Electron app uses.

## Requirements

- **SPEC-CTRL-001** — A settings page offers Starlink, Router, and App tabs.
  Every label comes from the shared catalogs; no user-facing string is written
  in Rust. The existing header theme control stays as the one theme switch.
- **SPEC-CTRL-002** — Every device control is a one-shot, user-initiated write
  routed through the single recorder thread. The settings page never constructs
  a dish or router client, never retries automatically, and adds no poll.
- **SPEC-CTRL-003** — The Starlink tab can clear the obstruction map, reboot the
  dish, factory reset the dish, and stow or unstow it. Stow is offered only when
  the latest status reports actuators.
- **SPEC-CTRL-004** — The Router tab can reboot the router and factory reset it,
  enabled only while the router is answering the existing status cycle. A
  router that has not answered yet reads as contacting, not as failed.
- **SPEC-CTRL-005** — The App tab edits the host's language, notification
  permission, start-at-login, and tray throughput choices. Choices persist to
  the shared preferences file. Changing language rewrites the preference and
  rebuilds the window in the new locale without restarting the recorder.
- **SPEC-CTRL-006** — Reboots, factory resets, and stow changes require an
  explicit confirmation dialog naming the action; a single click cannot send
  them. Every control result arrives as a toast: the action's success catalog
  key on success, or `common.failed` with the device's error on failure.
- **SPEC-CTRL-007** — The router address defaults to the host's default gateway
  discovered from IP routing tables, falling back to 192.168.1.1 if absent. It can
  be edited and saved before the router answers. A valid IPv4 or IPv6 literal is
  persisted and immediately becomes the recorder's router endpoint; clearing it
  restores the discovered default gateway.
- **SPEC-CTRL-008** — Maintenance actions share a compact height and use
  content-fitting labels so long destructive-action names do not make a button
  expand vertically.
- **SPEC-CTRL-009** — Router address, DNS, subnet-password, and mesh-name inputs
  share consistent dimensions; their Save actions also use consistent sizing.

## Acceptance criteria

- `dishy-core` tests map every action to its target, request field, destructive
  flag, and success key, and assert that stow encodes `unstow` correctly.
- The GTK shell test builds the settings page, asserts the three tabs and the
  maintenance rows, and verifies a control command travels through the recorder
  channel rather than a client in the GUI.
- A GTK/settings test verifies the address editor is available while router
  status is unknown, and a core test verifies the saved address selects the
  correct IPv4 or bracketed IPv6 router endpoint.
- A failed write leaves the page showing the failure; it is never reported as a
  sent command.
- The default router endpoint is selected from the host's active Linux default
  route; the route parser is tested with multiple metrics, malformed entries,
  and no-route fallback.
- The GTK shell test verifies the maintenance action dimensions, router input
  and Save sizing, and dock clearance that keeps the settings page's final
  controls reachable.

## Deferred

- Router subnet changes and cloud factory reset are exposed in 0142 with
  read-modify-write context and explicit destructive confirmation. Mesh-node
  rename is also exposed in 0142 using cached network data and the cloud worker.
- Copy diagnostics is covered by 0163 and uses explicitly requested reads
  through the recorder-owned dish client.
- Native time-format preference is covered by 0164 and changes only display
  labels, not device settings.
- Native self-device identification is covered by 0165 using cached roster data
  and the local `selfClientId` preference.
- Native toolbar style is covered by 0166 and changes only navigation layout.
