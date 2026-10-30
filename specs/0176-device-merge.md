---
status: complete
---

# 0176 — Device merge prompt

## Purpose

Port the Electron merge prompt (`DeviceMergePrompt.tsx`, `mergeCandidates`,
`merge`, `rejectMerge` in `core/clientTotals.ts`). The router can reissue a
device's id. The odometer then holds two records for one device, and the user
is asked whether to fold them together.

## Requirements

- **SPEC-MERGE-001** — A record is proposed for merging into another when:
  - both have the same name (trimmed, ignoring case);
  - the source has been unseen for more than 10 minutes;
  - the target was seen more recently;
  - the user has not said the pair are different devices.

  The proposal states the result: whether the month's bytes add (same month
  only) and what the surviving record would read.

- **SPEC-MERGE-002** — Accepting folds the source into the target:
  - lifetime counters add;
  - month anchors add when the months match, otherwise the month's bytes stand;
  - per-month history merges, newest six months kept;
  - the source becomes an alias of the target, so later readings and rules
    follow it.

  An explicit merge retires an earlier "different devices" answer for the
  pair. Unknown keys, or two keys of one record, are refused.

- **SPEC-MERGE-003** — Declining records the pair, in either order, so it is
  not proposed again. Both answers persist with the odometer.
- **SPEC-MERGE-004** — The Data Usage page shows the first proposal: the two
  devices, the shared name, the outcome line, "Same device" and "Different
  devices", and how many more there are to review. Answers go through the
  recorder command channel.

## Acceptance criteria

- Core tests cover proposal rules, both outcomes, aliasing, and persistence.
- GTK tests cover the prompt copy and the typed commands.
