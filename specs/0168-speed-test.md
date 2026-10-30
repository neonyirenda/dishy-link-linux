---
status: complete
---

# 0168 — Speed test

## Purpose

Port the Electron speed-test panel (`lib/speedTest.ts`,
`components/speed-test/SpeedTestCard.tsx`) to the GTK Speed test page.
Throughput is measured through the real link against Cloudflare. Latency,
jitter, and loss come from the dish's own per-second telemetry for the run's
window, because timing a fetch measures DNS/TCP/TLS setup, not the link. The
dish's own speed-test RPCs are unimplemented on current firmware. The test
adds no dish or router request.

## Requirements

- **SPEC-SPEED-001** — A run downloads (`/__down?bytes=25000000`) and then
  uploads (`/__up`, 8 MB bodies) over 6 concurrent streams. Each stream
  re-requests until its phase ends, and a failed stream keeps the bytes it
  delivered without failing the run. A phase that delivered no bytes fails the
  run.
- **SPEC-SPEED-002** — Each phase lasts a fixed 1 s ramp-up plus a 10 s
  measurement window. The reported rate is the cumulative average since the end
  of the ramp, in Mbps. The live figure is shown only once the post-ramp window
  is 0.9 s wide, sampled every 250 ms.
- **SPEC-SPEED-003** — The phases run download, upload, resting, then done.
  The download result is held 0.9 s, then the upload phase starts with no
  value and rests 1.1 s. The upload result is held 0.9 s, followed by a 1.1 s
  rest. The run's measurement window ends when upload ends, not after the
  holds.
- **SPEC-SPEED-004** — Cancelling stops every stream promptly, reports nothing
  further, and is not an error. A run started with a cancelled token reports
  nothing. Starting a run cancels any run in flight, and leaving the page
  cancels the run.
- **SPEC-SPEED-005** — Latency is the median of the dish's per-sample latency
  over the run's window. Jitter is the mean absolute difference between
  consecutive samples, and loss is the mean drop rate as a percentage. With no
  latency samples in the window, all three are unknown.
- **SPEC-SPEED-006** — The gauge maps 0–500 Mbps onto its sweep by square root,
  with ticks at 0, 10, 25, 50, 100, 200, 350, and 500. The needle eases toward
  the reading instead of snapping. Download draws in the ink colour and upload
  in the warm colour.
- **SPEC-SPEED-007** — The page uses the shared catalog's words: phase line,
  Go / Run again / Try again, Ready / Failed captions, and the
  measured-against-Cloudflare note. Figures use 1 decimal for throughput, none
  for latency, and 1 for jitter and loss. Unmeasured headline figures read as a
  muted 0.

## Acceptance criteria

- `dishy-core` tests cover the phase meter against a fake clock, link quality,
  the gauge scale, and a run against a local mock server: phase order, a failed
  phase, and cancellation mid-download and before start.
- GTK tests cover the page copy and formatting.
- On the Starlink LAN, a run reads within reason of the Starlink app's test, and
  leaving the page mid-run stops the traffic.

## Deferred

- None. The "Starlink" beam view landed in spec 0181.
