# Test Specification — nixops3 Daemon

All tests are written in Rust. Unit tests live in `src/` as `#[cfg(test)]` modules. Integration tests live in `tests/`.

S3 and DynamoDB interactions use a mock/stub implementation injected via trait objects. No real AWS calls in tests.

---

## Unit Tests

### 1. Config Parsing (`src/config.rs`)

**1.1 valid full config**
Input: valid TOML with all fields set.
Expected: `Config` struct populated correctly; no error.

**1.2 valid minimal config**
Input: TOML with only required fields (`bucket`, `region`, `role`).
Expected: optional fields take defaults (`poll_interval_secs = 600`, `inventory.enabled = false`).

**1.3 missing required field — bucket**
Input: TOML without `bucket`.
Expected: parse error; error message names the missing field.

**1.4 missing required field — region**
Input: TOML without `region`.
Expected: parse error.

**1.5 missing required field — role**
Input: TOML without `role`.
Expected: parse error.

**1.6 invalid poll_interval_secs — zero**
Input: `poll_interval_secs = 0`.
Expected: validation error (must be > 0).

**1.7 invalid poll_interval_secs — negative**
Input: `poll_interval_secs = -1` (TOML negative integer).
Expected: parse error.

**1.8 inventory disabled by default**
Input: minimal config without `[inventory]` section.
Expected: `config.inventory.enabled == false`.

**1.9 inventory enabled explicitly**
Input: `[inventory]\nenabled = true\ntable = "my-table"`.
Expected: `config.inventory.enabled == true`, `config.inventory.table == "my-table"`.

**1.10 inventory enabled without table name**
Input: `[inventory]\nenabled = true` (no `table`).
Expected: validation error (table required when enabled).

**1.11 aws credentials optional**
Input: config without `[aws]` section.
Expected: `config.aws == None`; daemon uses default AWS credential chain.

---

### 2. S3 Path Construction (`src/paths.rs`)

**2.1 role main.nix path**
Input: `role = "home/production/ada"`.
Expected: `roles/home/production/ada/main.nix`.

**2.3 host main.nix path**
Input: `role = "home/production/ada"`, `hostname = "ada-01.waldman.internal"`.
Expected: `roles/home/production/ada/ada-01.waldman.internal/main.nix`.

**2.4 queries.toml paths**
Input: `role = "home/production/ada"`, `hostname = "ada-01.waldman.internal"`.
Expected:
- `roles/home/production/ada/queries.toml`
- `roles/home/production/ada/ada-01.waldman.internal/queries.toml`

**2.5 canary.txt path**
Expected: `canary.txt` (bucket root).

**2.6 secrets prefixes**
Input: `role = "home/production/ada"`, `hostname = "ada-01.waldman.internal"`.
Expected:
- Role prefix: `NixOps/home/production/ada/shared/`
- Host prefix: `NixOps/home/production/ada/ada-01.waldman.internal/`

---

### 3. Hash Computation (`src/hash.rs`)

**3.1 deterministic output**
Input: same set of file contents in different insertion order.
Expected: identical SHA-256 hash both times.

**3.2 hash changes on content change**
Input: file set A, then file set A with one file modified.
Expected: different hashes.

**3.3 hash changes on file addition**
Input: file set A, then file set A plus one new file.
Expected: different hashes.

**3.4 hash changes on file removal**
Input: file set A, then file set A minus one file.
Expected: different hashes.

**3.5 empty file set**
Input: empty file list.
Expected: deterministic hash (not panic, not empty string).

**3.6 queries.toml excluded from hash**
Input: file set including a `queries.toml` file.
Expected: hash is identical to the same set without `queries.toml`.

---

### 3b. `<nixops3/...>` Import Scanner (`src/daemon.rs`)

**3b.1 no imports**
Input: `"{ ... }: {}"`.
Expected: empty list.

**3b.2 single import**
Input: `"imports = [ <nixops3/profiles/base.nix> ];"`.
Expected: `["profiles/base.nix"]`.

**3b.3 multiple imports**
Input: two `<nixops3/...>` references on separate lines.
Expected: both paths returned in order of appearance.

