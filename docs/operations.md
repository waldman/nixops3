# Operations

## Logs

The daemon logs to journald via stderr. View logs with:

```bash
# Follow live
journalctl -u nixops3 -f

# Last 100 lines
journalctl -u nixops3 -n 100

# Since last boot
journalctl -u nixops3 -b

# Errors only
journalctl -u nixops3 -p err
```

### Log messages

| Message | Meaning |
|---------|---------|
| `target sha unchanged (abc1234) — no-op` | Symlink already at target; skipped |
| `canary active, hostname not listed — skipping` | Canary skip; no fetch, no rebuild |
| `fetching commits/abc1234/ (12 objects)` | Downloading a new commit tree |
| `running nixos-rebuild switch` | About to apply |
| `apply succeeded in 47s` | `nixos-rebuild` exited 0 |
| `symlink advanced: current -> commits/abc1234` | New state committed |
| `apply failed: <stderr>` | `nixos-rebuild` failed; symlink unchanged |
| `invalid target sha: "..."` | `current` in S3 is malformed |
| `s3 download failed: <error>` | Could not reach S3 or missing object |
| `secrets fetch error: <error>` | Secrets Manager issue; apply continues |

## Service management

```bash
# Status
systemctl status nixops3

# Restart
systemctl restart nixops3

# Force an immediate cycle (bypass the sleep)
# Either restart the daemon...
systemctl restart nixops3
# ...or run single-shot manually (respects flock so it won't collide):
sudo nixops3

# Stop the daemon (host will not auto-update until re-enabled)
systemctl stop nixops3
```

## Manual rebuild (debugging)

The daemon writes `/etc/nixos/configuration.nix` — the standard NixOS
location. To manually rebuild for debugging without running the daemon:

```bash
sudo nixos-rebuild switch -I nixops3=/var/lib/nixops3/current
```

The `-I nixops3=` is required because the role's `main.nix` uses imports
like `<nixops3/profiles/base.nix>` which resolve against the current
commit tree.

## Forcing a re-apply

The daemon skips `nixos-rebuild` when the local symlink already points at
the target sha. To force a re-apply without changing the target:

```bash
# On the host — remove the symlink, then run a cycle:
rm /var/lib/nixops3/current
sudo nixops3
```

Or trigger fleet-wide by bumping `current` to a fresh commit (e.g. push a
no-op change through CI so a new `<sha>` is produced).

## Checking what a host is running

```bash
# The one command that matters:
readlink /var/lib/nixops3/current
# → commits/abc1234...

# What files are in that commit tree
ls /var/lib/nixops3/current/
ls /var/lib/nixops3/current/roles/

# What role is this host?
grep '^role' /etc/nixops3/nixops3.toml

# What the daemon last wrote for nixos-rebuild
cat /etc/nixos/configuration.nix
```

The symlink IS the state. `readlink` is the whole answer. There is no state
file to consult.

## Checking what the fleet target is

```bash
aws s3 cp s3://your-bucket/current -
# abc1234...
```

Compare against a host's `readlink /var/lib/nixops3/current` to see if it's
converged. Or use the DynamoDB heartbeat: each host writes `applied_sha`
and `target_sha` — `applied_sha == target_sha` means converged.

## Rollback

Fleet-wide, one command:

```bash
echo "<previous-sha>" | aws s3 cp - s3://your-bucket/current
```

Every host that isn't canary-gated will pick up the previous commit on its
next poll. The old commit tree is still in S3 (never overwritten) — no
republish needed.

## Checking secrets

```bash
# List fetched secrets (names only — don't print values)
ls /run/nixops3/secrets/

# Verify a secret was fetched
test -f /run/nixops3/secrets/api-key && echo "present" || echo "missing"
```

Secrets are on tmpfs and disappear on reboot. They are re-fetched on the
next apply cycle.

## Checking inventory

```bash
# Query DynamoDB directly
aws dynamodb get-item \
  --table-name nixops3-inventory \
  --key '{"hostname": {"S": "web-01.example.com"}}' \
  --query 'Item.{status: last_run_status.S, applied: applied_sha.S, target: target_sha.S, seen: last_seen.S, ip: ip.S}'

# List all alive machines (TTL > now)
aws dynamodb scan \
  --table-name nixops3-inventory \
  --filter-expression "#ttl > :now" \
  --expression-attribute-names '{"#ttl": "ttl"}' \
  --expression-attribute-values "{\":now\": {\"N\": \"$(date +%s)\"}}" \
  --query 'Items[*].{host: hostname.S, status: last_run_status.S, applied: applied_sha.S, target: target_sha.S}'
```

