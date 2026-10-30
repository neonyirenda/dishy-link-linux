---
status: verified
---

# 0163 — User-requested dish diagnostics

## Purpose

Restore the Settings action for copying diagnostics while keeping device reads
behind an explicit user action and on the recorder-owned dish client.

## Requirements

- **SPEC-DIAG-001** — The Starlink Settings page offers a localized action to
  copy support diagnostics.
- **SPEC-DIAG-002** — Clicking the action requests device info and diagnostics
  once through the recorder thread. The GTK view never creates a device client,
  and no timer, page visibility change, or startup path triggers these reads.
- **SPEC-DIAG-003** — The copied JSON includes capture time, device info,
  diagnostics, and the latest cached status and configuration when available.
- **SPEC-DIAG-004** — The action is disabled while its request is in flight and
  reports localized success or failure. A failed read or unavailable clipboard
  must not be reported as copied.

## Acceptance criteria

- A GTK shell test clicks the action and proves a single diagnostics command is
  sent through the recorder command channel, with no command sent on page build.
- A result test verifies the diagnostics JSON contains its required fields and
  reports clipboard/read failures without success-shaped fallback.
- Workspace tests and formatting pass; the spec IDs appear in their
  implementation/tests.