**3b.4 deduplication**
Input: same `<nixops3/profiles/base.nix>` appearing twice.
Expected: `["profiles/base.nix"]` (one entry).

---

### 4. Canary Check (`src/canary.rs`)

**4.1 no canary file — apply**
Input: S3 returns 404 for `canary.txt`.
Expected: `CanaryResult::Apply`.

**4.2 hostname in canary file — apply**
Input: `canary.txt` contains `ada-01.waldman.internal\n`.
Hostname: `ada-01.waldman.internal`.
Expected: `CanaryResult::Apply`.

**4.3 hostname not in canary file — skip**
Input: `canary.txt` contains `ada-02.waldman.internal\n`.
Hostname: `ada-01.waldman.internal`.
Expected: `CanaryResult::Skip`.

**4.4 empty canary file — all skip**
Input: `canary.txt` is empty.
Hostname: `ada-01.waldman.internal`.
Expected: `CanaryResult::Skip`.

**4.5 hostname partial match not accepted**
Input: `canary.txt` contains `ada-01\n` (no domain).
Hostname: `ada-01.waldman.internal`.
Expected: `CanaryResult::Skip` (exact FQDN match required).

**4.6 comment lines ignored**
Input: `canary.txt` contains `# comment\nada-01.waldman.internal\n`.
Expected: `CanaryResult::Apply`.

**4.7 blank lines ignored**
Input: `canary.txt` contains `\n\nada-01.waldman.internal\n\n`.
Expected: `CanaryResult::Apply`.

**4.8 multiple hostnames in file**
Input: `canary.txt` contains `ada-01.waldman.internal\nada-02.waldman.internal\n`.
Hostname: `ada-02.waldman.internal`.
Expected: `CanaryResult::Apply`.

---

### 5. configuration.nix Generation (`src/nixgen.rs`)

**5.1 basic generation — no host**
Input: `role = "home/production/ada"`, `hostname = None`.
Expected output contains exactly:
1. `/etc/nixos/hardware-configuration.nix`
2. `./roles/home/production/ada/main.nix`

**5.2 host included when present**
Input: `role = "home/production/ada"`, `hostname = Some("ada-01.waldman.internal")`.
Expected: host import `./roles/home/production/ada/ada-01.waldman.internal/main.nix` is last.

**5.3 hardware-configuration.nix always first**
Input: any role/hostname combination.
Expected: first import is always `/etc/nixos/hardware-configuration.nix`.

---

### 6. queries.toml Parsing and Merging (`src/queries.rs`)

**6.1 valid single query**
Input:
```toml
[[query]]
name        = "zk_nodes"
role_prefix = "home/production/zookeeper"
```
Expected: one query with correct fields.

**6.2 valid multiple queries**
Input: two `[[query]]` blocks.
Expected: two query structs.

**6.3 empty queries.toml**
Input: empty file.
Expected: empty query list (not an error).

**6.4 missing name field**
Input: `[[query]]` block without `name`.
Expected: parse error.

**6.5 missing role_prefix field**
Input: `[[query]]` block without `role_prefix`.
Expected: parse error.

**6.6 merge — two files, no duplicates**
Input: file A with query `zk_nodes`, file B with query `kafka_nodes`.
Expected: merged list has both queries.

**6.7 merge — duplicate name, host wins**
Input: role `queries.toml` defines `zk_nodes` with `role_prefix = "home/production/zookeeper"`.
Host `queries.toml` defines `zk_nodes` with `role_prefix = "home/staging/zookeeper"`.
Expected: merged list has one `zk_nodes` with `role_prefix = "home/staging/zookeeper"`.

---

### 7. Secrets Path Construction (`src/secrets.rs`)

**7.1 role-level secret path**
Input: `role = "home/production/ada"`.
Expected prefix: `NixOps/home/production/ada/shared/`.

**7.2 host-level secret path**
Input: `role = "home/production/ada"`, `hostname = "ada-01.waldman.internal"`.
Expected prefix: `NixOps/home/production/ada/ada-01.waldman.internal/`.

**7.3 local path for secret**
Input: secret name `openrouter-api-key`.
Expected local path: `/run/nixops3/secrets/openrouter-api-key`.

