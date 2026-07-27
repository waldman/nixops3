# Canary Rollouts

## What it is

A canary rollout lets you apply a configuration change to one (or a few) machines before the rest of the fleet sees it. You validate on the canary, then promote.

The mechanism is a single file — `canary.txt` — at the root of the S3 bucket.

## canary.txt format

Plain text, one FQDN per line. Blank lines and `#` comments are ignored.

```
# Canary: testing nginx config update — 2026-07-26
web-01.example.com
```

## How the daemon uses it

At the start of every poll cycle, before downloading any config files, the daemon checks for `canary.txt`:

| canary.txt state | This host in the file | Action |
|-----------------|-----------------------|--------|
| Absent | — | Apply normally |
| Present | Yes | Apply normally |
| Present | No | Skip; write `status: canary_skip` to inventory |

Match is exact FQDN — `web-01` does not match `web-01.example.com`.

When skipping, the daemon **does not update `last-hash`**. The skipped machine will apply the accumulated changes as soon as `canary.txt` is removed (on its next poll cycle).

## Workflow

### 1. Start a canary rollout

Commit your config changes alongside a `canary.txt` at the repo root listing the canary host:

```
web-01.example.com
```

Push to main. CI/CD syncs both the config changes and `canary.txt` to S3. On the next poll cycle:
- `web-01.example.com` applies the new config.
- Every other machine skips.

### 2. Validate

Check the canary machine. Useful commands:

```bash
# Check daemon logs
journalctl -u nixops3d -f

# Check apply status via inventory (if enabled)
aws dynamodb get-item \
  --table-name nixops3-inventory \
  --key '{"hostname": {"S": "web-01.example.com"}}'

# Verify the service is healthy
systemctl status nginx
```

### 3. Promote to full rollout

**Option A — Git commit:**

Delete `canary.txt` from the repo. Commit and push. CI/CD removes it from S3. All machines apply on their next poll cycle.

**Option B — CI/CD task (no PR required):**

```bash
aws s3 rm s3://your-bucket/canary.txt
```

Equally auditable via CI/CD run history. No PR overhead.

### 4. Rolling back

Push a revert of the config change. CI/CD syncs to S3. All machines (including the canary, which already applied the bad config) apply the reverted config on their next cycle.

`canary.txt` can remain or be removed — it doesn't affect rollback either way.

## Multi-host canary

List multiple FQDNs to validate on a subset of the fleet before full promotion:

```
# Stage 1: 3 machines from different failure domains
web-01.example.com
web-05.example.com
db-02.example.com
```

## What canary does not do

- **Track which commit is being tested** — that context lives in git and CI/CD.
- **Auto-promote** — promotion is always a deliberate human or CI action.
- **Affect nixos-rebuild behaviour** — it only controls whether `nixos-rebuild` is called at all.
- **Prevent rollback** — you can revert the config change at any time regardless of canary state.

## Inventory during canary

If inventory is enabled, skipped machines still write a heartbeat to DynamoDB with `last_run_status = "canary_skip"`. This lets you see the fleet split in your inventory dashboards: some machines on the new config (`ok`), others holding (`canary_skip`).
