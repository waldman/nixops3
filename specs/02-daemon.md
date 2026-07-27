# nixops3 Daemon Spec

## Overview

`nixops3` is a single static binary that manages NixOS configuration from S3. It runs as root and supports two modes selected by CLI flag.

Binary: single static musl-linked executable, no runtime dependencies.
Privilege: runs as root (required for `nixos-rebuild switch`).

## CLI Modes

```
nixops3                  # single-shot: run one cycle and exit
nixops3 --daemon         # daemon: run poll loop indefinitely
nixops3 -d               # alias for --daemon
nixops3 bootstrap [flags]  # write config file and run one cycle (see spec 06)
```

### Single-shot mode (default, no flags)

Runs exactly one poll cycle — download, hash, optionally rebuild — then exits.

Exit codes:
- `0` — cycle completed successfully (`Applied`, `HashUnchanged`, or `CanarySkip`)
- `1` — cycle failed (`S3Error` or `RebuildFailed`)

Use cases: manual invocation from the shell, cron, one-off CI runs, debugging.

### Daemon mode (`--daemon` / `-d`)

Enters the normal poll loop: apply immediately on first start (no `last-hash`), then sleep + repeat indefinitely. This is the mode the systemd service uses.

### Bootstrap mode (`bootstrap`)

See spec 06. Writes `/etc/nixops3/nixops3.toml` from CLI flags, then runs one poll cycle (single-shot behaviour). Does not enter the daemon loop.

## Configuration File

**Path**: `/etc/nixops3/nixops3.toml`

```toml
# Required
bucket = "nixops3-waldman"
region = "us-east-1"
role   = "home/production/ada"        # full S3 path to the role directory

# Optional — defaults shown
poll_interval_secs = 600              # base interval; actual = interval + jitter(0..60s)

# AWS credentials — used for bare-metal machines
# On EC2/ECS, omit and rely on the instance IAM role instead
[aws]
access_key_id     = "AKIA..."
secret_access_key = "..."

# Inventory reporting — disabled by default
[inventory]
enabled  = true
table    = "nixops3-inventory"
ttl_secs = 1296000    # optional; defaults to 2 × poll_interval_secs
```

The `role` field encodes the full hierarchy: `<abstraction>/<environment>/<role-name>`. The daemon does not parse its structure; it uses it as an S3 path prefix.

## Filesystem Paths

| Path | Purpose | Notes |
|------|---------|-------|
| `/etc/nixops3/nixops3.toml` | Daemon config | Read at startup; reread each poll cycle |
| `/etc/nixos/hardware-configuration.nix` | Hardware config | Never fetched from S3; auto-generated if absent; imported if present |
| `/var/lib/nixops3/current/` | Working directory | Downloaded .nix files + generated configuration.nix |
| `/run/nixops3/last-hash` | Last applied config hash | tmpfs; cleared on reboot → first boot always applies |
| `/run/nixops3/secrets/` | Secrets from AWS SM | tmpfs, mode 0700, owner root |
| `/var/lib/nixops3/inventory.json` | DynamoDB query results | Written before each rebuild; read by .nix files |

## Poll Loop

```
loop:
  sleep(poll_interval_secs + jitter(0..60))

  reload config from /etc/nixops3/nixops3.toml

  if canary.txt exists in S3:
    if hostname not in canary.txt:
      log "canary active, skipping"
      write heartbeat to DynamoDB (status: "canary_skip")
      continue

  files = download_config_tree(role, hostname)
  hash  = sha256(concat(sorted(files)))

  if hash == read("/run/nixops3/last-hash"):
    write heartbeat to DynamoDB (status: "ok", no_change: true)
    continue

  if inventory.enabled:
    queries = collect_queries(files)
    results = run_dynamodb_queries(queries)
    write("/var/lib/nixops3/inventory.json", results)

  pull_secrets(role, hostname)             # writes to /run/nixops3/secrets/

  # If hardware-configuration.nix is missing, generate it now.
  # This handles fresh VMs where the file was not created by the installer.
  if not exists("/etc/nixos/hardware-configuration.nix"):
    exec("nixos-generate-config")

  write_configuration_nix(files)           # writes /var/lib/nixops3/current/configuration.nix

  result = exec("nixos-rebuild switch \
    -I nixos-config=/var/lib/nixops3/current/configuration.nix \
    -I nixops3=/var/lib/nixops3/current \
    [-I nixpkgs=<discovered path>]")

  if result.success:
    write("/run/nixops3/last-hash", hash)
    write heartbeat to DynamoDB (status: "ok")
    log "apply succeeded"
  else:
    log "apply failed: " + result.stderr
    write heartbeat to DynamoDB (status: "failed")
    # do NOT update last-hash — next cycle will retry
```

## Config Tree Download

Given `role = "home/production/ada"` and `hostname = "ada-01.waldman.internal"`:

**Pass 1 — Download entry points:**
- `roles/home/production/ada/main.nix`
- `roles/home/production/ada/ada-01.waldman.internal/main.nix` (optional; skip if absent)
- Scan both files for `<nixops3/...>` path references.

