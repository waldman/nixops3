# NixOpS3

Pull-based NixOS configuration management over S3. Each managed machine runs a daemon (`nixops3d`) that periodically fetches its NixOS configuration from an S3 bucket and applies it via `nixos-rebuild switch`.

S3 is the control plane. There is no master server.

## How it works

```
Git repository  ──CI/CD──▶  S3 bucket  ◀──poll──  nixops3d on each machine
(your .nix files)           (s3://your-bucket/)     └─▶ nixos-rebuild switch
```

You commit `.nix` files to a git repository. CI/CD syncs them to S3 (`aws s3 sync`). Each machine polls S3 every N minutes, detects changes via SHA-256 hash, and runs `nixos-rebuild switch` when something changed. If nothing changed, it's a no-op.

The S3 path encodes a machine's identity:

```
roles/<abstraction>/<environment>/<role>/main.nix        ← role config
roles/<abstraction>/<environment>/<role>/<hostname>/main.nix  ← host overrides
profiles/<name>.nix                                      ← shared profiles
```

## Features

- **No master server** — S3 is the only shared infrastructure
- **Hierarchical config** — global profiles → abstraction → environment → role → host
- **Profile-as-library** — profiles are NixOS modules; roles import only what they need
- **Canary rollouts** — apply changes to one host before the fleet via `canary.txt`
- **Secrets** — fetches from AWS Secrets Manager before each rebuild; never stored in S3
- **Fleet inventory** — optional DynamoDB heartbeat; query results available to `.nix` files via `builtins.fromJSON`
- **Static binary** — single `x86_64-unknown-linux-musl` Rust binary, no runtime dependencies

## Quickstart

### 1. Prerequisites

- An S3 bucket (e.g. `nixops3-myhomelab`)
- An IAM identity for each machine with S3 read access to that bucket
- `nixos-rebuild` available on the managed machine (it's a NixOS machine, so it is)

### 2. Build the daemon

```bash
# requires the musl target
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl

# binary at:
target/x86_64-unknown-linux-musl/release/nixops3d
```

Copy the binary to `/usr/local/bin/nixops3d` on each managed machine.

### 3. Organize your .nix files

```
your-repo/
  profiles/
    base.nix          ← shared across all machines
    docker.nix        ← pulled in by any role that needs Docker
  roles/
    home/
      production/
        webserver/
          main.nix    ← webserver role
          web-01.example.com/
            main.nix  ← host-specific overrides
```

Sync to S3:

```bash
aws s3 sync your-repo/ s3://nixops3-myhomelab/ --delete
```

### 4. Configure the daemon

Create `/etc/nixops3/nixops3.toml` on each machine (mode `0600`, owner `root:root`):

```toml
bucket = "nixops3-myhomelab"
region = "us-east-1"
role   = "home/production/webserver"
```

That's the minimal config. The machine will use its IAM role for AWS credentials and apply as soon as it detects a hash change.

### 5. Enable the systemd service

Add to your machine's NixOS config or drop the unit file manually:

```ini
[Unit]
Description=NixOpS3 configuration daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/nixops3d
Restart=on-failure
RestartSec=30s

[Install]
WantedBy=multi-user.target
```

```bash
systemctl enable --now nixops3d
```

The daemon applies immediately on first start (no initial sleep), then polls every `poll_interval_secs` seconds with a random jitter of up to 60 seconds.

## Configuration file

**Path**: `/etc/nixops3/nixops3.toml` — read at startup and reread each poll cycle.

```toml
# Required
bucket = "nixops3-myhomelab"
region = "us-east-1"
role   = "home/production/webserver"   # S3 path to role directory

# Optional (defaults shown)
poll_interval_secs = 600               # 10 minutes + up to 60s jitter

# AWS credentials — omit to use instance IAM role (recommended for EC2/ECS)
[aws]
access_key_id     = "AKIA..."
secret_access_key = "..."

# Fleet inventory via DynamoDB — disabled by default
[inventory]
enabled = true
table   = "nixops3-inventory"
```

## Writing roles and profiles

**Profile** (`profiles/base.nix`) — a reusable NixOS module:

```nix
{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [ curl git vim ];
  services.openssh.enable = true;
}
```

**Role** (`roles/home/production/webserver/main.nix`) — imports profiles, sets role options:

```nix
{ ... }:
{
  imports = [
    <nixops3/profiles/base.nix>
    <nixops3/profiles/nginx.nix>
  ];

  networking.hostName = "webserver";
  services.nginx.enable = true;
}
```

**Host override** (`roles/home/production/webserver/web-01.example.com/main.nix`):

```nix
{ ... }:
{
  networking.hostName = "web-01.example.com";
  # host-specific overrides here
}
```

The daemon scans `main.nix` for `<nixops3/...>` imports and downloads exactly those profile files — nothing else.

## Canary rollouts

To apply a change to one host before the fleet, upload `canary.txt` to the bucket root:

```
web-01.example.com
```

Only `web-01.example.com` applies the change. All other machines skip and write `status: canary_skip` to inventory. Delete the file to roll out to everyone.

## Documentation

- [Architecture](docs/architecture.md) — system design and components
- [S3 Structure](docs/s3-structure.md) — bucket layout, path conventions
- [Authoring Roles & Profiles](docs/authoring.md) — writing .nix files for NixOpS3
- [Configuration Reference](docs/configuration.md) — complete nixops3.toml reference
- [Canary Rollouts](docs/canary.md) — staged rollout workflow
- [Inventory & Queries](docs/inventory.md) — DynamoDB heartbeat and search queries
- [Secrets](docs/secrets.md) — AWS Secrets Manager integration
- [Bootstrap](docs/bootstrap.md) — Golden ISO and cloud-init
- [Operations](docs/operations.md) — monitoring, logs, troubleshooting

## License

MIT — see [LICENSE](LICENSE).
