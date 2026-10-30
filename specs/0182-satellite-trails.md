---
status: complete
---

# 0182 — Satellite motion and trails

## Purpose

Port Electron's moving constellation (`advanceLookAngles`,
`topocentricState`, `buildTrail`, `drawTrails`). Satellites glide between SGP4
samples instead of jumping once a second, and each draws a short trail. This
replaces the trail half of spec 0101's deferral.

## Requirements

- **SPEC-TRAIL-001** — Each look angle carries the satellite's position and
  velocity relative to the observer in east/north/up.
  - The velocity is the inertial velocity rotated into Earth-fixed axes, minus
    Earth's rotation term ω × r.
  - Stepping the position along the velocity is a first-order transport: one
    second later it matches a fresh SGP4 sample to within a few hundred metres.
- **SPEC-TRAIL-002** — Every frame, each satellite is drawn at its sample
  advanced by the time since sampling. The advance is capped at 1.5 s (one and
  a half 1 Hz samples), so a stalled sampler does not extrapolate the
  constellation off its orbits. Taps pick the drawn positions.
- **SPEC-TRAIL-003** — A trail has 12 points, 550 ms apart. Each earlier point
  is the sample stepped back along its velocity, and the head is the drawn
  position. The trail is rebuilt every frame and never accumulated. A look
  angle without a velocity has no trail.
- **SPEC-TRAIL-004** — The trail is drawn as a camera-facing ribbon:
  - the half-width is 0.004 × the distance to the eye, flaring by up to 0.8
    toward the tail;
  - the glow is 0.3 × the square of the fraction along the trail, so it is
    brightest at the head;
  - each vertex is feathered across the width (−1 to 1);
  - it is blended additively without writing depth.

## Acceptance criteria

- Sky tests cover the velocity against a later SGP4 sample, the advance cap,
  trail ordering and spacing, and the ribbon's widths, glow, and facing.
