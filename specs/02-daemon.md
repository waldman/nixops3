# nixops3 Daemon Spec

## Overview

`nixops3` is a single static binary that manages NixOS configuration from S3.
It runs as root and supports two modes selected by CLI flag.

Binary: single static musl-linked executable, no runtime dependencies.
Privilege: runs as root (required for `nixos-rebuild switch`).

## CLI Modes

```
nixops3                    # single-shot: run one cycle and exit
nixops3 --daemon           # daemon: run poll loop indefinitely
nixops3 -d                 # alias for --daemon
nixops3 bootstrap [flags]  # write config file and run one cycle (see spec 06)
```

### Single-shot mode (default, no flags)

Runs exactly one poll cycle — resolve target, fetch if needed, rebuild — then exits.

Exit codes:
- `0` — cycle completed (`Applied`, `NoOp`, or `CanarySkip`)
- `1` — cycle failed (`S3Error` or `RebuildFailed`)

### Daemon mode (`--daemon` / `-d`)

Enters the normal poll loop: apply immediately on first start (no local
symlink), then sleep + repeat indefinitely. This is the mode the systemd
service uses.

### Bootstrap mode (`bootstrap`)

See spec 06. Writes `/etc/nixops3/nixops3.toml` from CLI flags, then runs
one poll cycle (single-shot behaviour). Does not enter the daemon loop.

## Configuration File

**Path**: `/etc/nixops3/nixops3.toml`

```toml
# Required
bucket = "nixops3-waldman"
region = "us-east-1"
role   = "home/production/webserver"    # full S3 path to the role directory

# Optional — defaults shown
poll_interval_secs = 600                # base interval; actual = interval + jitter(0..60s)
trees_retain       = 5                  # LRU limit for local commit trees

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

# nixpkgs pinning (see spec 09) — all optional
[pins]
require_pin          = false     # true: missing pin: block → cycle fails
require_explicit_rev = false     # true: pin without rev → cycle fails
nixpkgs_retain       = 3         # LRU size for /var/lib/nixops3/nixpkgs/
channel_ttl_secs     = 300       # in-process cache TTL for channel resolution
```

The `role` field encodes the full hierarchy: `<abstraction>/<environment>/<role-name>`.
The daemon does not parse its structure; it uses it as an S3 path prefix.

## Filesystem Paths

| Path | Purpose | Notes |
|------|---------|-------|
| `/etc/nixops3/nixops3.toml` | Daemon config | Read at startup; reread each poll cycle |
| `/etc/nixos/hardware-configuration.nix` | Hardware config | Never in S3; auto-generated if absent; imported if present |
| `/var/lib/nixops3/commits/<sha>/` | Downloaded commit trees | LRU pruned to `trees_retain` most recent |
| `/var/lib/nixops3/nixpkgs/<rev>/` | Extracted nixpkgs (pinned tiers, spec 09) | LRU pruned to `nixpkgs_retain` most recent |
| `/var/lib/nixops3/current` | Symlink → `commits/<sha>` | THE state store; last successful apply |
| `/etc/nixos/configuration.nix` | Generated per apply cycle | Standard NixOS location; overwritten each cycle |
| `/run/nixops3/secrets/` | Secrets from AWS SM | tmpfs, mode 0700, owner root |
| `/var/lib/nixops3/inventory.json` | DynamoDB query results | Written before each rebuild; read by .nix files |

**No `/run/nixops3/last-hash` file.** The symlink is the state. `readlink
/var/lib/nixops3/current` is the complete answer to "what is this box running".

## Poll Loop

