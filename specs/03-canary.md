# Canary Rollout Mechanism

## Purpose

Canary allows a configuration change to be applied to a single designated
host of a role before rolling out to the rest of that role. This enables
human (or automated) validation before wide deployment.

Canary in v0.3+ is **role-scoped**: gating one role does not affect other
roles in the fleet. Canarying `webserver` does not hold back `generic_node`.

## The canary.txt file

**Location**: inside the commit tree, at the role level:

```
commits/<sha>/roles/<abstraction>/<environment>/<role>/canary.txt
```

**Format**: plain text, one FQDN per line, Unix line endings.

```
web-01.waldman.internal
web-02.waldman.internal
```

Blank lines and lines starting with `#` are ignored.

**Source of truth**: the automation repo. Commit `canary.txt` in the role
directory. CI syncs it into each new `commits/<sha>/` tree along with the
rest of the config.

## Daemon Behaviour

Each poll cycle, after resolving the target sha and before fetching the full
commit tree, the daemon issues a single GET for:

```
s3://<bucket>/commits/<sha>/roles/<role>/canary.txt
```

| canary.txt state | Hostname listed? | Action |
|-----------------|------------------|--------|
| 404 (absent)    | —                | Proceed to fetch and apply |
| Present         | Yes              | Proceed to fetch and apply |
| Present         | No               | Heartbeat `canary_skip`, stop |

When skipping, the daemon does not fetch the commit tree, does not rebuild,
does not advance the symlink. The skipped host stays at whatever sha its
symlink currently points to.

Hostname matching is exact FQDN match — no partial matches, no globbing.

## Workflow

### Starting a canary rollout

1. In the automation repo, add or update `canary.txt` at
   `roles/<abstraction>/<environment>/<role>/canary.txt` listing the canary
   host(s) for that role.
2. Commit and merge the PR containing your config changes.
3. CI syncs the entire tree into `commits/<sha>/`, then flips `current`.
4. Only the listed host(s) of that role apply the new commit. Other hosts of
   the same role skip. Hosts of other roles apply normally — canary is
   role-scoped.
5. Validate the canary host (logs, services, connectivity).

### Promoting to full rollout

Remove `canary.txt` from S3 for the current commit:

```bash
sha=$(aws s3 cp s3://<bucket>/current -)
aws s3 rm s3://<bucket>/commits/$sha/roles/<abstraction>/<environment>/<role>/canary.txt
```

On next poll, the remaining hosts of that role see 404 for canary.txt and
apply the commit. The pointer (`current`) is not touched.

**Note on the mutation:** this is the only operator-driven mutation permitted
inside `commits/<sha>/`. It is atomic per S3 semantics (one object DELETE)
and both states — present and absent — are valid operator intents.

### Rolling back

Fleet-wide rollback: overwrite `current` with the previous sha.

```bash
echo <old-sha> | aws s3 cp - s3://<bucket>/current
```

All hosts (including the canary) apply the previous commit on their next
cycle. `canary.txt` presence in either the old or new commit does not affect
rollback — the mechanism gates fresh applies, not the direction of movement.

## Bypassing canary intentionally

Two options:

1. **Per-PR**: omit `canary.txt` from the automation repo when merging the
   change. CI syncs no canary file; all hosts apply immediately.
2. **Post-merge**: `aws s3 rm` the canary file immediately after CI publish,
   before any canary validation.

Option 1 is cleaner — the intent is visible in the PR diff.

## Multi-host Canary

`canary.txt` may list multiple FQDNs for staged rollouts across a subset of
hosts before full promotion.

## What canary.txt Does NOT Do

- It does not gate other roles. Each role's `canary.txt` is independent.
- It does not track which commit is being tested — that context lives in
  `current` and in the git history of the automation repo.
- It does not auto-promote — promotion is always a deliberate operator or
  CI action.
- It does not affect `nixos-rebuild` behaviour — it only controls whether
  the cycle proceeds past the gate check.

## Interaction with the symlink

A gated host's local symlink is unchanged during a `canary_skip` — it still
points at whatever commit it last successfully applied. This is visible in
inventory as `applied_sha ≠ target_sha`, with `last_run_status = canary_skip`.
