---
status: active
---

# 0040 — GTK application shell

## Purpose

The host layer: one window, one recorder, a tray item, and preferences that
outlive the process. Ports `electron/main.ts` to GTK4/libadwaita.

## Requirements

- **SPEC-SHELL-001** — Closing the window hides it; recording continues. Quit
  exists only in the tray menu.
- **SPEC-SHELL-002** — A second launch focuses the first instance instead of
  starting a second recorder.
- **SPEC-SHELL-003** — Theme follows light, dark, or the system setting, and
  the choice persists.
- **SPEC-SHELL-004** — Locale comes from the existing five JSON translation
  files; no user-facing string is hard-coded in Rust.
- **SPEC-SHELL-005** — Window bounds are restored across runs, clamped to a
  display that still exists.
- **SPEC-SHELL-006** — Start-at-login installs and removes an XDG autostart
  entry, launching hidden.
- **SPEC-SHELL-007** — The tray item reports live throughput; when no tray host
  is available, the app still records and can be reopened.
- **SPEC-SHELL-008** — Notifications are throttled so a flapping link cannot
  spam.

## Platform notes

GNOME shows StatusNotifierItem entries only with the AppIndicator extension.
Without a tray host the recorder still runs, and a second launch reopens the
window. The optional always-on-top throughput pill that mirrors the Windows
widget uses `gtk4-layer-shell` and lands with the controls work; it is a
convenience surface, not a dependency of recording.

GTK4 has no client-side window-position request, and Wayland deliberately
leaves placement to the compositor. The shell restores the saved size and
clamps it to the display under the saved centre; the saved coordinates are kept
for the backends that can honour them.
