---
status: complete
---

# 0183 — Per-kit dish meshes

## Purpose

Draw the user's own kit at the centre of the sky dome, as Electron does,
instead of a procedural panel. The meshes are Electron's converted CAD models
(`dishModels/*.ts`). `scripts/export-dish-models.mts` exports them as bundled
binaries along with a golden of Electron's `buildDish`.

## Requirements

- **SPEC-DISHMESH-001** — Every model id the sky resolves has a mesh. An
  unknown id draws the Standard kit. Each mesh carries its scale, panel long
  axis, parts, and optional joint, beam origin, and keep-outside flag, and its
  index count matches its parts.
- **SPEC-DISHMESH-002** — The dish is placed as Electron's `buildDish` places
  it:
  - scaled so the panel's long axis spans 0.5 scene units;
  - a rigid kit leans whole to the boresight, while a hinged kit tilts only its
    head and keeps its foot flat, turning in azimuth;
  - each face is shaded by part tint times (0.3 + 0.7 × Lambert);
  - the model is lifted so its lowest point sits on the ground.

  The vertex data matches Electron's golden to within float rounding.

- **SPEC-DISHMESH-003** — The beam origin is the model's own exit point, 14 mm
  along the boresight by default. The camera keeps 1.1 × the model's radius
  only for a kit that asks for it (the aircraft); other kits leave the camera
  floor alone.

## Acceptance criteria

- Sky tests load every mesh and check the three golden cases (rigid, hinged,
  keep-outside) against Electron's output.
