# main.yaml — Role/Host Metadata

## Purpose

`main.yaml` consolidates all non-Nix metadata for a role or host. It sits
alongside `main.nix` in the same directory — the name symmetry is deliberate:

- `main.nix` — the NixOS configuration you want applied
- `main.yaml` — the nixops3-level metadata about how it should be applied

As of v0.4, `main.yaml` holds two sections:

- `pin` — nixpkgs pinning (spec 09)
- `queries` — DynamoDB inventory queries (was `queries.toml` in v0.3)

The format is extensible: future features add new top-level keys without a
schema break.

## Location

Three tiers of `main.yaml`, each optional:

| Path | Level | Applies to |
|---|---|---|
| `commits/<sha>/main.yaml` | fleet | Every host of every role |
| `commits/<sha>/roles/<abstraction>/<environment>/<role>/main.yaml` | role | Every host of one role |
| `commits/<sha>/roles/<abstraction>/<environment>/<role>/<hostname>/main.yaml` | host | One specific host |

All three are optional. An entirely absent `main.yaml` at all three levels
means no pin (falls back per spec 09) and no queries.

**Typical usage:**

- **Fleet-level pin, role/host overrides for exceptions.** The common case:
  everyone runs the same nixpkgs; one legacy host or role stays on an
  older channel while migrating. Put the fleet-wide pin at
  `commits/<sha>/main.yaml`; add per-host overrides only where needed.
- **Per-role queries.** DynamoDB queries usually make sense per-role
  (webserver queries zookeeper nodes; database queries nothing). Author
  them at role level.

## Format

Standard YAML. Empty file is valid (equivalent to `{}`).

```yaml
pin:
  nixpkgs:
    channel: nixos-25.05                    # required if pin present; string
    rev:     abc1234def567890abcdef123...   # optional; 40-char hex git sha

queries:
  zk_nodes:
    role_prefix: home/production/zookeeper
  kafka_nodes:
    role_prefix: home/production/kafka
```

**Type strictness.** The parser rejects loose YAML type coercion:

- `rev` must be a string. Quote it if it contains only digits.
- `channel` must be a string. Quote it if it could be misinterpreted (e.g. `no`, `on`, `off`).
- `role_prefix` must be a string.

## Merge Semantics

**Per-top-level-key, most-specific-wins across three tiers.** Layers apply in
order: fleet → role → host. For each top-level key (`pin`, `queries`, future
keys), the most-specific tier that defines the key wins; that tier's value
replaces any less-specific value **entirely** (whole-block replacement, no
deep-merge).

Formally: `effective[key] = host[key] || role[key] || fleet[key] || undefined`.

- No deep-merge. No list concatenation. No per-sub-field override.
- A tier that omits a key inherits from the next less-specific tier.
- A tier that defines a key wins over everything less specific.

**Example — fleet-wide pin with one canary host on newer nixpkgs:**

```yaml
# commits/<sha>/main.yaml                (fleet default)
pin:
  nixpkgs: { channel: nixos-25.05, rev: abc1234... }
```

```yaml
# commits/<sha>/roles/home/production/webserver/main.yaml   (role)
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }
# pin: omitted → inherits fleet's pin
```

```yaml
# commits/<sha>/roles/home/production/webserver/web-canary-01/main.yaml
pin:
  nixpkgs: { channel: nixos-25.11 }        # override just this host
# queries: omitted → inherits role's queries
```

**Effective config for `web-canary-01`:**

```yaml
pin:
  nixpkgs: { channel: nixos-25.11 }        # host wins on `pin`
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }   # from role
```

**Effective config for `web-02` (webserver role, no host YAML):**

```yaml
pin:
  nixpkgs: { channel: nixos-25.05, rev: abc1234... }     # from fleet
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }   # from role
```

Note that the host's `pin:` block in the canary example **entirely replaces**
whatever was inherited. `web-canary-01` isn't just overriding
`pin.nixpkgs.channel` — it's replacing the whole `pin:` block. There's no
`rev`, so the host becomes floating on 25.11 even though the fleet had a
pinned rev on 25.05. Whole-block replacement is a deliberate simplicity
trade: predictable > powerful.

This is a deliberate simplicity trade: predictable > powerful.

## Sections in v0.4

### `pin`

See spec 09 for full semantics. Summary: three tiers (Loose, Floating, Pinned)
selected by which fields are present.

### `queries`

Replaces `queries.toml` from v0.3.

**v0.3 format** (deprecated):
```toml
[[query]]
name        = "zk_nodes"
role_prefix = "home/production/zookeeper"
```

**v0.4 format** (in `main.yaml`):
```yaml
queries:
  zk_nodes:
    role_prefix: home/production/zookeeper
```

The `name` field is gone — it's now the map key. `role_prefix` remains
required. Additional query types can add new fields under each key without
a schema break.

**Merging still applies:** when queries appear in both role and host
`main.yaml`, host wins for the entire `queries:` block (whole replacement,
per the merge rules above). This differs from v0.3 where duplicate names
merged with host-wins-per-key. If a host wants to add one query to a role's
set, it must re-declare all of them.

## Migration from v0.3

`queries.toml` support is removed in v0.4. The daemon does not read it, and
its presence is not an error (silently ignored).

Migration is manual: rewrite each `queries.toml` as a `queries:` section in
the same directory's `main.yaml`. There is no automated migration tool.

Example: to migrate `roles/home/production/webserver/queries.toml`, add or
create `roles/home/production/webserver/main.yaml` with the equivalent
`queries:` block, then delete the `.toml`.

## What NOT to put in main.yaml

- **canary.txt** stays as its own file. Rationale: the `aws s3 rm` promotion
  workflow (spec 03) depends on canary living as its own atomic S3 object.
  Merging it into main.yaml would require in-place edits of the whole file,
  or a full commit rebuild, both worse than the current one-command promotion.
- **`nixops3.toml`** stays as a separate file on each machine, unchanged.
  It's the daemon's config (per-machine, not per-role), not fleet metadata.