Convergence check: `applied_sha == target_sha`. Anything else is either
mid-apply, canary-gated, or failing.

## Troubleshooting

### nixos-rebuild fails every cycle

The symlink won't advance until an apply succeeds. Meanwhile the daemon
retries the same target sha each cycle:

```bash
readlink /var/lib/nixops3/current   # last successful sha (old)
aws s3 cp s3://your-bucket/current - # target sha (what daemon keeps trying)
```

View the full `nixos-rebuild` output in the journal:

```bash
journalctl -u nixops3 -n 200 | grep -A 50 "apply failed"
```

Common causes:
- A profile has a syntax error — fix in the automation repo, ship a new commit
- A secret referenced by a profile is missing — provision it in Secrets Manager
- `inventory.json` is malformed or missing when a profile reads it — check DynamoDB connectivity

### S3 download fails

```bash
journalctl -u nixops3 | grep "s3 download failed"
```

Check:
- Bucket name and region in `nixops3.toml` are correct
- Machine's IAM identity has `s3:GetObject` and `s3:ListBucket`
- Test: `aws s3 cp s3://your-bucket/current -` and `aws s3 ls s3://your-bucket/commits/`

### Malformed pointer

If a host logs `invalid target sha: "..."`, `current` in S3 doesn't contain a
valid 40-character hex sha. Check with `aws s3 cp s3://your-bucket/current -`
and fix by writing a valid sha:

```bash
echo "<valid-sha>" | aws s3 cp - s3://your-bucket/current
```

### Canary skip not clearing

The host reports `canary_skip` and won't apply even though you deleted the
canary file:

```bash
# Verify the canary file is actually gone
sha=$(aws s3 cp s3://your-bucket/current -)
aws s3 ls s3://your-bucket/commits/$sha/roles/home/production/webserver/canary.txt
# Should be empty (404)
```

If it's gone but the host still skips, the daemon hasn't polled yet. Wait,
or trigger an immediate cycle: `ssh <host> sudo nixops3`.

### Secrets not updated after rotation

Secrets are fetched each cycle that reaches the fetch step (i.e. any cycle
where `target != applied`). A steady-state no-op cycle does not re-fetch
secrets.

To force a secrets refresh: `rm /var/lib/nixops3/current && sudo nixops3` on
the host, or bump the fleet target to a fresh commit.

### Host disappeared from inventory

DynamoDB TTL expires items after `2 * poll_interval_secs` by default. If a
host stopped reporting, it disappeared. Check:

```bash
systemctl is-active nixops3
journalctl -u nixops3 -n 50
```

If the daemon is running and healthy but the host is missing from inventory,
check DynamoDB write permissions.

### Local disk full from old commit trees

Each commit tree is a full copy of the config. The daemon prunes to
`trees_retain` (default 5) after each successful apply. If disk is filling:

```bash
du -sh /var/lib/nixops3/commits/*
```

Lower `trees_retain` in `nixops3.toml` or manually clean old trees (never
remove the current symlink target):

```bash
current=$(readlink /var/lib/nixops3/current | xargs basename)
for d in /var/lib/nixops3/commits/*/; do
  sha=$(basename "$d")
  [ "$sha" = "$current" ] || rm -rf "$d"
done
```

## IAM quick reference

Minimum permissions per machine (replace placeholders):

```json
[
  {
    "Effect": "Allow",
    "Action": ["s3:GetObject"],
    "Resource": [
      "arn:aws:s3:::BUCKET/current",
      "arn:aws:s3:::BUCKET/commits/*"
    ]
  },
  {
    "Effect": "Allow",
    "Action": ["s3:ListBucket"],
    "Resource": "arn:aws:s3:::BUCKET",
    "Condition": {
      "StringLike": { "s3:prefix": ["commits/*"] }
    }
  },
  {
    "Effect": "Allow",
    "Action": ["dynamodb:PutItem", "dynamodb:Scan"],
    "Resource": "arn:aws:dynamodb:REGION:ACCOUNT:table/TABLE"
  },
  {
    "Effect": "Allow",
    "Action": ["secretsmanager:GetSecretValue"],
    "Resource": "arn:aws:secretsmanager:REGION:ACCOUNT:secret:NixOps/*"
  },
  {
    "Effect": "Allow",
    "Action": ["secretsmanager:ListSecrets"],
    "Resource": "*"
  }
]
```
