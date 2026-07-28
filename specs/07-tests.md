# Test Specification — nixops3 Daemon

All tests are written in Rust. Unit tests live in `src/` as `#[cfg(test)]` modules. Integration tests live in `tests/`.

S3 and DynamoDB interactions use a mock/stub implementation injected via trait objects. No real AWS calls in tests.

---

## Unit Tests

### 1. Config Parsing (`src/config.rs`)

**1.1 valid full config**
Input: valid TOML with all fields set (including `trees_retain`).
Expected: `Config` struct populated correctly; no error.

**1.2 valid minimal config**
Input: TOML with only required fields (`bucket`, `region`, `role`).
Expected: optional fields take defaults (`poll_interval_secs = 600`, `trees_retain = 5`, `inventory.enabled = false`).

**1.3 missing required field — bucket**
Input: TOML without `bucket`.
Expected: parse error; error message names the missing field.

**1.4 missing required field — region**
Expected: parse error.

**1.5 missing required field — role**
Expected: parse error.

**1.6 invalid poll_interval_secs — zero**
Input: `poll_interval_secs = 0`.
Expected: validation error (must be > 0).

**1.7 invalid poll_interval_secs — negative**
Expected: parse error.

**1.8 trees_retain default**
Input: minimal config.
Expected: `config.trees_retain == 5`.

**1.9 trees_retain minimum**
Input: `trees_retain = 1`.
Expected: parsed (`trees_retain = 1`); zero or negative rejected.

**1.10 inventory disabled by default**
Expected: `config.inventory.enabled == false`.

**1.11 inventory enabled without table name**
Expected: validation error.

**1.12 aws credentials optional**
Expected: `config.aws == None`; daemon uses default AWS credential chain.

---

### 2. S3 Path Construction (`src/paths.rs`)

**2.1 pointer path**
Expected: `"current"` (bucket root).

**2.2 commit tree prefix**
Input: `sha = "abc1234..."`.
Expected: `"commits/abc1234.../"`.

**2.3 role main.nix path**
Input: `sha = "abc1234"`, `role = "home/production/webserver"`.
Expected: `"commits/abc1234/roles/home/production/webserver/main.nix"`.

**2.4 host main.nix path**
Input: `sha = "abc1234"`, `role = "home/production/webserver"`, `hostname = "web-01.waldman.internal"`.
Expected: `"commits/abc1234/roles/home/production/webserver/web-01.waldman.internal/main.nix"`.

**2.5 canary.txt path (role-scoped, per-commit)**
Input: `sha = "abc1234"`, `role = "home/production/webserver"`.
Expected: `"commits/abc1234/roles/home/production/webserver/canary.txt"`.

**2.6 secrets prefixes**
Expected:
- Role prefix: `"NixOps/home/production/webserver/shared/"`
- Host prefix: `"NixOps/home/production/webserver/web-01.waldman.internal/"`

---

### 3. Pointer Parsing and Validation (`src/pointer.rs`)

**3.1 valid sha**
Input: `"abc1234...deadbeef01234567890abcdef1234567890"` (40 hex chars).
Expected: parsed sha.

**3.2 trailing newline accepted**
Input: `"abc1234...\n"`.
Expected: parsed sha (newline stripped).

**3.3 trailing whitespace accepted**
Input: `"abc1234... \n"`.
Expected: parsed sha.

**3.4 too short — rejected**
Input: `"abc"`.
Expected: parse error.

**3.5 too long — rejected**
Input: 41 hex chars.
Expected: parse error.

**3.6 non-hex — rejected**
Input: `"abcdefghij..."` (40 chars, includes `g-z`).
Expected: parse error.

**3.7 empty — rejected**
Expected: parse error.

**3.8 embedded whitespace — rejected**
Input: `"abc 1234..."`.
Expected: parse error.

---

### 4. Canary Check (`src/canary.rs`)

**4.1 no canary file — apply**
Input: S3 returns 404 for `commits/<sha>/roles/<role>/canary.txt`.
Expected: `CanaryResult::Apply`. Log level: debug (not warn).

**4.2 hostname listed — apply**
Input: canary.txt contains `web-01.waldman.internal\n`. Hostname: `web-01.waldman.internal`.
Expected: `CanaryResult::Apply`.

**4.3 hostname not listed — skip**
Input: canary.txt contains `web-02.waldman.internal\n`. Hostname: `web-01.waldman.internal`.
Expected: `CanaryResult::Skip`.

**4.4 empty canary file — all skip**
Input: canary.txt is empty. Any hostname.
Expected: `CanaryResult::Skip`.

