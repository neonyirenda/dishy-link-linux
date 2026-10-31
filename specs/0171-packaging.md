---
status: complete
---

# 0171 — Linux packaging (M11)

## Purpose

Ship the GTK app as a Flatpak that installs like any other desktop app, keeps
the tray and start-at-login working inside the sandbox, and asks for no more
access than it uses.

## Requirements

- **SPEC-PKG-001** — The desktop entry and AppStream metadata agree with the
  binary and the app:
  - one id, `com.dishylink.app`, shared by the entry, the metadata's
    launchable, the icon, and the GApplication id;
  - `Exec` names the `dishy-gtk` binary;
  - both files pass `desktop-file-validate` and `appstreamcli validate`.
- **SPEC-PKG-002** — The Flatpak manifest builds offline from vendored crate
  sources generated from `Cargo.lock`, and grants exactly what the app uses:
  - network;
  - the Wayland or X11 display, IPC, and GPU (for the GL sky view);
  - the Secret Service (`org.freedesktop.secrets`) for the account session;
  - the StatusNotifier names for the tray, and notifications;
  - create access to the host's `xdg-config/autostart`, and nothing broader
    on the filesystem.
- **SPEC-PKG-003** — Inside the sandbox, start-at-login writes the entry to the
  host's `~/.config/autostart`, and the entry launches through
  `flatpak run --command=dishy-gtk com.dishylink.app --hidden`. Outside the
  sandbox it writes to `$XDG_CONFIG_HOME/autostart`, launching the current
  executable as before.
- **SPEC-PKG-004** — The package installs the scalable app icon, a 512 px
  bitmap icon, the desktop entry, and the metadata under the app id. It also
  installs the `secret-tool` helper the session store calls.
- **SPEC-PKG-005** — CI builds the Flatpak bundle on every push and pull
  request, and validates both metadata files.

## Acceptance criteria

- A `dishy-gtk` test checks the ids, the `Exec` binary, and the manifest's
  finish-args against this list, and that every `Cargo.lock` crate has a
  vendored source.
- Autostart tests cover the sandboxed path and `Exec`.
- `flatpak-builder --user --install` produces an app that launches, shows the
  tray, and starts hidden at login on a GNOME session.