**7.4 host secret overrides role secret**
Input: both role-level and host-level secret named `api-key` exist.
Expected: host-level value written to `/run/nixops3/secrets/api-key`.

---

### 8. Jitter (`src/timer.rs`)

**8.1 jitter is within bounds**
Input: `poll_interval_secs = 600`.
Run 1000 iterations.
Expected: all computed sleep durations in `[600, 660)` seconds.

**8.2 jitter is not constant**
Input: `poll_interval_secs = 600`.
Run 100 iterations.
Expected: at least 2 distinct sleep duration values (not all identical).

---

## Integration Tests

### 9. Full Apply Cycle (`tests/apply.rs`)

**9.1 happy path — first run**
Setup:
- Mock S3 with valid profile + role + host files
- No `canary.txt`
- No `last-hash`

Expected:
- `nixos-rebuild` called once with correct `-I` argument
- `last-hash` written after success
- DynamoDB heartbeat written with `status: ok`

**9.2 no-op — hash unchanged**
Setup:
- Mock S3 with files
- `last-hash` already matches current hash

Expected:
- `nixos-rebuild` NOT called
- DynamoDB heartbeat written with `status: ok`

**9.3 apply triggered — hash changed**
Setup:
- First cycle: apply, write hash
- Second cycle: one file in S3 changed

Expected:
- `nixos-rebuild` called on second cycle
- `last-hash` updated to new hash

**9.4 canary skip — no apply**
Setup:
- `canary.txt` in S3 with a different hostname
- Different hash from last run

Expected:
- `nixos-rebuild` NOT called
- `last-hash` NOT updated
- DynamoDB heartbeat written with `status: canary_skip`

**9.5 nixos-rebuild failure — no hash update**
Setup:
- Mock `nixos-rebuild` returning exit code 1

Expected:
- `last-hash` NOT updated
- DynamoDB heartbeat written with `status: failed`
- Next cycle retries (hash still differs, apply runs again)

**9.6 S3 download failure — apply skipped**
Setup:
- Mock S3 returning 500 on file download

Expected:
- `nixos-rebuild` NOT called
- `last-hash` NOT updated
- DynamoDB heartbeat written with `status: failed`
- Error logged to journald

**9.7 host main.nix absent — apply succeeds**
Setup:
- S3 has role `main.nix` but no host directory

Expected:
- `nixos-rebuild` called
- Generated `configuration.nix` does not include a host import

**9.8 inventory disabled — no DynamoDB writes**
Setup:
- `inventory.enabled = false` in config
- Mock DynamoDB that fails if called

Expected:
- Apply succeeds
- DynamoDB never called

**9.9 inventory write failure — apply continues**
Setup:
- DynamoDB mock returns error on `PutItem`
- Valid S3 config

Expected:
- `nixos-rebuild` still called
- Error logged but daemon does not crash

**9.10 secrets fetched before rebuild**
Setup:
- Mock Secrets Manager with two secrets (role-level + host-level)
- Mock filesystem

Expected:
- Both secrets written to `/run/nixops3/secrets/` before `nixos-rebuild` runs
- Host-level secret overwrites role-level secret of same name

**9.11 query results written before rebuild**
Setup:
- `queries.toml` in role directory with one query
- DynamoDB scan returns two items

Expected:
- `/var/lib/nixops3/inventory.json` written before `nixos-rebuild`
- JSON contains correct `queries` structure

---

## Test Helpers

All integration tests use a `TestContext` struct that wires together:
- `MockS3Client` — in-memory map of S3 keys to content
- `MockDynamoClient` — records `PutItem` calls for assertion
- `MockSecretsClient` — returns configured secret values
- `MockExecutor` — records shell command invocations, returns configurable exit codes
- Temp directories for working dir and secrets dir

```rust
let ctx = TestContext::builder()
    .s3_file("profiles/base.nix", "{ ... }:")
    .s3_file("roles/home/production/ada/main.nix", "{ ... }:")
    .last_hash("") // first run
    .build();

ctx.run_cycle().await;

assert!(ctx.rebuild_was_called());
assert_eq!(ctx.last_hash(), ctx.computed_hash());
assert_eq!(ctx.dynamo_status(), "ok");
```