**4.5 hostname partial match not accepted**
Input: canary.txt contains `web-01\n`. Hostname: `web-01.waldman.internal`.
Expected: `CanaryResult::Skip` (exact FQDN match required).

**4.6 comment lines ignored**
Input: `# comment\nweb-01.waldman.internal\n`.
Expected: `CanaryResult::Apply`.

**4.7 blank lines ignored**
Input: `\n\nweb-01.waldman.internal\n\n`.
Expected: `CanaryResult::Apply`.

**4.8 multiple hostnames**
Input: `web-01...\nweb-02...\n`. Hostname: `web-02...`.
Expected: `CanaryResult::Apply`.

---

### 5. configuration.nix Generation (`src/nixgen.rs`)

**5.1 basic generation — no host**
Input: `role = "home/production/webserver"`, `hostname = None`, tree at `/var/lib/nixops3/commits/abc1234/`, hw-config present.
Expected: three-import file with hw-config, role main.nix (absolute path into tree), and boot.loader.grub.device guard.

**5.2 host included when present**
Input: same + `hostname = Some("web-01...")`, host main.nix exists in tree.
Expected: four-import file (hw-config, role, host, guard).

**5.3 hardware-configuration.nix omitted if still absent**
Input: hw-config does not exist even after `nixos-generate-config` attempt.
Expected: role import present, hw-config import absent, boot.loader.grub.device guard still emitted.

**5.4 host import omitted when host main.nix absent from tree**
Expected: role import present, host import absent.

---

### 6. `main.yaml` Parsing and Merging (`src/main_yaml.rs`)

**6.1 valid full config**
Input: YAML with both `pin:` and `queries:` sections populated.
Expected: parsed struct with both sections; no error.

**6.2 empty file**
Input: empty string, or `{}`.
Expected: `MainYaml { pin: None, queries: None }`; no error.

**6.3 pin without channel**
Input: `pin: { nixpkgs: { rev: "abc..." } }` (missing required `channel`).
Expected: parse error naming the missing field.

**6.4 pin with channel only**
Input: `pin: { nixpkgs: { channel: "nixos-25.05" } }`.
Expected: parsed with `rev = None`.

**6.5 pin with channel and rev**
Input: both present.
Expected: parsed correctly; rev is a 40-char lowercase hex string.

**6.6 rev must be 40 hex chars**
Input: rev = `"abc"` (too short) or `"XYZ123..."` (non-hex).
Expected: parse error.

**6.7 queries as a map**
Input: `queries: { zk_nodes: { role_prefix: "..." } }`.
Expected: map with one entry keyed `zk_nodes`.

**6.8 queries missing role_prefix**
Input: `queries: { zk_nodes: {} }`.
Expected: parse error.

**6.9 merge — host overrides role's pin entirely**
Setup: role YAML has `pin.nixpkgs = {channel: A, rev: R1}`; host YAML has
`pin.nixpkgs = {channel: B}` (no rev).
Expected merged: `pin.nixpkgs = {channel: B, rev: None}`. Host wins wholesale;
the role's `rev` is not inherited.

**6.10 merge — host omits pin, inherits role's**
Setup: role has `pin`; host YAML omits `pin`.
Expected merged: role's `pin` unchanged.

**6.11 merge — host overrides queries entirely**
Setup: role has `queries: {a, b}`; host has `queries: {c}`.
Expected merged: `queries: {c}` only. No merge of individual keys.

**6.12 role YAML absent, host YAML present**
Expected: host YAML applies as-is; no error.

**6.13 both YAMLs absent**
Expected: `MainYaml { pin: None, queries: None }`; no error.

**6.14 strict types**
Input: `pin: { nixpkgs: { channel: no, rev: 12345 } }` (unquoted `no`, numeric `rev`).
Expected: parse error (rev must be string, channel expected as string).

---

### 6b. Pin Resolution (`src/pin.rs`)

**6b.1 Loose tier — no pin block**
Input: merged main.yaml with no `pin:`. Config: defaults.
Expected: `PinResolution::Loose`. No HTTP call.

**6b.2 Loose tier — require_pin fails cycle**
Input: no pin block. Config: `require_pin = true`.
Expected: error, cycle fails.

**6b.3 Floating tier — channel resolves to rev**
Input: `pin.nixpkgs = {channel: "nixos-25.05"}`. Mock channel resolver (channels.nixos.org/git-revision) returns sha `abc1234...`.
Expected: `PinResolution::Floating { channel, rev = "abc1234..." }`. One HTTP call.

