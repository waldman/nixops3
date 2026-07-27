# Inventory & Queries

## Purpose

When inventory is enabled, each machine writes a heartbeat record to DynamoDB after every poll cycle. This gives you:

- **Fleet visibility** — which machines are alive, what config they're running, when they last checked in
- **Dynamic inventory** — other machines can query DynamoDB and use the results inside their `.nix` files, enabling config that references live fleet state (e.g. Zookeeper cluster membership, Kafka broker list)

The feature is opt-in. Disabling it has no effect on config apply behaviour.

## Enable inventory

In `/etc/nixops3/nixops3.toml`:

```toml
[inventory]
enabled = true
table   = "nixops3-inventory"
```

## DynamoDB table setup

Create the table with `hostname` as the partition key and TTL enabled:

```bash
aws dynamodb create-table \
  --table-name nixops3-inventory \
  --attribute-definitions AttributeName=hostname,AttributeType=S \
  --key-schema AttributeName=hostname,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST

aws dynamodb update-time-to-live \
  --table-name nixops3-inventory \
  --time-to-live-specification Enabled=true,AttributeName=ttl
```

## Heartbeat record

The daemon writes one record per machine per poll cycle:

| Attribute | Type | Source | Example |
|-----------|------|--------|---------|
| `hostname` | S | `hostname --fqdn` | `web-01.example.com` |
| `machine_id` | S | `/etc/machine-id` | `a1b2c3d4...` |
| `role` | S | `nixops3.toml` | `home/production/webserver` |
| `iface` | S | default route interface | `eth0` |
| `ip` | S | IP of that interface | `192.168.1.50` |
| `network` | S | network/prefix | `192.168.1.0/24` |
| `last_run_status` | S | outcome of last cycle | `ok` |
| `last_seen` | S | ISO 8601 UTC | `2026-07-26T21:00:00Z` |
| `ttl` | N | Unix epoch | `1753570800` |

The TTL is set to `now + 2 × poll_interval_secs`. A machine that stops checking in will automatically disappear from the table after that window. Downstream consumers (dashboards, DNS generators) can treat absence as dead without polling themselves.

### Status values

| Status | Meaning |
|--------|---------|
| `ok` | Config applied successfully, or hash was unchanged |
| `failed` | S3 download failed, or `nixos-rebuild` returned non-zero |
| `canary_skip` | `canary.txt` was present and this host was not listed |

## IAM requirements

Each machine needs permission to write only its own item, but read all items:

```json
[
  {
    "Effect": "Allow",
    "Action": ["dynamodb:PutItem", "dynamodb:UpdateItem"],
    "Resource": "arn:aws:dynamodb:<region>:<account>:table/nixops3-inventory",
    "Condition": {
      "ForAllValues:StringEquals": {
        "dynamodb:LeadingKeys": ["${aws:PrincipalTag/hostname}"]
      }
    }
  },
  {
    "Effect": "Allow",
    "Action": ["dynamodb:Query", "dynamodb:Scan"],
    "Resource": "arn:aws:dynamodb:<region>:<account>:table/nixops3-inventory"
  }
]
```

## Search queries (queries.toml)

Any role or host directory can include a `queries.toml` file that tells the daemon which DynamoDB queries to run before `nixos-rebuild`. The results are written to `/var/lib/nixops3/inventory.json` and read by `.nix` files at eval time.

### queries.toml format

```toml
[[query]]
name        = "zookeeper_nodes"           # key in inventory.json
role_prefix = "home/production/zookeeper" # matches items where role starts with this

[[query]]
name        = "kafka_nodes"
role_prefix = "home/production/kafka"
```

Fields:

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Key in `inventory.json` under `queries` |
| `role_prefix` | yes | DynamoDB filter: `begins_with(role, prefix)` |

### Query merging

The daemon collects `queries.toml` files from the role directory and the host directory, merges them into a single list. If both files define a query with the same `name`, the host-level definition wins.

### inventory.json format

```json
{
  "generated_at": "2026-07-26T21:00:00Z",
  "queries": {
    "zookeeper_nodes": [
      {
        "hostname": "zk-01.example.com",
        "ip": "192.168.1.10",
        "role": "home/production/zookeeper",
        "last_run_status": "ok"
      },
      {
        "hostname": "zk-02.example.com",
        "ip": "192.168.1.11",
        "role": "home/production/zookeeper",
        "last_run_status": "ok"
      }
    ]
  }
}
```

Only nodes with a TTL in the future (alive nodes) are included.

### Using inventory in .nix files

```nix
# profiles/zookeeper-client.nix
let
  inv = builtins.fromJSON (builtins.readFile /var/lib/nixops3/inventory.json);
  zkNodes = inv.queries.zookeeper_nodes;
in
{
  services.kafka.zookeeper.servers = map (n: "${n.hostname}:2181") zkNodes;

  # Or use IPs if hostnames aren't in DNS
  # services.kafka.zookeeper.servers = map (n: "${n.ip}:2181") zkNodes;
}
```

`builtins.readFile` and `builtins.fromJSON` run at `nix eval` time during `nixos-rebuild`. Because the daemon writes `inventory.json` before calling `nixos-rebuild`, the data is always fresh.

### Where to put queries.toml

Put `queries.toml` in the role or host directory in S3 (not in `profiles/`):

```
roles/home/production/kafka-broker/
  main.nix
  queries.toml    ← queries needed by the kafka-broker role
```

```
roles/home/production/kafka-broker/kafka-01.example.com/
  main.nix
  queries.toml    ← additional or overriding queries for this host
```

## Detecting dead nodes

The DynamoDB TTL attribute causes items to expire automatically after `2 × poll_interval_secs` without a heartbeat. At the default 10-minute interval, a machine that goes silent disappears from inventory after 20 minutes.

Consumers reading `inventory.json` automatically see only alive nodes because the daemon filters to `ttl > now` when executing queries.
