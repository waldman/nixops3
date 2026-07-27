# Architecture

## Overview

NixOpS3 is built around one insight: S3 is an excellent configuration control plane. It is versioned, highly available, cheaply scalable to any fleet size, and already part of most infrastructure stacks.

```
┌──────────────────────────────────────────────────────────────┐
│  Git repository (source of truth)                             │
│  profiles/  roles/  canary.txt                               │
└──────────────────────┬───────────────────────────────────────┘
                       │  CI/CD: aws s3 sync on merge to main
                       ▼
┌──────────────────────────────────────────────────────────────┐
│  S3 Bucket  (s3://nixops3-<name>/)                           │
│  profiles/  roles/  canary.txt                               │
└──────────────────────┬───────────────────────────────────────┘
                       │  HTTP GET — poll every N seconds + jitter
                       ▼
┌──────────────────────────────────────────────────────────────┐
│  nixops3d  (daemon, runs as root on each machine)            │
│                                                              │
│  /etc/nixops3/nixops3.toml        (config)                   │
│  /var/lib/nixops3/current/        (downloaded .nix files)    │
│  /run/nixops3/last-hash           (tmpfs — resets on reboot) │
│  /run/nixops3/secrets/            (tmpfs — resets on reboot) │
│  /var/lib/nixops3/inventory.json  (query results)            │
└──────┬───────────────────────────────┬───────────────────────┘
       │                               │
       ▼                               ▼
nixos-rebuild switch           AWS services (optional)
                               ├─ DynamoDB   (inventory)
                               └─ Secrets Manager
```

## Poll cycle

Each poll cycle follows this sequence:

1. **Canary check** — fetch `canary.txt` from S3. If present and this machine's hostname is not listed, skip the cycle and write `status: canary_skip` to inventory.

2. **Download** (two passes):
   - Fetch `roles/<role>/main.nix` and (if present) the host `main.nix`.
   - Scan both for `<nixops3/...>` import references.
   - Fetch exactly those profile files — nothing more.
   - Fetch `queries.toml` files for inventory queries.

3. **Hash check** — compute SHA-256 of all downloaded `.nix` files sorted by S3 key. Compare to `/run/nixops3/last-hash`. If identical, write heartbeat and sleep — no rebuild.

4. **Inventory queries** — if enabled, run DynamoDB queries defined in `queries.toml` and write `/var/lib/nixops3/inventory.json`.

5. **Secrets** — list and fetch secrets from AWS Secrets Manager; write to `/run/nixops3/secrets/`.

6. **Generate `configuration.nix`** — write `/var/lib/nixops3/current/configuration.nix` with three imports: hardware config, role main.nix, host main.nix.

7. **Rebuild** — run `nixos-rebuild switch -I nixos-config=... -I nixops3=...`.

8. **Outcome** — on success, write new hash and `status: ok` heartbeat. On failure, write `status: failed` heartbeat; next cycle retries.

## Design decisions

### Pull not push

The daemon pulls on a timer. There is no inbound network connection to managed machines and no master that tracks state. Machines are independently correct; a machine that is powered off simply applies the accumulated changes when it comes back up.

### S3 as the only shared state

The only shared infrastructure is an S3 bucket (and optionally a DynamoDB table). There is no Puppet master, Salt master, or Ansible control node to operate, scale, or back up.

### Hash-based idempotency

Every cycle computes a hash of the downloaded `.nix` files. A rebuild only happens when the hash changes. This makes polling cheap at steady state: just a few S3 GETs and a hash comparison.

The hash covers only `.nix` files. `queries.toml` changes and secret rotations do not trigger a rebuild on their own.

### NixOS module system does the merging

Profiles are standard NixOS modules. The daemon does no custom merging — it generates a `configuration.nix` with an `imports` list and hands off to `nixos-rebuild`. The NixOS module system handles merge semantics, conflict detection, and `lib.mkDefault`/`lib.mkForce` priorities.

### Tmpfs for ephemeral state

`/run/nixops3/` is on tmpfs. The last-applied hash and all secrets are cleared on every reboot. On first boot (no `last-hash`), the daemon applies immediately without sleeping. This is the correct behaviour for a newly provisioned machine.

## Relationship to Puppet concepts

| Puppet | NixOpS3 |
|--------|---------|
| Puppet master | S3 bucket |
| Puppet agent | `nixops3d` daemon |
| Module | NixOS module (from nixpkgs or custom) |
| Profile | `.nix` file in `profiles/` |
| Role | `roles/<path>/main.nix` |
| Hiera | S3 path hierarchy + `lib.mkDefault` |
| PuppetDB `search()` | DynamoDB + `builtins.fromJSON` |
| `puppet agent -t` | `nixos-rebuild switch` |
