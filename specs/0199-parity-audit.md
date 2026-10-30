---
status: draft
---

# 0199 — Electron → GTK parity audit

Taken 2026-10-30 at the end of the Rust/GTK transition milestones (M1–M5), and
updated the same day once every deferred row had been ported (specs
0172–0184). This is a record, not a requirement list: it has no requirement
IDs.

## Method

1. Every Electron surface was listed: toolbar items, `App.tsx` panels, and each
   component under `src/components` (shared UI primitives excluded).
2. For each component, the shared-catalog keys it renders were checked against
   the Rust sources. A component whose keys are absent was then confirmed
   absent (or found under different keys) by reading the GTK code.
3. Hardware checks are listed separately. Nothing in this audit was checked on
   the Starlink LAN as part of the audit itself.

## Done

| Electron surface                                                                                                                  | GTK                        | Spec                        |
| --------------------------------------------------------------------------------------------------------------------------------- | -------------------------- | --------------------------- |
| Dashboard tiles, charts, searching hero, stale banner                                                                             | Dashboard page             | 0090                        |
| Satellite view: dome, obstruction key, compass, controls                                                                          | Sky page (GL)              | 0100                        |
| Live constellation, serving satellite, location setup, callout                                                                    | Sky page                   | 0101                        |
| Data usage (local + cloud) and per-device usage                                                                                   | Data Usage page            | 0110, 0111                  |
| Network roster, mesh nodes, radio temperatures, cloud fallback                                                                    | Network page               | 0120, 0162                  |
| Network Rules tab: limits, schedules, timers, groups                                                                              | Network → Rules            | 0169, 0170                  |
| Device pause / unpause / rename                                                                                                   | Network device rows        | 0170                        |
| Speed test (gauge and "Starlink" beam views)                                                                                      | Speed test page            | 0168, 0181                  |
| Alignment instruments and facts                                                                                                   | Alignment page             | 0167                        |
| Settings: Starlink, Router, App tabs (incl. dish sleep, DNS, bypass, subnet, mesh names, time format, toolbar style, self device) | Settings page              | 0130, 0142, 0143, 0164–0166 |
| Account sign-in, service plan, billing                                                                                            | Account page               | 0140, 0141                  |
| Alerts: active, history, known checks, notifications                                                                              | Alerts page + header badge | 0150, 0161                  |
| Tray readout, autostart, single instance, hide-on-close                                                                           | Shell                      | 0040, 0171                  |
| Packaging                                                                                                                         | Flatpak manifest + CI      | 0171                        |
| Tile drill-downs: averages, latency overlay and histogram, quality grade, energy history                                          | Dashboard → tile detail    | 0172                        |
| Dish terminal card and expanded facts                                                                                             | Dashboard                  | 0173                        |
| Outage log, post-mortem reports, band-switch detail                                                                               | Alerts → Events            | 0174                        |
| Node and device detail with per-device throughput                                                                                 | Network page               | 0175                        |
| Duplicate-device merge prompt                                                                                                     | Data Usage page            | 0176                        |
| Account device list                                                                                                               | Account page               | 0177                        |
| Alerts menu header, mute toggle, alert chime                                                                                      | Alerts popover             | 0178                        |
| Obstruction time-lapse                                                                                                            | Sky page                   | 0179                        |
| "Use this device location"                                                                                                        | Sky → location form        | 0180                        |
| Satellite motion and trails                                                                                                       | Sky page                   | 0182                        |
| Per-kit dish meshes                                                                                                               | Sky page                   | 0183                        |
| 3-D satellite model                                                                                                               | Sky page                   | 0184                        |

## Deferred (still to port)

None.

## Intentionally dropped

| Electron surface                                         | Reason                                                                                                                                 |
| -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `AppPrompts`, `PromptDialog` (rating / donation prompts) | Store-review and donation nags belong to the Electron distribution channels                                                            |
| `SupportMenu` update check                               | Flatpak handles updates                                                                                                                |
| `RecoveringErrorBoundary`                                | React-specific; GTK has no render-error boundary                                                                                       |
| App icon style and dock badge options in App settings    | macOS-only Electron features                                                                                                           |
| Automatic rule enforcement (`devicePause.ts` retry loop) | By decision for this port: rules are watch-only and a pause is only ever an explicit, confirmed, single cloud write (specs 0169, 0170) |

## Hardware checks still owed

These are implemented and tested, but have not been run against the real kit.
Each spec names its check in its acceptance criteria.

- 0101: constellation appears with a saved location and clears with it; no
  new device request in the recorder log.
- 0168: a speed-test run compares sensibly with the Starlink app; leaving the
  page stops the traffic.
- 0169: an existing `monthly-limits.json` migrates on first launch, and the
  limits read the same afterwards.
- 0170: pause, unpause, and rename land on the router through a signed-in
  account; the viewer's own row never offers a pause.
- 0171: `flatpak-builder --user --install` builds and runs (tray, autostart
  hidden). The first build will be the CI job.
- 0172: each tile's detail opens with live figures, and opening one adds no
  dish or router request.
- 0173: the terminal facts match the Starlink app's device page.
- 0174: a real outage writes a post-mortem report, and the Events tab lists it.
- 0175: a busy device's throughput chart fills from the existing roster reads.
- 0176: a device that changes MAC gets a merge prompt, and both answers stick.
- 0177: the account device list matches the Starlink app.
- 0178: an alert chimes audibly; mute and unmute persist across restarts.
- 0179: the scrubber walks real hourly snapshots and returns to live.
- 0180: on GNOME, the button raises the location prompt and fills the site.
- 0181: the beam animates through a real run and rests lit afterwards.
- 0182: satellites glide and trail smoothly on screen at 60 Hz.
- 0183: the dome draws the actual kit, hinged kits tilting only their head.
- 0184: the crafts render, sized and oriented like Electron's.
