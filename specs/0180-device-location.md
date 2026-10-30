---
status: complete
---

# 0180 — Device location through the Location portal

## Purpose

Bring Electron's "Use this device location" to Linux. Electron asks the
browser; the GTK app asks the XDG Location portal (GeoClue), which shows the
desktop's own permission prompt. No dish or router read is involved, which
keeps spec 0101's rule.

## Requirements

- **SPEC-LOC-001** — The location form offers "Use this device location"
  beside the IP lookup.
  - While waiting it reads "Locating…" and both source buttons are disabled.
  - A refusal, a timeout (15 s), or a portal that is unavailable shows the
    catalog's device-location failure note.
- **SPEC-LOC-002** — The request uses the portal's session API: CreateSession,
  then Start, then the first LocationUpdated signal for that session, then the
  session is closed. A Start response other than success fails the request.
- **SPEC-LOC-003** — A LocationUpdated payload becomes an observer location
  from Latitude and Longitude, plus Altitude when it is finite. A payload
  without valid coordinates fails the request. The location is saved like any
  other source.

## Acceptance criteria

- GTK tests cover payload parsing and the form's busy and failure states.
- On a GNOME session, the button raises the location prompt and, once allowed,
  fills in the site.