**6b.4 Floating tier — cached resolution**
Setup: two back-to-back resolutions of the same channel within `channel_ttl_secs`.
Expected: second resolution hits the in-process cache; only one HTTP call issued.

**6b.5 Floating tier — require_explicit_rev fails cycle**
Input: `pin.nixpkgs = {channel: "..."}` (no rev). Config: `require_explicit_rev = true`.
Expected: error, cycle fails.

**6b.6 Pinned tier — rev used directly**
Input: `pin.nixpkgs = {channel: "nixos-25.05", rev: "abc1234..."}`.
Expected: `PinResolution::Pinned { channel, rev }`. No HTTP call.

**6b.7 Pinned tier — malformed rev**
Input: rev is not 40 hex chars.
Expected: parse error (caught at YAML parse, spec 6.6).

**6b.8 Channel resolution HTTP failure**
Setup: mock resolver returns network error.
Expected: error, cycle fails.

**6b.9 Channel resolution 404**
Setup: mock resolver returns 404 (channel not published to channels.nixos.org).
Expected: error naming the channel, cycle fails.

---

### 6c. nixpkgs URL construction (`src/pin.rs`)

The daemon no longer manages a local nixpkgs cache — Nix handles fetching,
extraction, and store retention. All the daemon does is construct the URL
passed to `nixos-rebuild` via `-I nixpkgs=`.

**6c.1 Pinned tier → GitHub archive URL**
Input: `pin.nixpkgs = {channel: "nixos-25.05", rev: "abc1234..."}`.
Expected: `nixpkgs_arg(&pin_res) == "https://github.com/NixOS/nixpkgs/archive/abc1234....tar.gz"`.

**6c.2 Floating tier → same URL shape with resolved rev**
Input: Floating resolves to `def5678...`.
Expected: URL substitutes the resolved rev.

**6c.3 Loose tier → local path from find_nixpkgs()**
Input: no pin.
Expected: `nixpkgs_arg` is whatever `find_nixpkgs()` returned (existing v0.3 behavior).

---

### 7. Secrets Path Construction (`src/secrets.rs`)

**7.1 role-level prefix**
Expected: `"NixOps/home/production/webserver/shared/"`.

**7.2 host-level prefix**
Expected: `"NixOps/home/production/webserver/web-01.waldman.internal/"`.

**7.3 local path for secret**
Expected: `/run/nixops3/secrets/openrouter-api-key`.

**7.4 host secret overrides role secret**
Expected: host-level value written to disk.

---

### 8. Jitter (`src/timer.rs`)

**8.1 jitter within bounds**
Input: `poll_interval_secs = 600`. 1000 iterations.
Expected: all in `[600, 660)` seconds.

**8.2 jitter is not constant**
Expected: at least 2 distinct values across 100 iterations.

---

### 9. Tree Extraction (`src/tree.rs`)

**9.1 fetches all objects under prefix**
Input: mock S3 with 3 objects under `commits/<sha>/`.
Expected: 3 local files written under `/var/lib/nixops3/commits/<sha>/` with matching content.

**9.2 preserves nested paths**
Input: S3 keys `commits/<sha>/roles/home/production/webserver/main.nix` and `commits/<sha>/profiles/base.nix`.
Expected: both written at their nested local paths.

**9.3 extraction is to tmp then renamed**
Input: mock that inspects filesystem mid-download.
Expected: files land in `commits/.tmp-<sha>-<pid>/` first; final rename to `commits/<sha>/` only after all writes succeed.

**9.4 partial extraction leaves no `commits/<sha>/`**
Input: mock returns error on the 2nd of 3 GETs.
Expected: `commits/<sha>/` does not exist; `commits/.tmp-<sha>-*` may remain but is cleaned up on next cycle start.

**9.5 no-op when tree already local and complete**
Input: `commits/<sha>/` already exists.
Expected: no list_prefix call, no GET calls issued.

**9.6 stale `.tmp-*` cleanup**
Input: an orphaned `commits/.tmp-abc-9999/` from a previous run.
Expected: removed at cycle start before any new extraction.

---

### 10. Symlink Advance (`src/symlink.rs`)

**10.1 first-time creation**
Input: no existing symlink.
Expected: `/var/lib/nixops3/current` created pointing at `commits/<sha>`.

**10.2 replacement is atomic**
Input: existing symlink pointing at old sha.
Expected: new sha is visible via `readlink` after the operation; no intermediate state where symlink is missing (verified by concurrent reader).

**10.3 leaves symlink alone on rebuild failure**
Input: prior symlink at `commits/def5678`, rebuild fails.
Expected: `readlink` still returns `commits/def5678`.

