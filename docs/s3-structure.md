# S3 Bucket Structure

## Layout

```
s3://your-bucket/
  current                                        # sha pointer to promoted commit

  commits/<git-sha>/                             # one prefix per promoted commit
    profiles/                                    # shared profile library
      base.nix
      docker.nix
      monitoring.nix
    roles/
      home/                                      # abstraction
        production/                              # environment
          webserver/                             # role
            main.nix
            canary.txt                           # optional; role-scoped gate
            queries.toml                         # optional DynamoDB queries
            web-01.example.com/                  # host
              main.nix
          zookeeper/
            main.nix
            zk-01.example.com/
              main.nix
```

## Two things at bucket root

That's it — one pointer, one prefix. Everything else is inside `commits/<sha>/`.

- **`current`** — plain text file containing a 40-character git sha (trailing
  newline OK). This is the ONLY mutable object in normal operation. Rollback
  is `echo <old-sha> | aws s3 cp - s3://your-bucket/current`.

- **`commits/<sha>/`** — immutable per-commit tree. CI populates it once via
  `aws s3 sync`. The only permitted post-publish mutation is deleting a
  `canary.txt` file to promote past a canary gate (see [Canary Rollouts](canary.md)).

## Hierarchy levels

The role path encodes up to four levels of specificity:

| Level | Example | Purpose |
|-------|---------|---------|
| abstraction | `home`, `aws-us-east-1`, `datacenter-a` | Infrastructure or organisational boundary |
| environment | `production`, `staging`, `dev` | Deployment phase |
| role | `webserver`, `zookeeper`, `miner` | Machine function |
| hostname | `web-01.example.com` | Individual host overrides |

The abstraction level is intentionally free-form. Use it to group machines by
datacenter, cloud provider, business unit, or however your infrastructure is
organised. The daemon does not interpret its meaning.

## Magic filenames

The daemon recognises three special filenames inside a commit tree.
Everything else is downloaded but has no special meaning.

| Filename | Location | Purpose |
|----------|----------|---------|
| `main.nix` | role or host directory | Entry point for that role or host |
| `queries.toml` | role or host directory | DynamoDB queries to run before rebuild |
| `canary.txt` | role directory | Role-scoped canary gate |

## How profiles are discovered

The daemon downloads the **entire** `commits/<sha>/` tree on each commit
transition, not just the profiles referenced by a role. This is cheap at
homelab scale (dozens of files, tens of KB) and gives you the whole config
locally to inspect (`ls /var/lib/nixops3/current/`).

Roles import profiles using `<nixops3/...>`:

```nix
# roles/home/production/webserver/main.nix
{ ... }:
{
  imports = [
    <nixops3/profiles/base.nix>
    <nixops3/profiles/nginx.nix>
  ];
  ...
}
```

The daemon passes `-I nixops3=/var/lib/nixops3/commits/<sha>/` to
`nixos-rebuild`, so these imports resolve to the current commit tree.

## Profile directory conventions

Although the daemon downloads whatever is under `commits/<sha>/`, the
following layout is recommended for clarity:

```
commits/<sha>/
  profiles/                     # universal — suitable for any machine
  roles/
    <abstraction>/
      profiles/                 # abstraction-scoped
      <environment>/
        profiles/               # environment-scoped
```

A role references whichever levels it needs:

```nix
imports = [
  <nixops3/profiles/base.nix>                             # global
  <nixops3/roles/home/profiles/homelab-network.nix>       # abstraction-scoped
  <nixops3/roles/home/production/profiles/hardening.nix>  # environment-scoped
];
```

## Canary file (role-scoped)

`canary.txt` inside a role directory holds back all other hosts of that role.
Plain text, one FQDN per line, blank lines and `#` comments ignored.

```
# Testing the new nginx config
web-01.example.com
```

When present in `commits/<sha>/roles/home/production/webserver/canary.txt`,
only `web-01.example.com` applies. Other webserver hosts skip and report
`canary_skip` in inventory. **Other roles are unaffected** — canarying
webserver does not hold back generic_node.

To promote past the canary, delete the file:

```bash
sha=$(aws s3 cp s3://your-bucket/current -)
aws s3 rm s3://your-bucket/commits/$sha/roles/home/production/webserver/canary.txt
```

See [Canary Rollouts](canary.md) for the full workflow.

## Publishing to S3

CI publishes each merge as an immutable commit tree, then flips the pointer:

```bash
# 1. Populate the immutable prefix
aws s3 sync ./ s3://your-bucket/commits/$GIT_SHA/ \
  --exclude ".git/*" --exclude ".github/*"

# 2. Flip the pointer (fleet-wide rollout trigger)
echo "$GIT_SHA" | aws s3 cp - s3://your-bucket/current
```

**Order matters.** `current` must be flipped last, only after the sync
completes. Daemons only fetch from `commits/<sha>/` after seeing `<sha>` in
`current`, so they never observe a partial tree.

## Rollback

One command:

```bash
echo "$OLD_SHA" | aws s3 cp - s3://your-bucket/current
```

Every unpinned host rolls back to the previous commit on next poll. The old
tree is still in S3 (was never overwritten), so no republish is needed.

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

For machines that only manage their own role, tighten to specific prefixes.
Note that machines need to `GetObject` the `current` pointer plus everything
under `commits/*/` for their role's paths:

```json
{
  "Effect": "Allow",
  "Action": ["s3:GetObject"],
  "Resource": [
    "arn:aws:s3:::your-bucket/current",
    "arn:aws:s3:::your-bucket/commits/*"
  ]
},
{
  "Effect": "Allow",
  "Action": ["s3:ListBucket"],
  "Resource": "arn:aws:s3:::your-bucket",
  "Condition": {
    "StringLike": { "s3:prefix": ["commits/*"] }
  }
}
```

The operator (person doing promotion, rollback, or canary removal) needs
write access to `current` and `commits/*/roles/*/canary.txt` — typically the
operator uses admin credentials rather than a scoped policy.
