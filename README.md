# Dishylink Linux

A native Linux desktop app for monitoring a Starlink kit, written in Rust with
GTK 4 and libadwaita, plus a GNOME Shell extension that puts your link's
status in Quick Settings.

> **Forked from [DaveyHert/Dishylink](https://github.com/DaveyHert/Dishylink).**
> The upstream project is an Electron app for macOS and Windows, plus a browser
> extension. This fork is a from-scratch native port for Linux. It keeps the
> upstream's device protocol knowledge, on-disk history format, translations
> and test fixtures. It drops the Electron app, the macOS and Windows builds,
> the browser extension, the Docker image and the landing site. Thanks to
> [@DaveyHert](https://github.com/DaveyHert) and the upstream contributors.
> MIT-licensed, like the original.

![Dashboard, dark](docs/screenshots/dashboard-dark.webp)

Dishylink Linux talks to your dish and router directly over your local network,
so it keeps working during an outage, which is exactly when you want to see what
happened. It records history to your own machine. Signing in to your Starlink
account is optional; it adds plan and billing details and a few router
controls. The session is kept in your desktop keyring (Secret Service) and is
sent only to Starlink.

## Screens

Every screen, in the default dark-glass theme. Light-mode versions follow
below.

| | |
| :-: | :-: |
| **Dashboard**: live throughput, latency, power, ping success, obstructions, events | **Satellite view**: 3-D obstruction dome, live constellation, time-lapse |
| ![Dashboard](docs/screenshots/dashboard-dark.webp) | ![Satellite view](docs/screenshots/satellite-dark.webp) |
| **Data usage**: per-period totals and per-device usage | **Network**: connected devices, mesh nodes, metering rules |
| ![Data usage](docs/screenshots/datausage-dark.webp) | ![Network](docs/screenshots/network-dark.webp) |
| **Alignment**: rotation and tilt against the dish's recommendation | **Speed test**: Starlink beam view and gauge |
| ![Alignment](docs/screenshots/alignment-dark.webp) | ![Speed test](docs/screenshots/speedtest-dark.webp) |
| **Account**: optional Starlink sign-in | **Alerts**: active, history, status checks, events |
| ![Account](docs/screenshots/account-dark.webp) | ![Alerts](docs/screenshots/alerts-dark.webp) |
| **Settings → Starlink**: dish info, snow melt, sleep, updates, controls | **Settings → Router**: address, DNS, subnet, Wi-Fi password |
| ![Settings, Starlink](docs/screenshots/settings-starlink-dark.webp) | ![Settings, Router](docs/screenshots/settings-router-dark.webp) |
| **Settings → App**: language, toolbar, notifications, tray, Quick Settings pill | |
| ![Settings, App](docs/screenshots/settings-app-dark.webp) | |

Device names, addresses and IDs are blurred in the Network and Data usage
screenshots.

<details>
<summary>Light mode</summary>

| | |
| :-: | :-: |
| ![Dashboard](docs/screenshots/dashboard-light.webp) | ![Satellite view](docs/screenshots/satellite-light.webp) |
| ![Data usage](docs/screenshots/datausage-light.webp) | ![Network](docs/screenshots/network-light.webp) |
| ![Alignment](docs/screenshots/alignment-light.webp) | ![Speed test](docs/screenshots/speedtest-light.webp) |
| ![Account](docs/screenshots/account-light.webp) | ![Alerts](docs/screenshots/alerts-light.webp) |
| ![Settings, Starlink](docs/screenshots/settings-starlink-light.webp) | ![Settings, Router](docs/screenshots/settings-router-light.webp) |
| ![Settings, App](docs/screenshots/settings-app-light.webp) | |

</details>

### GNOME Shell extension

The Quick Settings pill shows whether the link is online, the live download and
upload rates, latency, and active alerts, and opens the app. It only reads the
status the app publishes on D-Bus (`com.dishylink.Status1`); it never talks to
the dish or router itself.

<!-- extension screenshot: docs/screenshots/gnome-extension.webp -->

## Features

- **Dashboard**: stat tiles with sparklines and drill-downs; throughput, latency
  and power charts over 15 minutes, 1 hour or 6 hours; obstruction map; outage
  and event log with post-mortems.
- **Always-on recording**: the recorder runs on its own thread, so closing the
  window doesn't stop it. History uses atomic JSON Lines files with torn-tail
  recovery, plus per-minute energy and latency, events, thermal and hourly
  obstruction snapshots.
- **Sky view**: obstruction dome rendered natively with OpenGL, live
  constellation from the public CelesTrak catalogue (SGP4), serving-satellite
  highlight and per-kit 3-D dish models.
- **Network**: router clients and mesh nodes, per-device usage with a
  billing-month odometer, metering rules with allowances, countdowns and
  weekly timetables, and confirmed device pause and rename.
- **Alerts**: alert engine with 48-hour history, desktop notifications (opt-in)
  and an alert chime.
- **Settings**: dish and router controls (reboot, stow, factory reset, snow
  melt, sleep schedule, update window, DNS, subnet, Wi-Fi password). Every
  write is confirmed and sent exactly once.
- **Desktop integration**: single instance, tray icon (StatusNotifierItem) with a
  throughput readout, start at login, the GNOME Quick Settings pill, and five
  languages (English, Spanish, French, German, Portuguese).
- **Headless recorder**: `dishy record` runs the same recorder without a GUI,
  for example on a server or a Raspberry Pi.

## Building

Requires Rust (stable, edition 2024, 1.92+) and the GTK stack. On Fedora:

```sh
sudo dnf install gtk4-devel libadwaita-devel webkitgtk6.0-devel libsecret
```

On Debian or Ubuntu:

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev libsecret-tools
```

Then:

```sh
cd rust
cargo run -p dishy-gtk --release      # the desktop app
cargo run -p dishy-cli -- record      # headless recorder
cargo test --workspace                # tests
```

### Flatpak

```sh
flatpak-builder --user --install --force-clean build-dir packaging/linux/com.dishylink.app.json
flatpak run com.dishylink.app
```

The Flatpak builds offline from the crates vendored in
`packaging/linux/cargo-sources.json`. Re-run
`python3 packaging/linux/generate-cargo-sources.py` whenever `rust/Cargo.lock`
changes.

### GNOME Shell extension

Install it from **Settings → App → Quick Settings pill**, or by hand:

```sh
packaging/gnome-shell-extension/make-extension.sh
gnome-extensions install --force packaging/gnome-shell-extension/dishylink@dishylink.app.shell-extension.zip
```

On Wayland, log out and back in, then enable it in Extensions. It supports
GNOME Shell 46–51.

## Hardware safety

The router is a small embedded box that has rebooted under ordinary load.
Dishylink Linux never calls the router's `get_ping`, adds no poll the specs
don't approve, and reuses replies it already fetches wherever it can. A test
enforces the `get_ping` rule. See `specs/` for the approved cadences.

## Repository layout

```
rust/crates/
  dishy-proto/   protoset loading, request encoding, response decoding
  dishy-core/    grpc-web transport, device clients, polling, recording
  dishy-cloud/   starlink.com account and device gateway
  dishy-sky/     sky maths: survey, camera, vertex buffers
  dishy-cli/     headless recorder and diagnostics
  dishy-gtk/     GTK4/libadwaita application (translations in locales/)
packaging/
  linux/                    .desktop, AppStream, Flatpak manifest, vendored crates
  gnome-shell-extension/    the Quick Settings pill
specs/           requirements with IDs; every active one is cited by a test
contracts/       protocol and recorder fixtures shared with upstream
schema/          the dish's gRPC protoset
```

## License

MIT. See [LICENSE](LICENSE). Copyright © 2026 daveyhert (upstream Dishylink)
and the Dishylink Linux contributors. Bundled fonts (Barlow, IBM Plex Mono) are
SIL OFL; their licences ship with them.

Dishylink Linux is not affiliated with or endorsed by SpaceX or Starlink.
