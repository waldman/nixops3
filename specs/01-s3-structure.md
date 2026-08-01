# S3 Structure

## Bucket Layout

```
s3://<bucket>/
  current                                     # sha pointer to the promoted commit
  commits/<sha>/                              # per-commit tree, published once by CI
    profiles/
      base.nix
      nixops3.nix
      users.nix
    roles/
      <abstraction>/                          # free-form: "home", "aws-us-east-1"
        <environment>/                        # "production", "staging"
          <role>/                             # "webserver", "generic_node"
            main.nix                          # role entry point
            main.yaml                         # role metadata (optional)
            canary.txt                        # optional; role-scoped gate (spec 03)
            files/                            # static files (referenced via builtins.readFile)
            templates/                        # nix-interpolated files (convention, no daemon logic)
            hosts/                            # per-host overrides (optional)
              <fqdn>/
                main.nix                      # host-specific NixOS overrides
                main.yaml                     # host-specific metadata (optional)
                files/                        # host-specific static files
```

Only two things live at the bucket root: the mutable `current` pointer and
the `commits/` prefix containing one immutable-by-convention tree per git
commit. No other objects, no other prefixes.

## Objects

### `current`

Plain text file containing the git sha of the promoted commit.

- Exactly 40 hex characters (SHA-1 git sha), optionally followed by whitespace
- Trailing newline permitted; anything else that isn't hex is rejected
- This is the ONLY object that gets overwritten during normal operation
- Rollback: `echo <old-sha> | aws s3 cp - s3://<bucket>/current`

### `commits/<sha>/`

A per-commit tree. `<sha>` is the git sha of the config repository commit
CI built from — human-readable, greppable, `git show`-able.

**Immutability contract:**

- CI populates `commits/<sha>/` once via `aws s3 sync`
- After publish, the ONLY mutation permitted is the operator deleting a
  `canary.txt` file inside a role directory (see spec 03 — canary promotion)
- Nothing else in `commits/<sha>/` may be modified, added, or deleted
- Old commits are pruned by lifecycle rules (out of spec scope)

**Layout** mirrors the source repository:

- `profiles/*.nix` — profile modules importable by roles
- `roles/<abstraction>/<environment>/<role>/main.nix` — role entry point
- `roles/<abstraction>/<environment>/<role>/canary.txt` — optional gate
- `roles/<abstraction>/<environment>/<role>/files/` — static files (see below)
- `roles/<abstraction>/<environment>/<role>/hosts/<fqdn>/main.nix` — host overrides (optional)

## CI Publish Contract

On merge to the config repo's release branch, CI performs these steps in order:

1. `aws s3 sync . s3://<bucket>/commits/<sha>/`
2. Wait for sync completion
3. `aws s3 cp - s3://<bucket>/current` with `<sha>`

**Order is critical.** `current` must be flipped LAST, only after the full
tree is uploaded. Daemons only ever fetch from `commits/<sha>/` after seeing
`<sha>` in `current`. This guarantees they never observe a partial tree.

Torn-read hazard: none. The pointer flip is a single atomic S3 PUT.

## Hierarchy Levels

| Level | Example | Purpose |
|-------|---------|---------|
| abstraction | `home`, `aws-us-east-1` | Infrastructure or organisational boundary |
| environment | `production`, `staging` | Deployment phase |
| role | `webserver`, `generic_node` | Machine function |
| hostname | `web-01.example.internal` | Individual host overrides |

The abstraction level is intentionally free-form. The daemon does not
interpret its semantics; it treats the concatenated path as an S3 prefix.

## Magic Filenames

| Filename / Dir | Location | Purpose |
|----------------|----------|---------|
| `main.nix` | role dir, host dir | NixOS config entry point |
| `main.yaml` | role dir, host dir | Role/host metadata: pin, queries (see spec 08) |
| `canary.txt` | role dir | Role-scoped canary gate (see spec 03) |
| `files/` | role dir, host dir | Static files; referenced by `.nix` via `builtins.readFile` |
| `templates/` | role dir, host dir | Nix-interpolated files; convention only, no daemon logic |
| `hosts/` | role dir | Container for per-host subdirectories |

Anything else in the tree is downloaded but has no special meaning to the daemon.

Note: `queries.toml` from v0.3 is superseded by the `queries:` section of
`main.yaml` (spec 08). v0.4 daemons do not read `queries.toml`.

## Profile Selection

The daemon downloads **the entire `commits/<sha>/` tree** on each commit
transition. There is no selective fetching, no import scanner. The tradeoff:

- **Cost:** a homelab tree is tiny (dozens of files, tens of KB). Bandwidth is
  negligible.
- **Benefit:** simpler daemon; every host has the full config locally
  (`ls /var/lib/nixops3/current/roles/` is a debugging superpower).

Roles import profiles using `<nixops3/profiles/...>`:

```nix
imports = [
  <nixops3/profiles/nixops3.nix>
  <nixops3/profiles/users.nix>
];
```

The daemon invokes `nixos-rebuild` with `-I nixops3=/var/lib/nixops3/commits/<sha>/`
so these imports resolve to the current commit tree. See spec 02.

## Role and Host Entry Points

Every apply cycle considers:

1. `commits/<sha>/roles/<role>/main.nix` — role entry point (required)
2. `commits/<sha>/roles/<role>/hosts/<hostname>/main.nix` — host overrides (optional)

The daemon's generated `configuration.nix` imports both from the local
extraction of the commit tree.

## files/ and templates/ Convention

`files/` and `templates/` are naming conventions for the Nix layer, not
interpreted by the daemon. The daemon syncs the full commit tree; `.nix`
files then reference these paths via standard Nix built-ins:

```nix
# In a role's main.nix — reference a file next to it in S3:
configFile = pkgs.writeText "app-config" (builtins.readFile ./files/config.yaml);
```

`templates/` holds `.nix` files whose content is parameterised with Nix
string interpolation rather than copied verbatim. The distinction is
documentation-level only; both live in the same synced tree.

Host-level `files/` works the same way from within `hosts/<fqdn>/main.nix`.

## NixOS Import Conventions

### Role `main.nix`

```nix
{ ... }:
{
  imports = [
    <nixops3/profiles/base.nix>
    <nixops3/profiles/users.nix>
  ];

  networking.hostName = "web-server";
  services.openssh.enable = true;
}
```

### Host `main.nix`

```nix
{ ... }:
{
  networking.hostName = "web-01.example.internal";

  users.users.local-admin = {
    isNormalUser = true;
    openssh.authorizedKeys.keys = [ "ssh-ed25519 AAAA..." ];
  };
}
```

Host `main.nix` does not import the role — the daemon-generated
`configuration.nix` imports both independently.

## Generated configuration.nix

The daemon writes an ephemeral `configuration.nix` per apply cycle. See spec 02
for the full generation rules and file placement.