```
loop:
  sleep(poll_interval_secs + jitter(0..60))    # skip on first cycle

  reload config from /etc/nixops3/nixops3.toml

  # 1. Resolve target
  target = GET s3://bucket/current
  validate: 40 hex chars, else log error and continue
  
  # 2. No-op if already there
  if readlink(/var/lib/nixops3/current) == "commits/<target>":
    write heartbeat (status: ok, applied_sha=target, target_sha=target)
    continue

  # 3. Canary gate — role-scoped, per-commit
  canary = GET s3://bucket/commits/<target>/roles/<role>/canary.txt
  if canary exists and hostname not in canary:
    log "canary active, hostname not listed — skipping"
    write heartbeat (status: canary_skip, applied_sha=<current symlink>, target_sha=target)
    continue

  # 4. Ensure the commit tree is local
  if not exists(/var/lib/nixops3/commits/<target>/):
    list = list_prefix("commits/<target>/")
    parallel: for key in list: GET key, write to /var/lib/nixops3/<key>

  # 5. Load main.yaml (merged role + host per spec 08)
  meta = load_main_yaml(commit_tree, role, hostname)

  # 6. Resolve pin per spec 09 (three-tier: Loose / Floating / Pinned)
  # Sets `nixpkgs_path` to /var/lib/nixops3/nixpkgs/<rev>/ for Floating/Pinned,
  # or None for Loose (fall back to channel discovery).
  # Fails the cycle on: malformed pin, require_pin / require_explicit_rev
  # violation, channel resolution error, tarball download error.
  nixpkgs_path = resolve_pin(meta.pin, config.pins)

  # 7. Ensure hardware config exists
  if not exists(/etc/nixos/hardware-configuration.nix):
    exec("nixos-generate-config")

  # 8. Inventory queries (from meta.queries)
  if inventory.enabled and meta.queries not empty:
    results = run_dynamodb_queries(meta.queries)
    write("/var/lib/nixops3/inventory.json", results)

  # 9. Fetch secrets
  pull_secrets(role, hostname)             # writes to /run/nixops3/secrets/

  # 10. Generate configuration.nix at the default NixOS location
  write("/etc/nixos/configuration.nix", generate_config(target))

  # 11. Rebuild — pass -I nixos-config= explicitly because systemd strips
  # NIX_PATH; the /etc/nixos location still helps for manual debugging
  # from an interactive shell (where NIX_PATH is set).
  # -I nixpkgs= comes from resolve_pin above (pinned path), or from
  # channel discovery (Loose tier fallback).
  result = exec("nixos-rebuild switch \
    -I nixos-config=/etc/nixos/configuration.nix \
    -I nixops3=/var/lib/nixops3/commits/<target> \
    -I nixpkgs=<nixpkgs_path or discovered>")

  # 10. On success, atomically advance the symlink
  if result.success:
    ln -sfn commits/<target> /var/lib/nixops3/current.tmp
    mv /var/lib/nixops3/current.tmp /var/lib/nixops3/current
    write heartbeat (status: ok, applied_sha=target, target_sha=target)
    log "apply succeeded"
    prune_trees()
  else:
    log "apply failed: " + result.stderr
    write heartbeat (status: failed, applied_sha=<unchanged>, target_sha=target)
    # do NOT advance symlink — next cycle will retry
```

## Config Tree Download

Given `target = <sha>`:

- `list_prefix("commits/<sha>/")` → all object keys under the prefix
- GET each key in parallel (bounded concurrency, e.g. 8 at a time)
- Write each to `/var/lib/nixops3/<key>` (preserving the path structure)

Extraction is to a temp dir first (`commits/.tmp-<sha>-<pid>/`), then renamed
to `commits/<sha>/` on completion. Interrupted downloads leave a `.tmp-*` dir
that is cleaned up at the start of the next cycle (any `commits/.tmp-*` older
than one cycle is removed).

No import scanning. No selective fetching. The full tree is fetched or
nothing is. If any GET fails, the cycle fails and the `.tmp-*` dir is
discarded on the next cycle.

## Symlink Advance

The symlink is the state. Advancing it atomically:

```
ln -sfn commits/<sha> /var/lib/nixops3/current.tmp
mv /var/lib/nixops3/current.tmp /var/lib/nixops3/current
```

`rename(2)` on the same filesystem is atomic per POSIX. `/var/lib/nixops3/`
must be a single filesystem; `commits/` and `current` on separate mounts
would break atomicity. Standard NixOS installs satisfy this trivially.

## Tree Pruning

After each successful apply:

1. List `/var/lib/nixops3/commits/*/` (excluding `.tmp-*`)
2. Sort by mtime descending
3. Keep the newest `trees_retain` (default 5)
4. Delete the rest — **never** delete the symlink target, even if it falls
   outside the retention window

Pruning only runs after a successful apply. Failed cycles never touch existing
trees.

## configuration.nix Generation

Written per cycle to `/etc/nixos/configuration.nix` — the standard NixOS
location. The daemon still passes `-I nixos-config=` explicitly (systemd
strips `NIX_PATH`, without which `nixos-rebuild` can't find the "default"
location). The standard location helps manual debugging from an interactive
shell: `sudo nixos-rebuild switch -I nixops3=/var/lib/nixops3/current` Just
Works because interactive shells have `NIX_PATH` set with `nixos-config=`
pointing at `/etc/nixos/configuration.nix`.

The daemon overwrites this file every cycle. On a nixops3-managed host,
`/etc/nixos/configuration.nix` is not user-editable — S3 is the source of
truth, and any manual edit is lost on the next poll.

Contents:

1. `/etc/nixos/hardware-configuration.nix` — absolute path, first; omitted if
   still absent after `nixos-generate-config` ran
2. `<commit-tree>/roles/<role>/main.nix` — absolute path into the extracted
   commit tree
3. `<commit-tree>/roles/<role>/<hostname>/main.nix` — absolute path; only if
   the file exists in the tree

Plus a default bootloader guard to satisfy the NixOS bootloader assertion on
fresh machines. Roles that manage a real bootloader must override with
`lib.mkForce`.

