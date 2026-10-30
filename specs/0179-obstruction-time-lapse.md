---
status: complete
---

# 0179 — Obstruction time-lapse

## Purpose

Port the Electron time-lapse scrubber (`ObstructionTimeLapse.tsx` and its use
in `SatelliteView.tsx`). The user scrubs the sky page back through the hourly
obstruction snapshots the recorder already keeps.

## Requirements

- **SPEC-LAPSE-001** — A snapshot's packed cells (base64, two bits per cell)
  decode into the grid the dome draws, using the same kinds and frame rules as
  the live map. A snapshot that does not decode is skipped.
- **SPEC-LAPSE-002** — The scrubber has one stop per snapshot, oldest first,
  then a final live stop.
  - The label is the snapshot's local time while scrubbing, "LIVE" at the live
    stop, or "OFFLINE" there while the dish is not answering.
  - Moving to the live stop returns to the live map.
- **SPEC-LAPSE-003** — While viewing a snapshot:
  - the page says which time it is showing;
  - the live constellation and serving marker are hidden, since a past sky has
    no live satellites;
  - live obstruction replies do not replace the snapshot on screen.

  Snapshots are fetched from the recorder when the page opens. This is a local
  read.

## Acceptance criteria

- Tests cover snapshot decoding, the stop labels, and that scrubbing hides the
  constellation and survives a live reply.