---

### 11. Tree Pruning (`src/tree.rs`)

**11.1 keeps N most recent**
Input: 8 local trees, `trees_retain = 5`.
Expected: 5 remain (the 5 newest by mtime).

**11.2 never deletes symlink target**
Input: symlink points at the 6th-newest tree; `trees_retain = 5`.
Expected: symlink target retained; only the 3 oldest non-target trees are deleted (net: 6 remaining).

**11.3 no-op when count ≤ N**
Input: 3 trees, `trees_retain = 5`.
Expected: no deletions.

**11.4 does not touch `.tmp-*` dirs**
Expected: `.tmp-*` cleanup is separate; pruning ignores them.

---

## Integration Tests

### 12. Full Apply Cycle (`tests/apply.rs`)

**12.1 happy path — first run**
Setup: mock S3 with `current=abc1234` and populated `commits/abc1234/`. No local symlink. No canary.txt.
Expected:
- List + GETs against `commits/abc1234/` prefix
- `/etc/nixos/configuration.nix` written before `nixos-rebuild` invoked
- `nixos-rebuild` called once with `-I nixops3=/var/lib/nixops3/commits/abc1234` (no `-I nixos-config=`)
- Symlink `/var/lib/nixops3/current` → `commits/abc1234`
- Heartbeat: `status=ok, applied_sha=abc1234, target_sha=abc1234`

**12.2 no-op — symlink already at target**
Setup: `current=abc1234`, local symlink already `commits/abc1234`.
Expected:
- Zero GETs against `commits/`
- `nixos-rebuild` NOT called
- Heartbeat: `status=ok, applied_sha=abc1234, target_sha=abc1234`

**12.3 apply triggered — pointer flipped**
Setup: local symlink at `commits/def5678`. S3 `current=abc1234`. Tree present in S3.
Expected:
- Full fetch of `commits/abc1234/`
- `nixos-rebuild` called
- Symlink advanced to `commits/abc1234`

**12.4 canary skip — pointer unchanged locally**
Setup: `current=abc1234`, local symlink at `commits/def5678`, `canary.txt` at `commits/abc1234/roles/<role>/canary.txt` listing a DIFFERENT host.
Expected:
- Zero GETs against `commits/abc1234/*` other than the canary.txt itself
- `nixos-rebuild` NOT called
- Symlink NOT advanced (still `commits/def5678`)
- Heartbeat: `status=canary_skip, applied_sha=def5678, target_sha=abc1234`

**12.5 nixos-rebuild failure — no symlink advance**
Setup: mock `nixos-rebuild` returns exit code 1.
Expected:
- Symlink unchanged
- Heartbeat: `status=failed, applied_sha=<old>, target_sha=<target>`
- Next cycle retries (symlink still != target)

**12.6 S3 pointer fetch failure — apply skipped**
Setup: mock S3 returns 500 on GET `current`.
Expected:
- No tree fetch, no rebuild
- Symlink unchanged
- Heartbeat: `status=failed, applied_sha=<old>, target_sha=""`

**12.7 malformed pointer — apply skipped**
Setup: `current` contains `"not-a-sha"`.
Expected:
- No tree fetch, no rebuild
- Symlink unchanged
- Error logged
- Heartbeat: `status=failed, applied_sha=<old>, target_sha=""`

**12.8 partial tree fetch — recovers next cycle**
Setup: mock S3 returns 500 on the 2nd of 3 GETs.
Expected:
- Cycle 1: no `commits/<sha>/` written, error logged, symlink unchanged
- Cycle 2 (S3 healthy): fetches complete, `commits/<sha>/` written, rebuild runs, symlink advances

**12.9 host main.nix absent from tree — apply succeeds**
Setup: `commits/<sha>/roles/<role>/` has `main.nix` but no `<hostname>/` subdir.
Expected: `nixos-rebuild` called; generated `configuration.nix` does not include a host import.

**12.10 inventory disabled — no DynamoDB writes**
Setup: `inventory.enabled = false`, mock DynamoDB fails if called.
Expected: apply succeeds; DynamoDB never called.

**12.11 inventory write failure — apply continues**
Setup: DynamoDB mock returns error on PutItem.
Expected: `nixos-rebuild` still called; symlink advances; error logged.

**12.12 secrets fetched before rebuild**
Setup: mock Secrets Manager with role-level + host-level secrets.
Expected: both written to `/run/nixops3/secrets/` before `nixos-rebuild` runs; host-level wins on same short name.