```nix
# Generated by nixops3 — do not edit manually
{ lib, ... }:
{
  imports = [
    /etc/nixos/hardware-configuration.nix
    /var/lib/nixops3/commits/abc1234/roles/home/production/webserver/main.nix
    /var/lib/nixops3/commits/abc1234/roles/home/production/webserver/web-01.waldman.internal/main.nix
  ];

  boot.loader.grub.device = lib.mkDefault "nodev";
}
```

Profile imports (`<nixops3/profiles/...>`) resolve via `-I nixops3=` — the
daemon does not emit profile imports itself.

## nixos-rebuild Invocation

```sh
nixos-rebuild switch \
  -I nixos-config=/etc/nixos/configuration.nix \
  -I nixops3=/var/lib/nixops3/commits/<sha> \
  -I nixpkgs=<discovered path>          # optional; see nixpkgs discovery below
```

`-I nixos-config=` is passed explicitly because systemd strips `NIX_PATH`.
Without it, `nixos-rebuild` fails with `file 'nixos-config' was not found in
the Nix search path`. The file lives at the standard `/etc/nixos/` location so
manual debugging from an interactive shell (where `NIX_PATH` is populated)
Just Works without needing to know the path.

`-I nixops3=` registers the extracted commit tree as a Nix path, resolving
role imports like `<nixops3/profiles/base.nix>`.

`-I nixpkgs=` source depends on the pinning tier (see spec 09):

- **Loose** (no `pin:` block): path comes from channel discovery — see below.
- **Floating** / **Pinned**: path is `/var/lib/nixops3/nixpkgs/<rev>/`,
  materialized by the daemon per spec 09.

### nixpkgs Channel Discovery (Loose tier fallback)

For the Loose tier only. The daemon searches, stopping at the first hit:

1. `NIX_PATH` environment variable
2. `/etc/set-environment` (NixOS activation output)
3. `/nix/var/nix/profiles/per-user/root/channels/nixos`
4. `/nix/var/nix/profiles/per-user/root/channels/nixpkgs`

If none found, `-I nixpkgs=` is omitted and `nixos-rebuild` must locate
nixpkgs itself. This is likely to fail on freshly provisioned machines
where `sudo` strips `NIX_PATH` — which is why pinning (spec 09) exists.

stdout and stderr are captured. On non-zero exit: log full stderr to journald,
do not advance the symlink.

## Jitter

Uniform random value in `[0, 60)` seconds added to `poll_interval_secs` each
cycle. Prevents thundering herd on large fleets started simultaneously.

## Hostname Resolution

The daemon reads the machine hostname from `/proc/sys/kernel/hostname`. It
does **not** call `hostname --fqdn` because that binary is not in systemd's
restricted `PATH`.

The hostname is resolved once at daemon startup and reused for the lifetime
of the process. If the hostname changes (e.g. via a config apply that sets
`networking.hostName`), the daemon must be restarted to pick it up.

## Startup Behaviour

**Daemon mode** — on first start (no local symlink): skip the sleep, apply
immediately, then enter the normal poll loop.

**Single-shot mode** — always runs the cycle immediately (no sleep), regardless
of whether a symlink exists.

## Concurrency

Both daemon and single-shot modes acquire an exclusive `flock(2)` on
`/var/lib/nixops3/.lock` before starting a cycle. This prevents surprising
interactions when an operator runs `nixops3` in a shell while `nixops3
--daemon` is running under systemd. The second invocation blocks until the
first completes, then runs its own cycle.

## Logging

All output goes to journald via stderr. Log lines are plain text, prefixed
with severity.

Log level policy:
- **debug** — routine state (canary 404s that are expected, "already at target")
- **info** — meaningful transitions (starting apply, apply succeeded, symlink advanced)
- **warn** — unusual but recoverable (unexpected 404 on a known key, slow response)
- **error** — apply failures, invalid pointers, network errors that abort the cycle

```
DEBUG target sha unchanged (abc1234) — no-op
DEBUG no canary.txt for role — proceeding
INFO  fetching commits/abc1234/ (12 objects)
INFO  running nixos-rebuild switch
INFO  apply succeeded in 47s
INFO  symlink advanced: current -> commits/abc1234
INFO  canary active, hostname not listed — skipping
ERROR apply failed: <nixos-rebuild stderr>
ERROR invalid target sha: "not-a-hex-string"
ERROR s3 download failed: <error>
```

No structured logging in v1. No log rotation required (journald handles it).

## Systemd Unit

The daemon is managed by a systemd system service (not user service — it runs
as root):

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

The `/run/nixops3/` tmpfs directory must exist before the daemon starts.
Create it via systemd-tmpfiles:

```
d /run/nixops3         0700 root root -
d /run/nixops3/secrets 0700 root root -
```
