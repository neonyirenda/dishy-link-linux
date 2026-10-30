# Dishylink GTK

The native Linux app: Rust, GTK4, and libadwaita. It shares the device schema,
translation files, and on-disk history format with the TypeScript app, and is
developed against the same specifications in `../specs/`.

## Status

Every navigation section is now a native page, and the app packages as a
Flatpak. Every surface of the Electron app has been ported: the live
constellation with motion, trails, and 3-D models; the speed test in both
views; network metering rules with explicit device pause and rename; tile
drill-downs; the dish terminal view; the outage log and post-mortems; node and
device detail; and the account device list. `../specs/0199-parity-audit.md`
records what was intentionally dropped and which hardware checks are still owed
before the specs can be marked verified.

M2 covers the typed history/status models, ring-buffer unrolling, GPS-epoch
outages, cursor-gated accumulation, per-minute latency and energy folds, and the
per-device throughput tracker.

M3 adds the durable recorder: atomic JSON Lines writes with torn-tail recovery,
the same one-writer pidfile protocol as the TypeScript collector, per-minute
energy and latency stores with calendar compaction, the event and thermal logs,
the hourly obstruction store, and the rolling sample snapshot. `dishy record`
polls history at the historian's 5 s cadence and folds each reply; the router
status reply it already needs stamps live router readings onto samples.

M4 adds the native shell: one `AdwApplicationWindow` whose close button hides it,
single-instance activation that reopens the first process, a `ksni` tray with the
live throughput readout, XDG autostart launching `--hidden`, persisted theme,
window bounds, and preferences, and translated strings read from the same five
JSON catalogs as the TypeScript app. The recorder runs on its own thread owned by
the application, so hiding the window cannot stop collection.

M5 adds the dashboard: six stat tiles, throughput/latency/power charts over
15 minutes, 1 hour, or 6 hours, native Cairo sparklines, the searching hero, and
the stale-feed banner. It reads the recorder's rolling sample window; it adds no
poll and does not make the router or dish answer twice.

M6 adds the sky view: the obstruction grid built into a native `gtk::GLArea`
scene with a `glow` renderer, faceted terrain, compass, stars, and a dish aimed
at the live boresight. The recorder now also owns the approved 2 s status poll
and 30 s obstruction poll, feeding the dashboard's obstruction tile, the sky
statistics, and an hourly obstruction snapshot. The scene has no browser or
WebGL dependency; when no GL context can be opened it shows a translated
fallback instead of a blank canvas.

M7 adds local data usage and the network view. The data-usage page folds the
recorder's existing per-minute traffic into the Electron panel's seven ranges,
with honest coverage and null gaps, and asks the recorder for a fresh summary
through its command channel. The network page is read-only for now: it lists the
router's clients and mesh nodes, joins paired nodes from the Wi-Fi config, and
shows radio temperatures. Its router reads are demand-gated and approved — the
client roster every 5 s, config on the first answer and after a reconnect, radio
temperatures every 15 s — and stop when the page is hidden. Per-device usage is
now stored from the router's counters, with naming, OUI/type classification,
and a billing-month odometer. Watch-only monthly limits are now available per
device; account-backed roster and Wi-Fi config reads are covered by spec 0162.
Client renaming, pausing, and full metering rules remain for later milestones.

M8 adds the settings page. Its Starlink tab clears the obstruction map, reboots
or factory-resets the dish, and stows or unstows it when the status reports
actuators; its Router tab reboots or factory-resets the router while the router
is answering the existing status cycle. Every device action is confirmed, sent
once through the recorder thread, and reported as a toast, so the GTK crate still
holds no client and no write is retried. The App tab owns language,
notifications, start-at-login, and the tray throughput readout; changing
language rebuilds the window without restarting the recorder. M9 adds a
host-owned Starlink session in the Linux Secret Service, an isolated sign-in
window that closes only after session verification, account/service-plan/billing
views, the authenticated device gateway, router DNS/bypass, and typed dish
configuration writes. Dish settings are read from the LAN only while Settings
is visible (at most once per minute), and remain disabled until a real reply
arrives. Mesh-node renaming now uses the cached Network snapshot and the
existing cloud worker without adding a router poll. Support diagnostics can be
copied on explicit request; the recorder fetches device info and diagnostics
once, and the payload includes only already-cached status and config. The App
tab's time-format preference is persisted locally and updates the dish
software-update window labels without changing the selected hour or contacting
the device. The selected self-device is also persisted locally, offered from
the cached network roster, and marked in the client list without another poll.

M10 adds native alert state and history. The alert engine observes only the
existing dish status and router status replies, stores 48-hour closed-episode
history, and restores open alerts on restart. The GTK Alerts page shows active
alerts, recent history, and known checks (marked stale when a device is
unreachable). Desktop notifications respect explicit opt-in, use shared
translations, and throttle each firing/clear key independently.

The first network-metering slice adds watch-only monthly limits per device.
Progress follows the odometer's current local month, the caps are stored
atomically through the recorder thread, and reaching a limit never pauses the
router. It reuses the existing demand-gated roster reads; groups, schedules,
countdowns, arbitrary cycles, and pause enforcement remain deferred. Reached
caps also open named, durable warning episodes in Alerts and use the existing
explicit notification opt-in.

