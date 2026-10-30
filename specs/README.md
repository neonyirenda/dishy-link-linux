# Specs

Behaviour lives here before it lives in code.

## Format

Every spec starts with YAML front matter:

```yaml
---
status: draft | active | verified
---
```

- **draft** — agreed direction, not yet enforceable. Its IDs are not checked.
- **active** — being implemented. Every requirement ID must be cited by at
  least one Rust source file (usually a test).
- **complete** — implemented and tested, but not yet checked against real
  hardware where the requirement touches it.
- **verified** — implemented, tested, and checked against real hardware where
  the requirement touches it.
- **superseded** — replaced by the spec it names; kept for its history.

Each requirement is a bullet:

```markdown
- **SPEC-AREA-001** — The behaviour, stated so a test can fail.
```

IDs are stable. Do not renumber; retire a requirement by marking it superseded.

## Workflow

1. Add or amend the requirement in a draft spec.
2. Write a failing test named after the ID (`// SPEC-AREA-001: …`).
3. Implement until green.
4. Move the spec to `active` once every ID has a test.
5. Move to `verified` after the hardware or UI check named in its acceptance
   criteria passes.

`cargo test -p dishy-core --test spec_traceability` enforces the middle of that
loop for every spec past draft (active, complete, and verified).
