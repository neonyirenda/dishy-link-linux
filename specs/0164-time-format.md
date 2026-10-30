---
status: verified
---

# 0164 — Native time-format preference

## Purpose

Bring the shared App settings time-format choice to GTK without changing date
formatting or introducing device activity.

## Requirements

- **SPEC-TIME-001** — App settings offers localized Automatic, 12-hour, and
  24-hour choices and defaults to Automatic.
- **SPEC-TIME-002** — The choice persists in the shared host preferences JSON
  as `timeFormat`, accepting only `auto`, `12h`, or `24h`; invalid values default
  to `auto` without invalidating other preferences.
- **SPEC-TIME-003** — Automatic follows the shared app convention: English uses
  12-hour labels and Spanish, French, German, and Portuguese use 24-hour labels.
  Forced choices override that convention.
- **SPEC-TIME-004** — The dish software-update window choices use the selected
  hour format while retaining the same underlying 03:00/09:00/15:00/21:00
  values. Twelve-hour labels use the current language's meridiem markers.
  Changing the format does not change the selected window.
- **SPEC-TIME-005** — Changing the preference is host-local UI behavior only;
  it does not issue a router/dish request or alter any date/day-name formatting.

## Acceptance criteria

- Preference tests cover default, valid round-trip, and invalid-field fallback.
- GTK tests cover localized selector choices, persistence intent, 12/24-hour
  labels, and selection preservation when the format changes.
- Existing full Rust workspace checks and spec traceability pass.
