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

Same hierarchy as `main.nix`:

- `commits/<sha>/roles/<abstraction>/<environment>/<role>/main.yaml` — role level
- `commits/<sha>/roles/<abstraction>/<environment>/<role>/<hostname>/main.yaml` — host level

Both are optional. An entirely absent `main.yaml` at both levels means no
pin (falls back per spec 09) and no queries.

**No fleet-default in v1.** If you want the same pin fleet-wide, put it in
every role's `main.yaml`. A fleet-default key (`commits/<sha>/main.yaml`) can
be added in a later version if the need is real.

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

**Per-top-level-key, most-specific-wins.** If both role and host `main.yaml`
exist, each top-level key is resolved independently:

- If the host YAML has the key, the host's value replaces the role's entirely.
- If the host YAML omits the key, the role's value is used.
- No deep-merge. No list concatenation. Whole-block replacement per key.

**Example:**

```yaml
# roles/home/production/webserver/main.yaml
pin:
  nixpkgs: { channel: nixos-25.05, rev: abc1234... }
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }
```

```yaml
# roles/home/production/webserver/web-canary-01/main.yaml
pin:
  nixpkgs: { channel: nixos-25.11 }        # host on newer nixpkgs for testing
# queries: omitted → inherits role's queries
```

**Effective config for `web-canary-01`:**

```yaml
pin:
  nixpkgs: { channel: nixos-25.11 }        # host wins on `pin`
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }   # from role
```

Note that the host's `pin:` block **entirely replaces** the role's `pin:`.
The host isn't just overriding `pin.nixpkgs.channel` — it's replacing the
whole `pin:` block. In this example there's no `rev`, so the host also
loses the role's rev (i.e. becomes floating on 25.11). If the host wanted
to keep the rev semantics but change the channel, it would need to spell
both out.

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
