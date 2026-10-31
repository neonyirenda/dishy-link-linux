---
status: complete
---

# 0189 — GNOME Quick Settings status pill

## Purpose

This is a GNOME-first app, so its live status belongs in GNOME's Quick
Settings, beside Wi-Fi and Bluetooth, rather than only in a legacy tray item.
An app cannot add a Quick Settings pill on its own, and a Flatpak cannot
install a Shell extension. So the app publishes its status on D-Bus, and a
small Dishylink Shell extension, installed separately, shows it. The tray
remains for desktops without the extension, behind a switch.

## Requirements

- **SPEC-PILL-001** — The app exports `com.dishylink.Status1` on its own
  session-bus connection at `/com/dishylink/app/Status`, with read-only
  properties:
  - Online, and Connection ("online", "connecting", or "unreachable");
  - Country, UptimeS, DownBps, UpBps, LatencyMs, and ActiveAlerts;
  - Summary, the localized subtitle such as "online · ↓2.9Mb/s".

  The values come from replies the app already holds: no new dish or router
  read.

- **SPEC-PILL-002** — PropertiesChanged is emitted only for properties whose
  values changed. A stale throughput feed reads as zero, as in the tray.
- **SPEC-PILL-003** — The Shell extension, in
  `packaging/gnome-shell-extension/dishylink@dishylink.app`, for GNOME Shell
  46 to 51, adds a "Starlink" Quick Settings menu toggle:
  - its subtitle is the Summary, it is highlighted while online, and it shows
    the Dishylink mark;
  - its menu lists download, upload, latency, and active alerts, then "Open
    Dishylink" (`org.freedesktop.Application.Activate`, falling back to
    launching the desktop file);
  - it watches the bus name and hides while the app is not running.
- **SPEC-PILL-004** — A "Show tray icon" setting, on by default, starts and
  stops the StatusNotifier tray live. It persists as `showTray`.

## Acceptance criteria

- Rust tests cover the property mapping, the changed-properties diff, the
  introspection XML, and the tray preference. The extension's JavaScript
  passes a syntax check.
- Manual: install the extension (see the README) and log in again. The
  pill shows the live status and opens the app.
