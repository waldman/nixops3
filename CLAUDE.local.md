# CLAUDE.local.md — NixOpS3 Project Instructions

## Development Methodology: Spec-Driven Development (SDD)

This codebase is written using **Spec-Driven Development**.

**The rule is absolute: specs come before code.**

1. All design decisions, behaviours, and interfaces are first documented in `specs/`.
2. Code is written only after the relevant spec is complete and agreed upon.
3. Any change to behaviour — bug fix, feature, refactor — requires updating the spec first, then updating the code to match.
4. If code and spec diverge, the spec is the source of truth. Fix the code, not the spec (unless the spec itself is wrong — in which case update the spec first, then the code).

## Spec Location

All specs live in `specs/`. Files are numbered for reading order:

```
specs/
  00-overview.md        — system goals, non-goals, architecture
  01-s3-structure.md    — S3 bucket layout, hierarchy, magic filenames
  02-daemon.md          — nixops3d daemon: config, loop, apply pipeline
  03-canary.md          — canary rollout mechanism
  04-inventory.md       — DynamoDB inventory, heartbeat, search queries
  05-secrets.md         — AWS Secrets Manager integration
  06-bootstrap.md       — Golden ISO, first boot, machine identity
```

## Implementation Rules

- Do not implement anything not described in a spec.
- If a spec is ambiguous, stop and ask — do not interpret silently.
- Tests are written alongside or before implementation, never after.
- The daemon is a static musl binary (Rust, `x86_64-unknown-linux-musl`). No runtime dependencies.
