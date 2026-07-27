# Canary Rollout Mechanism

## Purpose

Canary allows a configuration change to be applied to a single designated node before rolling out to the full fleet. This enables human (or automated) validation before wide deployment.

## canary.txt

**S3 key**: `canary.txt` at the bucket root.

**Format**: plain text, one FQDN per line, Unix line endings.

```
ada-01.waldman.internal
ada-02.waldman.internal
```

Blank lines and lines starting with `#` are ignored.

## Daemon Behaviour

At the start of each poll cycle, before downloading any config files, the daemon checks for `canary.txt`:

| canary.txt state | Hostname in file | Action |
|-----------------|-----------------|--------|
| Absent | — | Normal apply |
| Present | Yes | Normal apply |
| Present | No | Skip apply; log; write heartbeat with `status: canary_skip` |

The daemon checks `canary.txt` using `hostname --fqdn` to resolve the local FQDN.

When skipping, the daemon does NOT update `last-hash`. The skipped node will apply the config as soon as `canary.txt` is removed.

## Workflow

### Starting a canary rollout

1. Commit config changes to git alongside a `canary.txt` listing the canary hostname(s).
2. CI/CD merges and syncs to S3 (`aws s3 sync`), including `canary.txt`.
3. Only the listed node(s) apply on their next poll cycle.
4. Validate the canary node (logs, services, connectivity).

### Promoting to full rollout

Two options:

**Option A — Git commit:**
Commit the removal of `canary.txt`. CI/CD removes it from S3. All nodes apply on their next cycle.

**Option B — CI/CD task (no PR required):**
Trigger a CI/CD job that runs `aws s3 rm s3://<bucket>/canary.txt`. Equally auditable via CI/CD run history; no additional PR overhead.

### Rolling back

Push a revert of the config change. CI/CD syncs. All nodes (including the canary) apply the reverted config on next cycle. `canary.txt` can remain or be removed — it does not affect rollback.

## Multi-node Canary

`canary.txt` may list multiple hostnames for staged rollouts across a subset of the fleet before full promotion.

## What canary.txt Does NOT Do

- It does not track which commit is being tested — that context lives in git.
- It does not auto-promote — promotion is always a deliberate human or CI action.
- It does not affect `nixos-rebuild` behaviour — it only controls whether `nixos-rebuild` is called at all.
