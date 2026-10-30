---
status: complete
---

# 0186 — Loading spinner size

## Purpose

The account page showed its loading spinner as the stack child itself. A
`GtkSpinner` draws at whatever size it is allocated, so it filled the whole
page.

## Requirements

- **SPEC-SPIN-001** — A loading state shows a 32 px spinner centred in the
  page. The spinner never takes the page's allocation itself.

## Acceptance criteria

- A GTK test checks the account page's loading spinner keeps its 32 px size
  and is centred rather than filling the page.
