# Architecture

## Overview

NixOpS3 is built around one insight: S3 is an excellent configuration control
plane. It is versioned, highly available, cheaply scalable to any fleet size,
and already part of most infrastructure stacks.

```
┌──────────────────────────────────────────────────────────────┐
│  Git repository (source of truth)                             │
│  profiles/  roles/                                            │
└──────────────────────┬───────────────────────────────────────┘
                       │  CI/CD on merge:
                       │    1. aws s3 sync . commits/<sha>/
                       │    2. aws s3 cp - current    (with <sha>)
                       ▼
┌──────────────────────────────────────────────────────────────┐
│  S3 Bucket  (s3://nixops3-<name>/)                           │
│    current                     (sha pointer)                  │
│    commits/<sha>/              (immutable per-commit tree)   │
│      profiles/  roles/                                        │
└──────────────────────┬───────────────────────────────────────┘
                       │  HTTP GET — poll every N seconds + jitter
                       ▼
┌──────────────────────────────────────────────────────────────┐
│  nixops3 --daemon  (runs as root on each machine)             │
│                                                              │
│  /etc/nixops3/nixops3.toml            (config)                │
│  /var/lib/nixops3/commits/<sha>/      (downloaded trees)      │
│  /var/lib/nixops3/current             (symlink → last apply)  │
│  /etc/nixos/configuration.nix         (generated per cycle)   │
│  /run/nixops3/secrets/                (tmpfs — resets)        │
│  /var/lib/nixops3/inventory.json      (query results)         │
└──────┬───────────────────────────────┬───────────────────────┘
       │                               │
       ▼                               ▼
nixos-rebuild switch           AWS services (optional)
                               ├─ DynamoDB   (inventory)
                               └─ Secrets Manager
```

## Poll cycle

Each poll cycle follows this sequence:

1. **Resolve target** — GET `s3://<bucket>/current`. Validate the value is a
   40-character hex sha; on failure log and skip the cycle.

2. **No-op check** — if `readlink /var/lib/nixops3/current` already resolves
   to `commits/<target>`, write heartbeat and continue. No further fetches.

3. **Canary gate** — GET `commits/<target>/roles/<role>/canary.txt`. If
   present and this host's FQDN is not listed, write `status: canary_skip`
   heartbeat and stop. If absent (404) or hostname listed, proceed.

4. **Fetch tree** — list all objects under `commits/<target>/` and download
   them in parallel into `/var/lib/nixops3/commits/<target>/`. Extraction is
   to a `.tmp-*` dir first, then renamed on completion.

5. **Hardware config** — if `/etc/nixos/hardware-configuration.nix` is
   missing, run `nixos-generate-config`.

6. **Inventory queries** — if enabled, run DynamoDB queries defined in
   `queries.toml` files inside the commit tree and write
   `/var/lib/nixops3/inventory.json`.

7. **Secrets** — list and fetch from AWS Secrets Manager; write to
   `/run/nixops3/secrets/`.

8. **Generate configuration.nix** — write `/etc/nixos/configuration.nix`
   (the standard NixOS location) importing the hardware config, the role's
   `main.nix`, and the host's `main.nix` (if present) from the extracted
   commit tree.

9. **Rebuild** — `nixos-rebuild switch -I nixos-config=/etc/nixos/configuration.nix -I nixops3=/var/lib/nixops3/commits/<target> [-I nixpkgs=...]`. The `-I nixos-config=` is passed explicitly because systemd strips `NIX_PATH`.

10. **Advance symlink** — on success only, atomically repoint
    `/var/lib/nixops3/current` at `commits/<target>` via
    `symlink → rename`. On failure the symlink is untouched; the next cycle
    will retry against the same target.

11. **Heartbeat** — write DynamoDB item with `applied_sha` (symlink target)
    and `target_sha` (resolved from `current`). Convergence is
    `applied_sha == target_sha`.

## Design decisions

### Pull not push

The daemon pulls on a timer. There is no inbound network connection to
managed machines and no master that tracks state. A machine that is powered
off simply applies the accumulated changes when it comes back up — it will
converge to whatever `current` points at.

### S3 as the only shared state

The only shared infrastructure is an S3 bucket (and optionally a DynamoDB
table). There is no Puppet master, Salt master, or Ansible control node to
operate, scale, or back up.

### Immutable-by-convention commit trees

Each promoted config lives at `commits/<git-sha>/`, populated once by CI and
treated as immutable thereafter. The only permitted mutation is the operator
deleting a `canary.txt` file to promote past a canary gate (see canary.md).

Fleet-wide state is a single object: `current`, a pointer to the currently-
promoted sha. Rollback is one `aws s3 cp` command.

### Symlink as state store

`/var/lib/nixops3/current` is a symlink pointing at the commit tree of the
last successful apply. `readlink` is the complete answer to "what is this
box running". There is no state file, no last-hash — the symlink is the
only truth.

Symlink replacement is atomic via `rename(2)` on the same filesystem, so a
concurrent reader either sees the old sha or the new sha, never a missing
symlink.

### NixOS module system does the merging

Profiles are standard NixOS modules. The daemon does no custom merging — it
generates a `configuration.nix` with an `imports` list and hands off to
`nixos-rebuild`. The NixOS module system handles merge semantics, conflict
detection, and `lib.mkDefault`/`lib.mkForce` priorities.

### No selective fetching

The daemon downloads the entire `commits/<sha>/` tree, not just what a role
imports. This is slightly more bandwidth in exchange for a much simpler
daemon and the ability to inspect the whole config on any host (`ls
/var/lib/nixops3/current/roles/`).

### Tmpfs for ephemeral state

`/run/nixops3/` is on tmpfs. Secrets and the generated `configuration.nix`
are cleared on every reboot. On first boot (no local symlink), the daemon
applies immediately without sleeping.

## Relationship to Puppet concepts

| Puppet | NixOpS3 |
|--------|---------|
| Puppet master | S3 bucket |
| Puppet agent | `nixops3` binary |
| Module | NixOS module (from nixpkgs or custom) |
| Profile | `.nix` file in `profiles/` |
| Role | `roles/<path>/main.nix` |
| Environment | prefix in the role path |
| Hiera | S3 path hierarchy + `lib.mkDefault` |
| PuppetDB `search()` | DynamoDB + `builtins.fromJSON` |
| `puppet agent -t` | `nixos-rebuild switch` |
| Deploying a change | `git push` → CI builds commit tree → flips `current` |
| Rollback | one `aws s3 cp` to overwrite `current` |
