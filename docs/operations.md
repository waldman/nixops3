# Operations

## Logs

The daemon logs to journald via stderr. View logs with:

```bash
# Follow live
journalctl -u nixops3d -f

# Last 100 lines
journalctl -u nixops3d -n 100

# Since last boot
journalctl -u nixops3d -b

# Errors only
journalctl -u nixops3d -p err
```

### Log messages

| Message | Meaning |
|---------|---------|
| `poll cycle started for hostname=...` | Beginning of a new cycle |
| `hash unchanged — skipping rebuild` | Config unchanged, no action taken |
| `canary active, hostname not listed — skipping` | Canary skip; no rebuild |
| `running nixos-rebuild switch` | About to apply config |
| `apply succeeded` | `nixos-rebuild` exited 0; hash written |
| `apply failed (exit N): ...` | `nixos-rebuild` failed; stderr follows |
| `s3 download failed: ...` | Could not reach S3 or missing file |
| `secrets fetch error: ...` | Secrets Manager issue; apply continues |
| `canary check failed: ...` | Could not fetch `canary.txt`; cycle aborted |

## Service management

```bash
# Status
systemctl status nixops3d

# Restart (e.g. after config file change)
systemctl restart nixops3d

# Force an immediate apply (bypass the sleep)
# The daemon applies immediately on start when last-hash is absent.
# Remove it and restart:
rm -f /run/nixops3/last-hash
systemctl restart nixops3d

# Stop the daemon (machine will not auto-update until re-enabled)
systemctl stop nixops3d
```

## Forcing a rebuild

The daemon skips `nixos-rebuild` when the config hash matches `last-hash`. To force a rebuild without changing any `.nix` file:

```bash
# On the machine:
rm /run/nixops3/last-hash
systemctl restart nixops3d
```

Or from the S3 side, touch any `.nix` file with a no-op comment change — that changes the hash fleet-wide.

## Checking current config

The generated `configuration.nix` and all downloaded `.nix` files are in `/var/lib/nixops3/current/`:

```bash
# What was last applied
cat /var/lib/nixops3/current/configuration.nix

# What profiles were downloaded
ls /var/lib/nixops3/current/profiles/

# What hash was last applied
cat /run/nixops3/last-hash
```

## Checking secrets

```bash
# List fetched secrets (names only — don't print values)
ls /run/nixops3/secrets/

# Verify a secret was fetched
test -f /run/nixops3/secrets/api-key && echo "present" || echo "missing"
```

Secrets are on tmpfs and disappear on reboot. They are re-fetched on the next apply cycle.

## Checking inventory

```bash
# Query the DynamoDB table directly
aws dynamodb get-item \
  --table-name nixops3-inventory \
  --key '{"hostname": {"S": "web-01.example.com"}}' \
  --query 'Item.{status: last_run_status.S, seen: last_seen.S, ip: ip.S}'

# List all alive machines (TTL > now)
aws dynamodb scan \
  --table-name nixops3-inventory \
  --filter-expression "#ttl > :now" \
  --expression-attribute-names '{"#ttl": "ttl"}' \
  --expression-attribute-values "{\":now\": {\"N\": \"$(date +%s)\"}}" \
  --query 'Items[*].{host: hostname.S, status: last_run_status.S, role: role.S}'
```

## Troubleshooting

### nixos-rebuild fails every cycle

Check the last-hash — if it's not written, every cycle triggers a rebuild:

```bash
cat /run/nixops3/last-hash  # should contain a hex string
```

View the full `nixos-rebuild` output in the journal:

```bash
journalctl -u nixops3d -n 200 | grep -A 50 "apply failed"
```

Common causes:
- A profile has a syntax error — fix and push to S3
- A secret referenced by a profile is missing — provision it in Secrets Manager
- `inventory.json` is malformed or missing when a profile reads it — check DynamoDB connectivity

### S3 download fails

```bash
journalctl -u nixops3d | grep "s3 download failed"
```

Check:
- The bucket name and region in `nixops3.toml` are correct
- The machine's IAM identity has `s3:GetObject` on the bucket
- The role path in `nixops3.toml` matches the actual S3 key (case-sensitive)
- Test with: `aws s3 cp s3://your-bucket/roles/your/role/main.nix /tmp/test.nix`

### Canary skip not clearing

The machine reports `canary_skip` and won't apply even though you removed `canary.txt`:

```bash
# Verify canary.txt is gone from S3
aws s3 ls s3://your-bucket/canary.txt
```

If it's gone from S3 but the machine still skips, the daemon hasn't polled yet. Wait for the next cycle, or restart the daemon to trigger an immediate poll.

### Secrets not updated after rotation

Secrets are fetched on every cycle that results in a rebuild. If the hash hasn't changed, the cycle is a no-op and secrets are not re-fetched.

To force a secrets refresh: remove `last-hash` and restart the daemon (see "Forcing a rebuild" above).

### Machine disappeared from inventory

DynamoDB TTL automatically expires items. If a machine's TTL expired, it disappeared from inventory because it stopped reporting. Check:

```bash
systemctl is-active nixops3d
journalctl -u nixops3d -n 50
```

If the daemon is running and healthy but the machine isn't in inventory, check DynamoDB write permissions.

## IAM quick reference

Minimum permissions per machine (replace placeholders):

```json
[
  {
    "Effect": "Allow",
    "Action": ["s3:GetObject", "s3:ListBucket"],
    "Resource": [
      "arn:aws:s3:::BUCKET",
      "arn:aws:s3:::BUCKET/*"
    ]
  },
  {
    "Effect": "Allow",
    "Action": ["dynamodb:PutItem", "dynamodb:Scan"],
    "Resource": "arn:aws:dynamodb:REGION:ACCOUNT:table/TABLE"
  },
  {
    "Effect": "Allow",
    "Action": ["secretsmanager:GetSecretValue", "secretsmanager:ListSecrets"],
    "Resource": "arn:aws:secretsmanager:REGION:ACCOUNT:secret:NixOps/*"
  }
]
```
