# Canary Rollouts

## What it is

A canary rollout lets you apply a configuration change to one (or a few)
machines of a role before the rest of that role sees it. You validate on
the canary, then promote.

The mechanism is a plain-text file — `canary.txt` — inside the role
directory of the commit tree.

Canary is **role-scoped**. A canary on `webserver` does not hold back
`generic_node` or any other role. Each role has its own independent gate.

## canary.txt format

Plain text, one FQDN per line. Blank lines and `#` comments are ignored.

```
# Canary: testing nginx config update — 2026-07-26
web-01.example.com
```

**Location:** `commits/<sha>/roles/<abstraction>/<environment>/<role>/canary.txt`.

The file lives inside the commit tree because it's synced there by CI from
the automation repo — you commit it to the role directory in git, and CI
publishes it into every commit tree along with the rest of the config.

## How the daemon uses it

After resolving the target sha but before fetching the full commit tree, the
daemon issues one GET for the canary file:

```
s3://<bucket>/commits/<target-sha>/roles/<role>/canary.txt
```

| canary.txt state | This host in the file | Action |
|-----------------|-----------------------|--------|
| Absent (404) | — | Proceed to fetch and apply |
| Present | Yes | Proceed to fetch and apply |
| Present | No | Heartbeat `canary_skip`; stop the cycle |

Match is exact FQDN — `web-01` does not match `web-01.example.com`.

When skipping, the daemon does not fetch the commit tree, does not rebuild,
and does not advance the local `current` symlink. The skipped host stays at
whatever sha its symlink currently points to. In inventory, this shows up as
`applied_sha ≠ target_sha` with `status = canary_skip`.

## Workflow

### 1. Start a canary rollout

In the automation repo, commit your config change alongside a
`canary.txt` at the role directory listing your canary host:

```
# automation-repo/roles/home/production/webserver/canary.txt
web-01.example.com
```

Merge the PR. CI:

1. Syncs the full tree into `commits/<new-sha>/` (canary.txt goes with it)
2. Flips `s3://your-bucket/current` to `<new-sha>`

On the next poll cycle:
- `web-01.example.com` applies the new commit.
- Every other webserver-role host skips (heartbeat `canary_skip`).
- Every host of every other role applies normally — canary is scoped to
  webserver only.

### 2. Validate

Check the canary host. Useful commands:

```bash
# Check daemon logs
journalctl -u nixops3 -f

# Check apply status via inventory (if enabled)
aws dynamodb get-item \
  --table-name nixops3-inventory \
  --key '{"hostname": {"S": "web-01.example.com"}}'

# Verify the service is healthy
systemctl status nginx
```

### 3. Promote to full rollout

Delete the canary file from S3:

```bash
sha=$(aws s3 cp s3://your-bucket/current -)
aws s3 rm s3://your-bucket/commits/$sha/roles/home/production/webserver/canary.txt
```

On the next poll, the remaining webserver hosts see 404 for canary.txt and
apply the commit.

The `current` pointer is not touched during promotion — the fleet was
already targeting this sha, canary was just gating.

**Auditability:** the file is still in git for future PRs, but its absence
from the current commit tree is what unblocks the rollout. Next PR will
resync it (canary always active for the next deploy).

### 4. Rolling back

Overwrite `current` with the previous sha:

```bash
echo "<old-sha>" | aws s3 cp - s3://your-bucket/current
```

All hosts (including the canary, which already applied the bad config) roll
back to the previous commit on their next poll cycle. `canary.txt` state in
the current commit doesn't matter for rollback — the mechanism only gates
forward moves, not the pointer flip itself.

## Bypassing canary intentionally

Two options:

1. **Per-PR**: omit `canary.txt` from the role directory when merging the
   change. CI syncs no canary file; every host of that role applies
   immediately. Cleanest option — the intent is visible in the PR diff.
2. **Post-merge**: `aws s3 rm` the canary file right after CI publish, before
   any validation. Same net effect but less auditable.

## Multi-host canary

List multiple FQDNs to validate on a subset of a role's fleet:

```
# Stage 1: 3 webservers from different racks
web-01.example.com
web-05.example.com
web-09.example.com
```

Each listed host applies; every other webserver host skips.

## What canary does not do

- **Gate other roles** — each role has its own `canary.txt`. Independent.
- **Track which commit is being tested** — that context is in `current` and
  in the git history of the automation repo.
- **Auto-promote** — promotion is always a deliberate operator or CI action.
- **Prevent rollback** — you can overwrite `current` at any time.
- **Affect nixos-rebuild behaviour** — it only controls whether the cycle
  proceeds past the gate check.

## Inventory during canary

If inventory is enabled, skipped hosts still write a heartbeat with:

- `last_run_status = "canary_skip"`
- `applied_sha = <old symlink target>` (unchanged)
- `target_sha = <new pointer value>`

You can spot fleet split at a glance: hosts with `applied_sha == target_sha`
are on the new commit; hosts with `applied_sha ≠ target_sha` and
`canary_skip` status are waiting behind a canary.
