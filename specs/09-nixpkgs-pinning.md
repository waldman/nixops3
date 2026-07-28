# nixpkgs Pinning

## Purpose

Two hosts running "the same config" must produce the same closure. In v0.3,
the daemon evaluated `.nix` sources against whatever `nixpkgs` each host
happened to have (via channel discovery), so identical source could yield
different systems. v0.4 introduces per-role/per-host nixpkgs pinning to
control this.

The design gives three tiers of guarantee, chosen per role/host by which
fields appear in `main.yaml` (spec 08):

| Tier | Config | Convergence guarantee |
|---|---|---|
| **Loose** | no `pin:` block | None — each host uses its own channel |
| **Floating** | `pin.nixpkgs.channel` only | Same channel; **within-channel drift bounded by poll interval** |
| **Pinned** | `pin.nixpkgs.channel + rev` | Bit-identical nixpkgs input across the fleet |

The operator can restrict which tiers are allowed via `nixops3.toml` flags
(see [Config Options](#config-options)).

## Config Fields

Inside `main.yaml` (spec 08):

```yaml
pin:
  nixpkgs:
    channel: nixos-25.05                    # required if pin present
    rev:     abc1234def567890abcdef123...   # optional; 40-char hex
```

**`channel`** — arbitrary string identifying the nixpkgs channel or branch
(e.g. `nixos-25.05`, `nixos-unstable`, `nixos-25.11-small`). The daemon
uses this to resolve to a rev in the Floating tier and to log/report in the
Pinned tier. No validation is performed.

**`rev`** — 40-character lowercase hex string (git sha). If present, the
daemon uses this rev directly and does not resolve the channel. If absent
(Floating tier), the daemon resolves the channel at every cycle.

## The Three Tiers

### Tier 1: Loose (no `pin:` block)

Effective when neither role nor host `main.yaml` declares a `pin:` block
(after merge per spec 08).

**Daemon behavior:** falls back to nixpkgs discovery (`NIX_PATH` →
`/etc/set-environment` → root channels), same as v0.3.

**Log:** `WARN nixpkgs unpinned — using channel discovery`

**Heartbeat:**
- `pin_mode = "loose"`
- `nixpkgs_channel = ""`
- `nixpkgs_rev = ""` (discovery result is not captured in the heartbeat)

**Disabled by:** `require_pin = true` in `nixops3.toml`. When set, missing
`pin:` block causes the cycle to fail with `CycleOutcome::S3Error`
(misconfiguration; not truly S3 but the same "config not usable" bucket).

### Tier 2: Floating (`channel` only)

Effective when merged `main.yaml` has `pin.nixpkgs.channel` but no `rev`.

**Daemon behavior:** resolves the channel to the current rev via GitHub:

```
GET https://api.github.com/repos/NixOS/nixpkgs/commits/<channel>
```

Response's `.sha` is the current rev for that branch. The daemon then
follows the same steps as the Pinned tier below with the resolved rev.

**Resolution caching:** the daemon caches channel→rev for `channel_ttl_secs`
seconds (default 300) to avoid hammering GitHub across rapid poll cycles.
Cache is in-process only (lost on daemon restart).

**Drift bound:** all hosts converge within one poll interval + jitter of a
channel update. During the drift window, `nixpkgs_rev` in the heartbeat
reveals which host is on which rev.

**Log:** `INFO nixpkgs channel=<channel> rev=<resolved> (floating)`

**Heartbeat:**
- `pin_mode = "floating"`
- `nixpkgs_channel = "<channel>"`
- `nixpkgs_rev = "<resolved>"`

**Disabled by:** `require_explicit_rev = true` in `nixops3.toml`. When set,
`pin.nixpkgs` without `rev` causes the cycle to fail.

### Tier 3: Pinned (`channel` + `rev`)

Effective when merged `main.yaml` has both `pin.nixpkgs.channel` and
`pin.nixpkgs.rev`.

**Daemon behavior:** uses `rev` directly, no resolution call.

**Log:** `INFO nixpkgs channel=<channel> rev=<rev> (pinned)`

**Heartbeat:**
- `pin_mode = "pinned"`
- `nixpkgs_channel = "<channel>"`
- `nixpkgs_rev = "<rev>"`

## Local Cache

For Floating and Pinned tiers, the daemon materializes nixpkgs locally:

- **Cache path:** `/var/lib/nixops3/nixpkgs/<rev>/`
- **Content-addressed by rev.** Git guarantees uniqueness of a sha; two
  roles pinning the same rev share the same cache entry.
- **Download URL:** `https://github.com/NixOS/nixpkgs/archive/<rev>.tar.gz`
- **Extraction:** `tar --strip-components=1` so the resulting directory is
  the nixpkgs root (contains `default.nix`, `pkgs/`, etc.). No wrapper dir.
- **Extraction is to a tmp dir first** (`nixpkgs/.tmp-<rev>-<pid>/`), then
  renamed into place — same pattern as commit tree extraction (spec 02).
  Interrupted downloads leave no partial cache entry.

## Cache Lifecycle

**Prune** after each successful apply:

- Keep the newest `nixpkgs_retain` entries (default 3, configurable) by mtime
- **Never** delete the rev currently in use by the applied commit
- Ignore `.tmp-*` directories (cleaned up separately at cycle start)

**Sizing:** a typical nixpkgs archive is ~50 MB compressed, ~200 MB extracted.
`nixpkgs_retain = 3` costs ~600 MB. Adjust if disk-constrained.

## nixos-rebuild Invocation

Once the pin is resolved and cached at `/var/lib/nixops3/nixpkgs/<rev>/`, the
daemon replaces the nixpkgs discovery result with the cache path:

```sh
nixos-rebuild switch \
  -I nixos-config=/etc/nixos/configuration.nix \
  -I nixops3=/var/lib/nixops3/commits/<sha> \
  -I nixpkgs=/var/lib/nixops3/nixpkgs/<rev>
```

For the Loose tier, `-I nixpkgs=` continues to use discovered channels
exactly as in v0.3 (see spec 02, nixpkgs discovery section).

## Config Options

Additions to `nixops3.toml`:

```toml
[pins]
require_pin          = false     # true: missing pin: block → cycle fails
require_explicit_rev = false     # true: pin without rev → cycle fails
nixpkgs_retain       = 3         # LRU size for /var/lib/nixops3/nixpkgs/
channel_ttl_secs     = 300       # in-process cache TTL for channel resolution
```

**Defaults are permissive.** All three tiers work out of the box. Operators
who care about strict convergence flip the flags.

**`require_pin` and `require_explicit_rev` are independent.** Setting
`require_pin = true` still allows Floating (channel-only) pins. To require
Pinned tier fleet-wide, set both flags true.

## Error Modes

| Condition | Outcome |
|---|---|
| `pin:` present but malformed YAML | Cycle fails with `S3Error`; log error |
| `pin.nixpkgs` present without `channel` | Cycle fails with `S3Error`; log error |
| `channel` present, `require_explicit_rev = true` | Cycle fails with `S3Error`; log error |
| No `pin:` block, `require_pin = true` | Cycle fails with `S3Error`; log error |
| Channel resolution HTTP fails (network, 404, timeout) | Cycle fails with `S3Error`; log error |
| Tarball download fails | Cycle fails with `S3Error`; log error |
| GitHub API rate limit hit | Cycle fails with `S3Error`; log error with hint about rate limits |

All errors leave the symlink and cache in a consistent state — either the
prior sha is still applied, or the new sha isn't yet. No half-state.

## Convergence Visibility

The heartbeat exposes `nixpkgs_channel`, `nixpkgs_rev`, and `pin_mode` per
host. Operators can spot drift or misconfiguration with a DynamoDB scan:

```bash
aws dynamodb scan --table-name nixops3-inventory \
  --projection-expression "hostname, role, applied_sha, nixpkgs_channel, nixpkgs_rev, pin_mode"
```

**Converged fleet** (Pinned tier): all hosts of a role show the same
`applied_sha`, same `nixpkgs_channel`, same `nixpkgs_rev`, `pin_mode =
pinned`.

**Converged fleet** (Floating tier): all hosts show the same `applied_sha`,
same `nixpkgs_channel`; `nixpkgs_rev` may differ during the drift window
after a channel update.

**Misconfiguration:** hosts showing `pin_mode = loose` when the operator
expected pinning — indicates a `main.yaml` missing or incomplete.

## Non-Goals for v0.4

- **Overlays, home-manager, other non-nixpkgs inputs.** The `pin.nixpkgs`
  namespace leaves room for `pin.home-manager`, `pin.overlays.foo`, etc.,
  but v0.4 supports `nixpkgs` only.
- **sha256 verification of the downloaded tarball.** Trust GitHub HTTPS.
  Add optional `sha256` field in a later version for belt-and-suspenders.
- **Alternative resolver URLs.** The GitHub API is hardcoded. If GitHub's
  rate limits become a real problem, we add a config option to point at a
  different resolver (nixos.org, a proxy, etc.).
- **Binary cache / pre-built closures.** Hosts still evaluate and build.
