---
status: complete
---

# 0178 — Alert sounds and the alerts menu header

## Purpose

Port the Electron alert chime (`lib/alertSound.ts`) and the alerts menu header
(`AlertsMenu.tsx`): a severity-toned sound when an alert opens or clears, a
mute toggle beside a notifications toggle, and an active-count line.

## Requirements

- **SPEC-SOUND-001** — The chime is synthesised:
  - critical plays 880 then 587.33 Hz (falling), warning plays 587.33 then
    880 Hz (rising), advisory plays 659.25 Hz, and a clear plays 523.25 Hz;
  - each note is a 0.26 s sine with a 12 ms attack and an exponential decay,
    and notes start 0.13 s apart;
  - the result is encoded as 16-bit mono PCM WAV.
- **SPEC-SOUND-002** — A transition chimes only when sounds are on and that
  alert has not chimed in the last minute. This is independent of the
  notification opt-in, so sound works with notifications off.
- **SPEC-SOUND-003** — Sounds are on by default. The choice persists in the
  shared preferences file as `alertSounds`.
- **SPEC-SOUND-004** — The alerts menu header shows "all healthy" or the
  active count, a mute toggle with its title, and the notifications toggle.
  Unmuting plays one soft note.

## Acceptance criteria

- GTK tests cover the tone table, WAV encoding, the throttle, preference
  persistence, and the header copy.
