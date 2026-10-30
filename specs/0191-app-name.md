---
status: complete
---

# 0191 — The GTK app is "Dishylink Linux"

## Purpose

Name the native GNOME app "Dishylink Linux", so it is distinct from the
Electron Dishylink on the same machine.

## Requirements

- **SPEC-NAME-001** — The GTK app shows "Dishylink Linux" in every locale as
  its wordmark, window title, tray title and Open/Quit items, desktop entry,
  AppStream name, autostart entry, and Shell extension. Electron's shared
  strings (`desktop.windowTitle`, `desktop.openApp`, `desktop.quit`) are
  unchanged; the GTK app reads its own keys. The app ID, config folder, and
  recordings keep their names, so nothing already saved is lost.

## Acceptance criteria

- A test checks every locale's GTK name keys and that Electron's are
  untouched. The packaging test checks the desktop entry and the AppStream
  name.
