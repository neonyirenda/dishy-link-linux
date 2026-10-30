---
status: complete
---

# 0184 — 3-D satellite model

## Purpose

Draw each satellite as Electron's 3-D craft (`satelliteGeometry.ts`, the
instanced `sat` program in `skyScene.ts`), flying nadir-pointing with its nose
along track. Points no longer stand in for it. This closes spec 0101's last
deferral.

## Requirements

- **SPEC-SATMODEL-001** — The craft mesh is Electron's `buildSatellite`, at
  the "distant" detail the sky uses, with the full detail available:
  - body, solar wings, and rear antenna, nose along +z;
  - per-face normals and body, dark, and panel tints.

  Triangle counts and sample vertices match Electron's golden.

- **SPEC-SATMODEL-002** — Each look angle carries the craft's orbital frame in
  east/north/up: radial (away from Earth) and along-track. These come from the
  inertial position and velocity. A degenerate velocity leaves the attitude
  unset.
- **SPEC-SATMODEL-003** — Each craft is drawn at its drawn position (spec 0182)
  in a right-handed basis:
  - up is radial, forward is along-track, and right is forward × up, mapped
    into the scene's east/up/south axes;
  - without an attitude, up points away from the centre and forward is level,
    so the craft never draws with a broken basis.
- **SPEC-SATMODEL-004** — One uploaded model is drawn for the whole
  constellation:
  - instanced, at Electron's size (0.032 × the satellite shell / 22);
  - lit at 0.34 + 0.66 × Lambert under the scene light.

  Plain dots are gone. Only the serving and selected markers remain as dots,
  per spec 0101.

## Acceptance criteria

- Sky tests check the mesh against the golden, the attitude's orthonormality
  and direction against SGP4, and the instance basis with and without an
  attitude.
