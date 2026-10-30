---
status: complete
---

# 0173 — Dish terminal facts

## Purpose

Port the Electron dish terminal card (`DishTerminalCard.tsx`): the dish's own
facts from the status reply the recorder already polls, on the dashboard and in
an expanded window. No new request is made.

## Requirements

- **SPEC-TERM-001** — The card lists, in the Electron order and words:
  - model, signal, hardware, firmware, country, uptime, boot count, service
    class, GPS satellites, position;
  - Ethernet link, NAT, downstream routers, bandwidth limit, subsystems,
    boresight, tilt, software update.

  An absent field reads "—".

- **SPEC-TERM-002** — Health facts carry a tone:
  - Signal: persistently low SNR is "weather" (warn), below the noise floor is
    "weak" (bad), above it is "normal" (good).
  - Subsystems: "all ready" only when every subsystem is explicitly ready;
    otherwise the ones still coming up are named (warn).
  - Position: locked with a fix is good, unless the filter is unconverged.
- **SPEC-TERM-003** — Service class reads as residential, roam (consumer kit
  with nomadic or mobile mobility), business, or business plus. Uptime uses the
  shared day/hour/minute catalog forms. A bypassed router is marked.
- **SPEC-TERM-004** — A pending update shows a banner: the reboot window when
  the dish reports one, else the update state when it is not idle.
- **SPEC-TERM-005** — While the dish is not answering, the header says so and
  says how old the facts are.

## Acceptance criteria

- GTK tests cover every fact's wording and tone, the banner, and the stale
  header, from recorded status replies.
