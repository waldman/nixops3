# NixOpS3 — System Overview

## What It Is

NixOpS3 is a pull-based NixOS configuration management system. Each managed machine runs a daemon (`nixops3d`) that periodically fetches its NixOS configuration from an S3 bucket and applies it via `nixos-rebuild switch`.

S3 is the control plane. There is no master server.

## Design Principles

- **Pull-based**: machines fetch their config; no push, no master
- **S3 as data bus**: versioned, highly available, infinitely scalable
- **Path as data**: the S3 path encodes the machine's identity (`<abstraction>/<environment>/<role>`)
- **NixOS module system does the merging**: no custom merge logic; Nix handles it
- **Static binary**: the daemon is a single musl-linked Rust binary with no runtime dependencies
- **Opt-in features**: inventory reporting and secrets are disabled by default

## Goals

- Manage NixOS machines declaratively from a central S3 bucket
- Support hierarchical configuration inheritance (profiles → roles → hosts)
- Enable controlled rollouts via canary mechanism
- Provide optional node inventory via DynamoDB (heartbeat + search queries)
- Integrate with AWS Secrets Manager for runtime secrets
- Scale from 1 machine to hundreds of thousands without architectural changes

## Non-Goals (v1)

- Dynamic host provisioning (role/hostname derived from hardware fingerprint)
- Binary cache management
- Multi-cloud or non-S3 backends
- Web dashboard (data is in DynamoDB; consumers build their own)
- Automatic canary promotion

## System Components

```
┌─────────────────────────────────────────────────────┐
│  Git Repository (source of truth)                    │
│  profiles/ · nix-roles/ · specs/                    │
└────────────────┬────────────────────────────────────┘
                 │ CI/CD: aws s3 sync on merge
                 ▼
┌─────────────────────────────────────────────────────┐
│  S3 Bucket  (s3://nixops3-<org>/)                   │
│  profiles/ · nix-roles/ · canary.txt                │
└────────────────┬────────────────────────────────────┘
                 │ poll every N seconds + jitter
                 ▼
┌─────────────────────────────────────────────────────┐
│  nixops3d  (daemon, runs as root on each machine)   │
│  /etc/nixops3/nixops3.toml                          │
│  /var/lib/nixops3/current/  (working dir)           │
│  /run/nixops3/  (tmpfs: last-hash, secrets/)        │
└──────┬──────────────────────┬───────────────────────┘
       │                      │
       ▼                      ▼
nixos-rebuild switch     DynamoDB (optional)
                         AWS Secrets Manager (optional)
```

## Technology Stack

| Component | Technology |
|-----------|-----------|
| Daemon | Rust, compiled to `x86_64-unknown-linux-musl` |
| Config format | TOML |
| S3 interaction | `aws-sdk-rust` |
| DynamoDB | `aws-sdk-rust` |
| YAML parsing | `serde_yaml` (canary.txt) |
| TOML parsing | `toml` crate |
| Hashing | SHA-256 via `sha2` |

## Relationship to Puppet Concepts

| Puppet | NixOpS3 |
|--------|---------|
| Puppet master | S3 bucket |
| Puppet agent | `nixops3d` daemon |
| Module | NixOS module (from nixpkgs or custom) |
| Profile | `.nix` file in `profiles/` |
| Role | `nix-roles/<path>/main.nix` |
| Hiera | S3 path hierarchy + NixOS `lib.mkDefault` |
| PuppetDB search() | DynamoDB inventory + `builtins.fromJSON` |
| `puppet agent -t` | `nixos-rebuild switch` |
