# Configuration Reference

## File location

`/etc/nixops3/nixops3.toml`

The file must be owned by `root:root` with mode `0600` — it may contain AWS credentials.

The daemon reads this file at startup and re-reads it at the beginning of every poll cycle. You can change poll intervals or toggle inventory without restarting the service.

## Complete reference

```toml
# ── Required ──────────────────────────────────────────────────────────────────

# S3 bucket containing your NixOS configuration files
bucket = "nixops3-myhomelab"

# AWS region where the bucket lives
region = "us-east-1"

# S3 path to this machine's role directory (without trailing slash)
# The daemon fetches: roles/<role>/main.nix
#                     roles/<role>/<hostname>/main.nix  (if present)
role = "home/production/webserver"

# ── Optional ──────────────────────────────────────────────────────────────────

# Base poll interval in seconds. Actual sleep = poll_interval_secs + random(0..60)
# Default: 600 (10 minutes)
poll_interval_secs = 600

# ── AWS credentials ───────────────────────────────────────────────────────────
# Omit this section to use the default AWS credential chain:
#   1. Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
#   2. ~/.aws/credentials
#   3. EC2/ECS instance IAM role  ← recommended for cloud machines
#
# Use this section for bare-metal machines that have no instance role.
[aws]
access_key_id     = "AKIAIOSFODNN7EXAMPLE"
secret_access_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"

# ── Inventory reporting ───────────────────────────────────────────────────────
# When enabled, the daemon writes a heartbeat item to DynamoDB after every
# poll cycle and runs any queries.toml queries before each rebuild.
# Default: disabled
[inventory]
enabled = true
table   = "nixops3-inventory"   # DynamoDB table name (must exist)
```

## Field reference

### `bucket` (required)

The S3 bucket name. Do not include the `s3://` prefix or trailing slash.

### `region` (required)

The AWS region of the S3 bucket. Must match exactly (e.g. `us-east-1`, `eu-west-2`).

### `role` (required)

The S3 path prefix for this machine's role. This is the path up to (and including) the role name — not the hostname directory. For a machine at `roles/home/production/webserver/web-01.example.com/`, set `role = "home/production/webserver"`.

The daemon resolves the hostname automatically via `hostname --fqdn` and looks for the host directory under the role path.

### `poll_interval_secs` (optional, default: `600`)

Base polling interval. The actual sleep duration is `poll_interval_secs + uniform_random(0, 60)`. The jitter prevents thundering herd on large fleets that were started simultaneously.

Must be greater than zero.

### `[aws]` section (optional)

Static AWS credentials. When absent, the daemon uses the default AWS credential provider chain. For EC2 and ECS, omit this section and attach an IAM role to the instance.

### `[inventory]` section (optional)

Controls fleet inventory. When `enabled = false` (the default), no DynamoDB calls are made and the daemon never contacts DynamoDB regardless of other config.

When enabled, `table` is required and must be the name of an existing DynamoDB table with `hostname` as the partition key.

See [Inventory & Queries](inventory.md) for DynamoDB setup and IAM requirements.

## Filesystem paths

These paths are created by the daemon or its systemd-tmpfiles configuration. You should not need to manage them directly.

| Path | Purpose | Notes |
|------|---------|-------|
| `/etc/nixops3/nixops3.toml` | Config file | Must be `0600 root:root` |
| `/etc/nixos/hardware-configuration.nix` | Hardware config | Never fetched from S3; always imported first |
| `/var/lib/nixops3/current/` | Working directory | Downloaded `.nix` files + generated `configuration.nix` |
| `/run/nixops3/last-hash` | Last-applied config hash | tmpfs — cleared on reboot; triggers apply on first boot |
| `/run/nixops3/secrets/` | Secrets from Secrets Manager | tmpfs — cleared on reboot; mode `0700 root:root` |
| `/var/lib/nixops3/inventory.json` | DynamoDB query results | Written before each rebuild; read by `.nix` files |

## Systemd tmpfiles

The daemon requires `/run/nixops3/` to exist on startup. Create it via systemd-tmpfiles:

```
# /etc/tmpfiles.d/nixops3.conf
d /run/nixops3         0700 root root -
d /run/nixops3/secrets 0700 root root -
```

## Minimal configuration examples

### EC2 instance with IAM role

```toml
bucket = "nixops3-prod"
region = "us-east-1"
role   = "aws-us-east-1/production/api-server"

[inventory]
enabled = true
table   = "nixops3-inventory"
```

### Bare-metal homelab machine

```toml
bucket = "nixops3-homelab"
region = "us-east-1"
role   = "home/production/ada"

[aws]
access_key_id     = "AKIA..."
secret_access_key = "..."
```

### Minimal (no inventory, no explicit credentials)

```toml
bucket = "nixops3-homelab"
region = "eu-west-1"
role   = "home/dev/workstation"
```