**12.13 query results written before rebuild**
Setup: `main.yaml` at `commits/<sha>/roles/<role>/main.yaml` with a `queries:` section defining one query; DynamoDB scan returns two items.
Expected: `/var/lib/nixops3/inventory.json` written before `nixos-rebuild`; correct structure.

**12.14 NIX_PATH resolution of `<nixops3/...>`**
Setup: role's `main.nix` contains `imports = [ <nixops3/profiles/base.nix> ]`. Tree extracted at `/var/lib/nixops3/commits/<sha>/`.
Expected: `nixos-rebuild` invoked with `-I nixops3=/var/lib/nixops3/commits/<sha>` such that the import resolves to the extracted `profiles/base.nix`.

**12.15 concurrency — flock serializes cycles**
Setup: two concurrent invocations against the same `/var/lib/nixops3/`.
Expected: second invocation blocks on `flock`; runs its own cycle after the first releases.

---

### 13. Pin Tiers End-to-End (`tests/apply.rs`)

**13.1 Loose tier — no main.yaml at all**
Setup: no `main.yaml` in commit tree; nixops3.toml has no `[pins]` or defaults.
Expected: apply succeeds via channel discovery; `pin_mode=loose`, `nixpkgs_channel=""`, `nixpkgs_rev=""` in heartbeat. WARN logged.

**13.2 Loose tier — require_pin blocks**
Setup: same as 13.1 but `require_pin = true`.
Expected: cycle fails; `status=failed`; error mentions missing pin.

**13.3 Floating tier — resolves + passes URL to nix**
Setup: `main.yaml` has `pin.nixpkgs.channel: nixos-25.05` (no rev). Mock channel resolver returns sha `abc1234...`.
Expected:
- Resolver called once with the channel
- `nixos-rebuild` invoked with `-I nixpkgs=https://github.com/NixOS/nixpkgs/archive/abc1234...tar.gz`
- **No local nixpkgs cache dir created by the daemon** — Nix owns the download
- Heartbeat: `pin_mode=floating`, `nixpkgs_channel=nixos-25.05`, `nixpkgs_rev=abc1234...`

**13.4 Floating tier — require_explicit_rev blocks**
Setup: same YAML as 13.3 but `require_explicit_rev = true`.
Expected: cycle fails; error mentions missing rev.

**13.5 Pinned tier — no resolver call**
Setup: `main.yaml` has both `channel` and `rev`.
Expected: mock resolver NOT called; `nixos-rebuild` invoked with
`-I nixpkgs=https://github.com/NixOS/nixpkgs/archive/<rev>.tar.gz`; heartbeat carries `pin_mode=pinned`.

**13.7 Merge — host `main.yaml` overrides role's pin**
Setup: role `main.yaml` pins `nixos-25.05, rev=X`; host `main.yaml` pins `nixos-25.11` (no rev).
Expected: daemon uses `nixos-25.11` in Floating mode; host's pin block replaced role's wholesale (role's `rev` NOT inherited).

**13.8 Merge — host omits pin, inherits role's**
Setup: role `main.yaml` has `pin`; host `main.yaml` has only `queries:`, no `pin:`.
Expected: role's pin applies unchanged.

**13.9 Pin resolution failure — cycle fails**
Setup: Floating pin; mock resolver returns 500.
Expected: no `nixos-rebuild` call; symlink unchanged; heartbeat `status=failed`.

**13.10 Malformed pin — cycle fails**
Setup: `pin: { nixpkgs: { rev: "abc..." } }` (missing channel).
Expected: cycle fails at YAML parse; heartbeat `status=failed`.

---

## Test Helpers

All integration tests use a `TestContext` struct that wires together:
- `MockS3Client` — in-memory map of S3 keys to content, `list_prefix` support, per-key failure injection
- `MockDynamoClient` — records `PutItem` calls for assertion
- `MockSecretsClient` — returns configured secret values
- `MockExecutor` — records shell command invocations, returns configurable exit codes
- Temp directories for `/var/lib/nixops3` and `/run/nixops3`

```rust
let ctx = TestContext::builder()
    .s3_pointer("abc1234...")
    .s3_file("commits/abc1234.../profiles/base.nix", "{ ... }:")
    .s3_file("commits/abc1234.../roles/home/production/webserver/main.nix", "{ ... }:")
    .build();

ctx.run_cycle().await;

assert!(ctx.rebuild_was_called());
assert_eq!(ctx.symlink_target(), "commits/abc1234...");
assert_eq!(ctx.dynamo_status(), "ok");
assert_eq!(ctx.dynamo_applied_sha(), "abc1234...");
assert_eq!(ctx.dynamo_target_sha(), "abc1234...");
```