**Pass 2 — Download referenced profiles:**
- For each `<nixops3/some/path.nix>` found, fetch S3 key `some/path.nix`.
- Only files explicitly referenced are downloaded. Nothing else.

**Collect queries.toml files:**
- `roles/home/production/ada/queries.toml`
- `roles/home/production/ada/ada-01.waldman.internal/queries.toml`
- Missing files are skipped silently.

All files are written to `/var/lib/nixops3/current/` preserving their relative S3 path.

## Hash Computation

Hash input: SHA-256 of the concatenation of all downloaded `.nix` file contents, sorted by their S3 key. The `queries.toml` files are excluded from the hash (they do not affect the NixOS config).

Stored at `/run/nixops3/last-hash` as a hex string. On first run (file absent), the hash is treated as empty string — guaranteed to differ, ensuring an apply always happens on first boot.

## configuration.nix Generation

The daemon generates `/var/lib/nixops3/current/configuration.nix` with these entries:

1. `/etc/nixos/hardware-configuration.nix` — absolute path, first; **omitted** if the file is still absent after `nixos-generate-config` ran
2. `./roles/<role>/main.nix` — role entry point (always present)
3. `./roles/<role>/<hostname>/main.nix` — host overrides (only if downloaded)

The file also includes a default bootloader guard to satisfy the NixOS bootloader assertion on fresh machines. Roles that manage a real bootloader must override this with `lib.mkForce`.

```nix
# Generated by nixops3 — do not edit manually
{ lib, ... }:
{
  imports = [
    /etc/nixos/hardware-configuration.nix
    ./roles/home/production/ada/main.nix
    ./roles/home/production/ada/ada-01.waldman.internal/main.nix
  ];

  boot.loader.grub.device = lib.mkDefault "nodev";
}
```

Profile imports are the role's own responsibility via `<nixops3/...>` in its `main.nix`. The daemon does not generate profile imports.

## nixos-rebuild Invocation

```sh
nixos-rebuild switch \
  -I nixos-config=/var/lib/nixops3/current/configuration.nix \
  -I nixops3=/var/lib/nixops3/current \
  -I nixpkgs=<discovered path>          # optional; see nixpkgs discovery below
```

`-I nixos-config=` points directly to the generated file (not the directory). Pointing at a directory would cause Nix to look for `default.nix`, which is not the filename the daemon generates.

`-I nixops3=` registers the working directory as a Nix path, allowing role `main.nix` files to reference profiles via `<nixops3/profiles/base.nix>` rather than fragile relative paths.

`-I nixpkgs=` is added when a nixpkgs path can be discovered. It is required on freshly provisioned machines where `sudo` has stripped `NIX_PATH` and root's Nix channels may not yet be set up.

### nixpkgs Discovery

The daemon searches for nixpkgs in this order, stopping at the first hit:

1. `NIX_PATH` environment variable (fastest — already set on login shells)
2. `/etc/set-environment` — generated by NixOS activation, present even without a login shell
3. `/nix/var/nix/profiles/per-user/root/channels/nixos`
4. `/nix/var/nix/profiles/per-user/root/channels/nixpkgs`

If none are found, the `-I nixpkgs=` flag is omitted and `nixos-rebuild` must locate nixpkgs itself.

stdout and stderr are captured. On non-zero exit: log full stderr to journald, do not update `last-hash`.

## Jitter

Jitter is a uniform random value in `[0, 60)` seconds added to `poll_interval_secs` each cycle. This prevents thundering herd on large fleets where all machines were started simultaneously.

## Hostname Resolution

The daemon reads the machine hostname from `/proc/sys/kernel/hostname`. This is equivalent to the short hostname set in the NixOS config. It does **not** call `hostname --fqdn` because that binary is not in systemd's restricted `PATH`.

The hostname is resolved once at daemon startup and reused for the lifetime of the process. If the hostname changes (e.g., via a config apply that sets `networking.hostName`), the daemon must be restarted to pick it up.

## Startup Behaviour

**Daemon mode** — on first start (no `last-hash`): skip the sleep, apply immediately, then enter the normal poll loop.

**Single-shot mode** — always runs the cycle immediately (no sleep), regardless of whether `last-hash` exists.

## Logging

All output goes to journald via stderr. Log lines are plain text, prefixed with severity:

```
INFO  poll cycle started
INFO  canary active, hostname not listed — skipping
INFO  hash unchanged — skipping rebuild
INFO  apply succeeded in 47s
ERROR apply failed: <nixos-rebuild stderr>
ERROR s3 download failed: <error>
```

No structured logging in v1. No log rotation required (journald handles it).

## Systemd Unit

The daemon is managed by a systemd system service (not user service — it runs as root):

```ini
[Unit]
Description=NixOpS3 configuration daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/nixops3 --daemon
Restart=on-failure
RestartSec=30s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

The `/run/nixops3/` tmpfs directory must exist before the daemon starts. Create it via systemd-tmpfiles:

```
d /run/nixops3         0700 root root -
d /run/nixops3/secrets 0700 root root -
```
