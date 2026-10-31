# Security Policy

## Reporting a vulnerability

Please report security issues privately rather than opening a public issue. Use
the **Report a vulnerability** button on this repository's
[Security tab](https://github.com/neonyirenda/dishy-link-linux/security/advisories/new),
which opens a private thread visible only to you and the maintainer.

Include what you found, how to reproduce it, your distribution and desktop, how
you installed the app (Flatpak or a cargo build), and the commit or version you
were running.

Reports are answered on a best-effort basis by a single maintainer. Please allow
a reasonable window for a fix before sharing details publicly.

## Supported versions

Fixes land on `main`. Older builds are not patched, so updating is the way to
pick up a fix.

## Scope

In scope:

- The GTK app (`dishy-gtk`), including its tray icon and the status it
  publishes on D-Bus
- The headless recorder (`dishy record`) and the other `dishy-cli` commands
- The GNOME Shell extension in `packaging/gnome-shell-extension`
- The Flatpak manifest and the permissions it grants
- How the app handles your Starlink account session, and anything it writes to
  disk or to the desktop keyring

Out of scope:

- Vulnerabilities in Starlink hardware, in dish or router firmware, or in
  `starlink.com` itself. Those belong to SpaceX and should go through SpaceX's
  own reporting channels.
- Issues that exist only in the original
  [Dishylink](https://github.com/DaveyHert/Dishylink) desktop app or browser
  extension. Report those upstream.
- Anything that requires an attacker to already control your user session.

## How the app handles your data

- The Starlink account session is stored in your desktop keyring through
  Secret Service and is sent only to Starlink. Sign-in uses a WebKit session
  that is never written to disk. Logs name cookies but never print their values.
- Recordings stay on your machine, in `~/.config/Dishylink/data` unless you
  point `HISTORIAN_DATA_DIR` or `--data-dir` somewhere else.
- The dish and router are reached over plain HTTP on your LAN, because that is
  all they offer. Every reply body, from any source, is read with a 16 MiB cap,
  so a hostile device answering for those addresses cannot exhaust memory.
- The only action the app exports on D-Bus is quit. The status object is read-only.

## Known trade-offs

The Flatpak can write to `~/.local/share/gnome-shell/extensions` so that
**Settings → App → Quick Settings pill** can install the extension. Code placed
there runs inside GNOME Shell, outside the sandbox, so this permission is
effectively a way out of it. If you don't want that, remove the permission with
`flatpak override --user --nofilesystem=xdg-data/gnome-shell/extensions com.dishylink.app`
and install the extension by hand as the README describes.

## Testing

Test against hardware you own. The app talks to a dish and router on your own
network, and the router is a small embedded device that has rebooted under
ordinary polling load. Please don't fuzz or stress its endpoints. A crashed
router takes the whole connection down with it, and that on its own is not a
finding.
