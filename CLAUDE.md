# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dishylink Linux: a native Rust/GTK4/libadwaita app and recorder for a Starlink kit, plus a
GNOME Shell extension (`packaging/gnome-shell-extension`). The Cargo workspace is the repo root
(`crates/`). Forked from DaveyHert/Dishylink, which is referenced only; nothing from it lives
here except what the build needs (protoset, OUI table, fixtures, translations, inside the crates).
Changes are verified against real hardware on the Starlink network.

## Hardware safety — read before touching anything router-facing

- **NEVER call or poll the router's `get_ping` (field 1009), at any cadence.** Each trial in
  July 2026 (at 2s, 5s, and 30s) was followed within ~15 minutes by a router watchdog reboot.
  Router ping success comes from `get_status`'s `popPingDropRate5m`, which rides a reply we
  already fetch. `SPEC-SAFETY-001` enforces this in `spec_traceability`.
- `wifi_get_ping_metrics` (3007) and `set_config` answer PERMISSION_DENIED to anonymous LAN
  clients on current firmware; writes go through the signed-in account gateway (`dishy-cloud`).
- **Never add a new poll against the router or dish without explicit approval.** Reuse replies
  the recorder already fetches.

## Workflow

- Spec first: add or amend a requirement ID in `specs/`, write the failing test citing it
  (`// SPEC-AREA-NNN`), then implement. Every active requirement must be cited by a test.
- CI (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test --workspace` under Xvfb, and a Flatpak build. Re-run
  `packaging/linux/generate-cargo-sources.py` whenever `Cargo.lock` changes.
- The app and `dishy record` share `~/.config/Dishylink/data` via `dishy_core::paths`.
- Every HTTP reply is read through `dishy_core::http_body::read_capped` (spec 0193).

## Credentials

The Starlink session lives in the Secret Service (`secret-tool`, service
`com.dishylink.desktop`). Never print, log, or commit cookie values.
