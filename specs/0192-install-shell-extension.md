---
status: complete
---

# 0192 — Install the Shell extension from Settings

## Purpose

Installing the Quick Settings pill (spec 0189) should not need a terminal.
Settings → App offers to install it, shows its state as GNOME Shell reports
it, and finishes the job after the login that Wayland needs.

## Requirements

- **SPEC-SHEXT-001** — The app bundles the extension's files. "Install"
  writes them to `~/.local/share/gnome-shell/extensions/dishylink@dishylink.app`,
  replacing an older copy, which is how an update is applied.
- **SPEC-SHEXT-002** — The row shows the extension's state, read from GNOME
  Shell's `org.gnome.Shell.Extensions.GetExtensionInfo`:
  - not installed;
  - installed, log out and back in to finish (files present, but the Shell
    has not loaded them yet, as on Wayland);
  - on, off, out of date, or failed.

  Off desktops other than GNOME, the row says the extension needs GNOME and
  offers nothing.

- **SPEC-SHEXT-003** — After an install, the app asks the Shell to enable the
  extension (`EnableExtension`) whenever it finds it known but off. It
  remembers the request as `shellExtensionRequested`, so the next start after
  logging in finishes the job without another click. Turning the extension
  off in GNOME Extensions after that is respected: the request is cleared
  once it has been enabled.
- **SPEC-SHEXT-004** — The Flatpak gains exactly two permissions: write
  access to `xdg-data/gnome-shell/extensions`, and talking to
  `org.gnome.Shell.Extensions`.

## Acceptance criteria

- Tests cover the written files, the state mapping, the request flag, the
  row's action, and the Flatpak permissions.
- Manual: install from Settings, log out and in, and the pill appears.