The sky view's live constellation (spec 0101) propagates the public CelesTrak
Starlink catalogue with SGP4 on a worker thread, only while the page is shown,
and caches it for six hours. The observer location is pasted or approximated
from the public IP; the dish's `get_location` is never called. Satellites sit
on a shell outside the dome, the likely serving satellite is amber (held for
its 15 s handoff slot), and a tap opens a callout.

The speed test (spec 0168) measures throughput over six streams against
Cloudflare, with Electron's fixed ramp and window. It reads latency, jitter,
and loss from the dish samples the recorder already holds, and is cancelled
when the page is left.

Metering rules (specs 0169, 0170) replace the monthly limits. A rule can carry
an allowance, a countdown, and a weekly timetable, on one device or a group.
Rules are stored in the historian's own `meters.json` / `device-groups.json`
format. Existing limits migrate once and keep their alerts. Rules are
watch-only: reaching one is announced, and pausing a device is a separate,
confirmed, single write through the signed-in Starlink account, offered only
for a device other than the viewer's own. Device rename uses the same path.

Packaging (spec 0171) is a Flatpak on the GNOME 51 runtime, built offline from
the crates vendored in `packaging/linux/cargo-sources.json`. Its permissions
cover only what the app uses. Start-at-login writes the host's autostart
folder and launches through `flatpak run`.

## Layout

```
rust/
  crates/
    dishy-proto/   protoset loading, request encoding, response decoding
    dishy-core/    grpc-web transport, device clients, polling, recording
    dishy-cloud/   starlink.com account and device gateway (M9)
    dishy-sky/     pure sky maths: survey, camera, vertex buffers (M6)
    dishy-cli/     headless recorder and diagnostics
    dishy-gtk/     GTK4/libadwaita application
specs/             requirements with IDs
contracts/         generated cross-language fixtures
packaging/linux/   .desktop, AppStream, Flatpak manifest, vendored crates
```

## Commands

```bash
cd rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run -p dishy-cli -- status          # needs the Starlink LAN
cargo run -p dishy-cli -- status --router # the router's own status
cargo run -p dishy-cli -- record          # record into collector/data
cargo run -p dishy-cli -- record --once   # one poll, then exit
cargo run -p dishy-gtk                    # native window, tray, and recorder
cargo run -p dishy-gtk -- --hidden        # tray-only start (what autostart uses)
```

The GTK crate needs the native development packages: `libgtk-4-dev` and
`libadwaita-1-dev` (libadwaita 1.4 or newer for `AdwToolbarView`). CI installs
these and runs the window smoke test under Xvfb.

Build and install the Flatpak (needs `flatpak-builder` and the GNOME 51 SDK
with the rust-stable extension):

```bash
python3 packaging/linux/generate-cargo-sources.py   # after any Cargo.lock change
flatpak-builder --user --install --force-clean build-flatpak \
  packaging/linux/com.dishylink.app.json
flatpak run com.dishylink.app
```

Regenerate the contract corpus after changing the TS client or the schema:

```bash
node_modules/.bin/tsx scripts/export-contracts.mts
```

Telemetry and recorder fixtures need no npm install:

```bash
node --experimental-transform-types scripts/export-telemetry-contracts.mts
node --experimental-transform-types scripts/export-recorder-fixtures.mts
TZ=UTC node --experimental-transform-types scripts/export-usage-contracts.mts
```

The recorder fixture generator pins `TZ=UTC` and a fixed clock so its expected
files remain stable. The usage generator does the same because its day, week,
and month boundaries are local calendar boundaries. Production uses the system
timezone, as the TypeScript stores do.

## GNOME Quick Settings pill

The app publishes its live status on D-Bus (`com.dishylink.Status1` at
`/com/dishylink/app/Status`, spec 0189). The Dishylink GNOME Shell extension
shows it as a "Starlink" pill in Quick Settings. The extension only reads that
status: it never talks to the dish or the router.

```sh
../packaging/gnome-shell-extension/make-extension.sh
gnome-extensions install --force ../packaging/gnome-shell-extension/dishylink@dishylink.app.shell-extension.zip
# Log out and back in (Wayland), then:
gnome-extensions enable dishylink@dishylink.app
```

The pill hides while the app isn't running. With it installed, you can turn
off Settings → App → Show tray icon.

## Hardware safety

The rules in the repository root's `CLAUDE.md` apply without modification:
never poll `get_ping`, never add a new poll against either box without approval,
and reuse replies the recorder already fetches. `SPEC-SAFETY-001` is enforced by
a source guard in the test suite. The approved cadences are 5 s history, 2 s
status, and 30 s obstruction; `SPEC-SAFETY-002` pins them in `dishy-core`. The
network page's 5 s roster, 15 s radio, and once-per-answer config reads are
demand-gated while that page is visible (`SPEC-SAFETY-003`); dish settings are
read only while Settings is visible and at most every 60 s (`SPEC-SAFETY-005`).
Maintenance actions
are one-shot, confirmed writes through the recorder thread, never polls and never
retried (`SPEC-SAFETY-004`). Device pause and rename are the same kind of
write, sent through the account instead of the LAN. Metering rules never pause
anything on their own.

Beyond the two boxes and the Starlink account, the app reaches three public
hosts, all on explicit demand: CelesTrak (satellite catalogue, while the sky
page is shown), geojs (only when "Approximate from IP" is pressed), and
Cloudflare (only during a speed test). These requests live in `dishy-core`;
the GTK crate makes no HTTP request itself (`SPEC-ARCH-002`, enforced by a
test).
