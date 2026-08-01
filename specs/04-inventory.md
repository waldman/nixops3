# Inventory — DynamoDB Heartbeat and Search Queries

## Purpose

When enabled, `nixops3` reports machine state to a DynamoDB table after each poll cycle. This provides:

- Fleet-wide visibility (last seen, status, IP, role)
- A queryable inventory used by `.nix` files via `builtins.fromJSON` (equivalent to Chef's `search()` or PuppetDB)

The feature is opt-in. Disabling it has no effect on config apply behaviour.

## DynamoDB Table Schema

**Table name**: configured in `nixops3.toml` (`inventory.table`)
**Partition key**: `hostname` (String)
**TTL attribute**: `ttl` (Number, Unix epoch seconds) — defaults to `now + 2 * poll_interval_secs`; overridable via `inventory.ttl_secs` in `nixops3.toml` (or `--ttl-days` in bootstrap)

### Item Attributes

| Attribute | Type | Source | Example |
|-----------|------|--------|---------|
| `hostname` | S | `/proc/sys/kernel/hostname` | `web-01.example.internal` |
| `machine_id` | S | `/etc/machine-id` | `a1b2c3d4...` |
| `role` | S | `nixops3.toml` | `home/production/webserver` |
| `iface` | S | primary network interface | `eth0` |
| `ip` | S | IP of primary interface | `192.168.15.50` |
| `network` | S | network/prefix (v1: always `"unknown"`) | `192.168.15.0/24` |
| `applied_sha` | S | basename of `/var/lib/nixops3/current` symlink target; `""` if no symlink yet | `abc1234...` |
| `target_sha` | S | value of `current` in S3 the daemon resolved this cycle | `abc1234...` |
| `pin_mode` | S | pinning tier used this cycle: `loose`, `floating`, or `pinned` (spec 09) | `pinned` |
| `nixpkgs_channel` | S | channel label from `pin.nixpkgs.channel`; `""` for Loose tier | `nixos-25.05` |
| `nixpkgs_rev` | S | concrete nixpkgs rev used; `""` for Loose tier | `abc1234...` |
| `last_run_status` | S | `ok`, `failed`, `canary_skip` | `ok` |
| `last_seen` | S | ISO 8601 UTC | `2026-07-26T21:00:00Z` |
| `ttl` | N | Unix epoch | `1753570800` |

### Primary Network Interface Detection

The daemon selects the interface with the default route (`ip route get 1.1.1.1`). If detection fails, `iface` and `ip` are set to `"unknown"`.

The `network` field (CIDR prefix) is not yet computed in v1. It is always written as `"unknown"`.

### TTL Configuration

The default TTL is `now + 2 × poll_interval_secs`. To override, set `inventory.ttl_secs` in `nixops3.toml`:

```toml
[inventory]
enabled  = true
table    = "nixops3-inventory"
ttl_secs = 1296000   # 15 days
```

The `--ttl-days` flag in the bootstrap command converts days to seconds and writes `ttl_secs` into the config file.

## IAM Policy Requirements

Each machine's IAM identity requires:

```json
{
  "Effect": "Allow",
  "Action": ["dynamodb:PutItem", "dynamodb:UpdateItem"],
  "Resource": "arn:aws:dynamodb:<region>:<account>:table/<table>",
  "Condition": {
    "ForAllValues:StringEquals": {
      "dynamodb:LeadingKeys": ["${aws:PrincipalTag/hostname}"]
    }
  }
},
{
  "Effect": "Allow",
  "Action": ["dynamodb:Query", "dynamodb:Scan"],
  "Resource": "arn:aws:dynamodb:<region>:<account>:table/<table>"
}
```

Machines can only write their own item but can read all items.

## Convergence Semantics

With the commit-pointer model (see spec 01), each heartbeat carries both
`applied_sha` (what the host actually ran) and `target_sha` (what the fleet
pointer said the host should be running).

| `applied_sha` vs `target_sha` | `last_run_status` | Meaning |
|-------------------------------|-------------------|---------|
| equal | `ok` | Converged — running the current fleet target |
| not equal | `ok` | Lagging — successful apply in progress, may catch up next cycle; also seen for a host that was gated and later ungated but hasn't polled yet |
| not equal | `failed` | Apply failed; symlink still on the previous sha; retry next cycle |
| not equal | `canary_skip` | Canary gate is active on this role and this host is not listed |

Downstream tooling can compute simple fleet health from a scan:

- Converged hosts: `applied_sha == target_sha AND last_run_status == "ok"`
- Lagging: `applied_sha != target_sha AND last_run_status IN ("ok", "canary_skip")`
- Broken: `last_run_status == "failed"`

## Search Queries — `main.yaml` `queries:` section

### Definition

Queries live in the `queries:` section of `main.yaml` at role or host level
(spec 08). Effective queries are the merged result of role and host YAML
per the rules in spec 08 (host `queries:` block, if present, replaces role's
entirely — no per-key merge).

```yaml
# roles/home/production/webserver/main.yaml
queries:
  zookeeper_nodes:
    role_prefix: home/production/zookeeper
```

**Fields per query (values under each name key):**

| Field | Required | Description |
|-------|----------|-------------|
| `role_prefix` | yes | S3 role path prefix; matches items where `role` starts with this value |

The query name is the map key (was the `name` field in v0.3's
`queries.toml`).

### v0.3 → v0.4 migration

v0.4 daemons do not read `queries.toml`. Rewrite each `queries.toml` as a
`queries:` section in the same directory's `main.yaml`. See spec 08.

### Query Merging

Queries follow the general `main.yaml` merge rule (spec 08): if the host's
`main.yaml` has a `queries:` section, it **replaces the role's `queries:`
block entirely**. There is no per-key merge (deliberate — see spec 08 for
rationale). A host that wants to add one query on top of the role's set
must re-declare all of them.

### DynamoDB Query Execution

For each query, the daemon performs a DynamoDB `Scan` with a `FilterExpression` on the `role` attribute:

```
FilterExpression: begins_with(#role, :prefix)
```

Results are filtered to items where `ttl > now` (alive nodes only).

### inventory.json Format

Written to `/var/lib/nixops3/inventory.json` before `nixos-rebuild switch`:

```json
{
  "generated_at": "2026-07-26T21:00:00Z",
  "queries": {
    "zookeeper_nodes": [
      {
        "hostname": "zk-01.example.internal",
        "ip": "192.168.15.10",
        "role": "home/production/zookeeper",
        "last_run_status": "ok"
      }
    ]
  }
}
```

### Usage in .nix Files

```nix
# profiles/zookeeper-client.nix
let
  inv = builtins.fromJSON (builtins.readFile /var/lib/nixops3/inventory.json);
  zkNodes = inv.queries.zookeeper_nodes;
in {
  services.zookeeper.servers = map (n: n.ip) zkNodes;
}
```

`builtins.readFile` and `builtins.fromJSON` are evaluated at `nix eval` time, which runs as part of `nixos-rebuild`. Because the daemon writes `inventory.json` before calling `nixos-rebuild`, the data is always fresh.

## Heartbeat Timing

The daemon writes to DynamoDB at the end of every poll cycle, regardless of
whether a rebuild occurred:

| Scenario | `last_run_status` | `applied_sha` | `target_sha` | `nixpkgs_*` |
|----------|-------------------|---------------|--------------|-------------|
| Symlink already at target sha (no-op) | `ok` | current symlink | resolved from `current` | not updated this cycle (kept from prior heartbeat if any) |
| Apply succeeded, symlink advanced | `ok` | new sha (equals target) | resolved from `current` | resolved for this cycle |
| Apply failed | `failed` | unchanged (old sha) | resolved from `current` | resolved for this cycle |
| Canary skip | `canary_skip` | unchanged | resolved from `current` | `""` (pin not evaluated) |
| S3 pointer fetch failed | `failed` | unchanged | `""` | `""` |
| Pin resolution failed (bad config, HTTP error) | `failed` | unchanged | resolved from `current` | `""` |

On any DynamoDB write failure: log the error to journald, continue.
Inventory failure must never block config apply.

## Detecting Dead Nodes

The TTL attribute causes DynamoDB to expire items automatically. A node that stops reporting will disappear from inventory after `2 * poll_interval_secs`. This allows downstream consumers (DNS generators, dashboards) to detect dead nodes without polling the inventory themselves.
