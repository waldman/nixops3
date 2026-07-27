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
| `hostname` | S | `/proc/sys/kernel/hostname` | `ada-01.waldman.internal` |
| `machine_id` | S | `/etc/machine-id` | `a1b2c3d4...` |
| `role` | S | `nixops3.toml` | `home/production/ada` |
| `iface` | S | primary network interface | `eth0` |
| `ip` | S | IP of primary interface | `192.168.15.50` |
| `network` | S | network/prefix (v1: always `"unknown"`) | `192.168.15.0/24` |
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

## Search Queries — queries.toml

### Definition

Any profile, role, or host directory may include a `queries.toml` file defining DynamoDB queries to run before `nixos-rebuild`:

```toml
[[query]]
name        = "zookeeper_nodes"
role_prefix = "home/production/zookeeper"
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Key in `inventory.json` under which results are stored |
| `role_prefix` | yes | S3 role path prefix; matches items where `role` starts with this value |

### Query Merging

The daemon collects all `queries.toml` files found during the config tree download (see spec 01). All `[[query]]` entries are merged into a single list. Duplicate `name` values take the last definition (most-specific wins: host > role > profile > global).

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
        "hostname": "zk-01.waldman.internal",
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

The daemon writes to DynamoDB at the end of every poll cycle, regardless of whether a rebuild occurred:

| Scenario | `last_run_status` written |
|----------|--------------------------|
| Config unchanged (hash match) | `ok` |
| Config applied successfully | `ok` |
| Config apply failed | `failed` |
| Canary skip | `canary_skip` |
| S3 download failed | `failed` |

On any DynamoDB write failure: log the error to journald, continue. Inventory failure must never block config apply.

## Detecting Dead Nodes

The TTL attribute causes DynamoDB to expire items automatically. A node that stops reporting will disappear from inventory after `2 * poll_interval_secs`. This allows downstream consumers (DNS generators, dashboards) to detect dead nodes without polling the inventory themselves.
