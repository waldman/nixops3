# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

## 5. The Zen of Python (any language)

These apply to any code in any language. Import them as first principles.

- **Beautiful is better than ugly.**
- **Explicit is better than implicit.** Magic that "just works" today becomes
  the load-bearing detail no one can find at 4 AM.
- **Simple is better than complex.**
- **Complex is better than complicated.** Complexity is inherent; complication
  is what we add on top.
- **Flat is better than nested.**
- **Sparse is better than dense.**
- **Readability counts.** Code is read far more often than written.
- **Special cases aren't special enough to break the rules.**
- **Although practicality beats purity.**
- **Errors should never pass silently. Unless explicitly silenced.**
- **In the face of ambiguity, refuse the temptation to guess.** Ask instead.
- **There should be one — and preferably only one — obvious way to do it.**
- **Now is better than never. Although never is often better than *right* now.**
- **If the implementation is hard to explain, it's a bad idea.**
- **If the implementation is easy to explain, it may be a good idea.**
- **Namespaces are one honking great idea — let's do more of those!**

### The specific checkpoint that would have saved this codebase 500 lines

Before writing any non-trivial code, ask:

> **"Does the tool or library I'm already using solve this problem for me?"**

Example lived through in this repo: writing a manual tarball downloader +
LRU cache + strip-components extraction for nixpkgs, when `nix-rebuild`
accepts a URL argument and handles all of it natively. The obvious way was
already available; I invented a second, worse way. If it's hard to explain
why my custom code exists instead of the built-in path, that's the signal
to stop and reconsider.

## 6. Absolute rule for this project: spec before code

This project has a `specs/` directory. **No code changes without a spec.**
The hook at `.claude/scripts/spec-check.sh` enforces this by blocking
Edit/Write on code files unless a `.claude/spec-ack` file exists referencing
an existing spec.

Workflow for any change:

1. Read the relevant spec(s) in `specs/`
2. If the change isn't covered, update the spec first, commit it, THEN implement
3. `printf 'spec: <filename>\n' > .claude/spec-ack` before editing code
4. Make the code change; the hook validates the ack

Drifts between spec and code are bugs. Finding one is a STOP moment: report
and align before adding more code.

## 7. Project-specific facts

- **Static musl binary.** Anything requiring dynamic linking to system
  libraries is likely wrong.
- **AWS SDK is in the tree** (already pulls in hyper, rustls, etc.). Prefer
  reusing what's transitively available before adding new HTTP/TLS deps.
- **The daemon runs as root in systemd's restricted environment.** Assume
  `PATH` is minimal, `NIX_PATH` is empty, `HOME` may not be set. Pass
  everything explicitly.
- **Immutability by convention.** `commits/<sha>/` in S3 is immutable-by-convention;
  `/var/lib/nixops3/current` is a symlink; `readlink` is the answer to
  "what is this box running." Don't invent state files.
- **Let Nix do what Nix does.** For anything involving nixpkgs, store paths,
  or evaluation — check whether `nix-rebuild`, `nix-store`, or a `-I` flag
  already handles it before writing code.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

@CLAUDE.local.md
