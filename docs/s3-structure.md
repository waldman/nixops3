# S3 Bucket Structure

## Layout

```
s3://your-bucket/
  profiles/                                    # shared profile library
    base.nix
    docker.nix
    monitoring.nix
    roles/
      home/
        profiles/                              # abstraction-scoped profiles
          homelab-network.nix
        production/
          profiles/                            # environment-scoped profiles
            prod-hardening.nix
          webserver/                           # role
            main.nix
            queries.toml                       # optional DynamoDB queries
            web-01.example.com/               # host
              main.nix
          zookeeper/
            main.nix
            zk-01.example.com/
              main.nix

  canary.txt                                   # optional — controls staged rollouts
```

## Hierarchy levels

The role path encodes up to four levels of specificity:

| Level | Example | Purpose |
|-------|---------|---------|
| abstraction | `home`, `aws-us-east-1`, `datacenter-a` | Infrastructure or organisational boundary |
| environment | `production`, `staging`, `dev` | Deployment phase |
| role | `webserver`, `zookeeper`, `miner` | Machine function |
| hostname | `web-01.example.com` | Individual host overrides |

The abstraction level is intentionally free-form. Use it to group machines by datacenter, cloud provider, business unit, or however your infrastructure is organised. The daemon does not interpret its meaning.

## Magic filenames

The daemon recognises four special filenames. Everything else is ignored.

| Filename | Location | Purpose |
|----------|----------|---------|
| `main.nix` | role or host directory | Entry point for that role or host |
| `queries.toml` | role or host directory | DynamoDB queries to run before rebuild |
| `canary.txt` | bucket root | Controls which hosts apply on this cycle |

## How profiles are discovered

The daemon does **not** download all files from the `profiles/` directory. Instead, it scans the role's `main.nix` for `<nixops3/...>` Nix path references and downloads exactly those files:

```nix
# roles/home/production/webserver/main.nix
{ ... }:
{
  imports = [
    <nixops3/profiles/base.nix>          # downloads profiles/base.nix
    <nixops3/profiles/nginx.nix>         # downloads profiles/nginx.nix
  ];
  ...
}
```

This means adding a new profile to `profiles/` has no effect on any machine until a `main.nix` references it.

The same scan applies to host `main.nix` files — if a host needs an extra profile, it can import it directly.

## Profile directory conventions

Although the daemon will download any `<nixops3/...>` path, the following directory layout is recommended for clarity:

```
profiles/                    # universal — suitable for any machine
roles/
  <abstraction>/
    profiles/                # abstraction-scoped — for all machines in this group
    <environment>/
      profiles/              # environment-scoped — e.g. production hardening
```

A role references whichever levels it needs:

```nix
imports = [
  <nixops3/profiles/base.nix>                           # global
  <nixops3/roles/home/profiles/homelab-network.nix>     # abstraction-scoped
  <nixops3/roles/home/production/profiles/hardening.nix> # environment-scoped
];
```

## Canary file

`canary.txt` at the bucket root is a plain-text list of FQDNs, one per line. Blank lines and lines starting with `#` are ignored.

```
# Currently testing the new nginx config
web-01.example.com
```

When present, only the listed hosts apply the current config. All other hosts skip and report `canary_skip` in inventory. Delete the file to roll out to all machines.

See [Canary Rollouts](canary.md) for the full workflow.

## Syncing to S3

The typical CI/CD step is:

```bash
aws s3 sync ./your-repo/ s3://your-bucket/ --delete --exclude ".git/*"
```

The `--delete` flag removes files from S3 when they are deleted from the repo, which is important: a deleted profile is only removed from machines on the next rebuild cycle if it disappears from S3.

## S3 permissions

Each machine needs read access to the bucket. A minimal IAM policy:

```json
{
  "Effect": "Allow",
  "Action": ["s3:GetObject", "s3:ListBucket"],
  "Resource": [
    "arn:aws:s3:::your-bucket",
    "arn:aws:s3:::your-bucket/*"
  ]
}
```

For machines that only manage their own role, tighten to specific prefixes:

```json
{
  "Effect": "Allow",
  "Action": "s3:GetObject",
  "Resource": [
    "arn:aws:s3:::your-bucket/canary.txt",
    "arn:aws:s3:::your-bucket/profiles/*",
    "arn:aws:s3:::your-bucket/roles/home/production/webserver/*"
  ]
}
```
