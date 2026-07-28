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

## Design principle: let Nix handle nixpkgs

Nix already knows how to fetch, cache, and garbage-collect nixpkgs source.
The daemon does **not** download, extract, or cache nixpkgs itself. Instead:

- **Pinned tier:** construct a URL like
  `https://github.com/NixOS/nixpkgs/archive/<rev>.tar.gz` and pass it as
  `-I nixpkgs=<url>`. Nix downloads if not in its store, extracts, uses.
- **Floating tier:** resolve the channel to a concrete rev (see below), then
  behave like Pinned with that resolved rev.
- **Loose tier:** existing local `find_nixpkgs()` fallback, unchanged from v0.3.

Nix's store handles caching automatically. Old revs are GC'd along with the
system generations that referenced them — the daemon has no LRU knob and no
local cache dir. Simpler than v0.4's original attempt, and Nix's GC is more
correct than any hand-rolled retention policy.

## Config Fields (in `main.yaml`)

See spec 08 for the file. Under `pin.nixpkgs`:

```yaml
pin:
  nixpkgs:
    channel: nixos-25.05                    # required if pin present
    rev:     abc1234def567890abcdef123...   # optional; 40-char lowercase hex
```

**`channel`** — arbitrary string identifying the nixpkgs channel or branch
(e.g. `nixos-25.05`, `nixos-unstable`, `nixos-25.11-small`). The daemon uses
this to resolve to a rev in the Floating tier. In the Pinned tier it is
recorded in the heartbeat (`nixpkgs_channel`) but has no operational effect.
No validation is performed on the string.

**`rev`** — 40-character lowercase hex string (git sha). If present, the
daemon uses it directly and does not resolve the channel. If absent (Floating
tier), the daemon resolves the channel at every cycle.

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
`pin:` block causes the cycle to fail.

### Tier 2: Floating (`channel` only)

Effective when merged `main.yaml` has `pin.nixpkgs.channel` but no `rev`.

**Resolution:** the daemon does one HTTP GET to:

```
https://channels.nixos.org/<channel>/git-revision
```

This is a small text file (40-char hex + newline) served by nixos.org's CDN.
**Not GitHub API** — no per-IP rate limit concern. Returns the last
Hydra-tested rev for that channel (more conservative than "latest commit on
branch" — matches what `nix-channel --update` would fetch).

**Caching:** in-process TTL cache (`channel_ttl_secs`, default 300 seconds)
so rapid poll cycles hit the resolver at most once per channel per TTL
window.

**Invocation:** with the resolved rev, the daemon passes:

```
-I nixpkgs=https://github.com/NixOS/nixpkgs/archive/<rev>.tar.gz
```

Nix downloads (from its own tarball cache if present) and uses.

**Drift bound:** channels.nixos.org updates roughly every 8 hours (when
Hydra publishes a new channel). All hosts converge within one poll interval
of a channel update. During the drift window, `nixpkgs_rev` in the heartbeat
reveals which host is on which rev.

**Log:** `INFO nixpkgs channel=<channel> rev=<resolved> (floating)`

**Heartbeat:**
- `pin_mode = "floating"`
- `nixpkgs_channel = "<channel>"`
- `nixpkgs_rev = "<resolved>"`

**Disabled by:** `require_explicit_rev = true` in `nixops3.toml`.

### Tier 3: Pinned (`channel` + `rev`)

Effective when merged `main.yaml` has both `pin.nixpkgs.channel` and
`pin.nixpkgs.rev`.

**Resolution:** none. Daemon uses `rev` directly.

**Invocation:**

```
-I nixpkgs=https://github.com/NixOS/nixpkgs/archive/<rev>.tar.gz
```

**Log:** `INFO nixpkgs channel=<channel> rev=<rev> (pinned)`

**Heartbeat:**
- `pin_mode = "pinned"`
- `nixpkgs_channel = "<channel>"`
- `nixpkgs_rev = "<rev>"`

## What the daemon does NOT do

Deliberately not in scope, because Nix already handles it:

- **Download tarballs.** Nix does this via its normal `fetchTarball`
  facility triggered by the `-I` flag.
- **Extract tarballs.** Nix does this.
- **Cache nixpkgs locally.** Nix uses its own tarball cache
  (`/nix/var/nix/tarballs/`) and store.
- **Prune old cached copies.** Nix's `nix-collect-garbage` handles this,
  driven by NixOS generation lifetime.
- **Verify sha256 of downloaded content.** Nix verifies against its internal
  narinfo. If we ever want a stronger check, we can add optional `sha256`
  to the pin block later, and pass it to Nix as `fetchTarball { url = ...;
  sha256 = ...; }` semantics.

## Config Options (`nixops3.toml`)

```toml
[pins]
require_pin          = false     # true: missing pin: block → cycle fails
require_explicit_rev = false     # true: pin without rev → cycle fails
channel_ttl_secs     = 300       # in-process cache TTL for channel resolution
```

No `nixpkgs_retain` — no local cache to size. Nix's GC handles retention.

**Defaults are permissive.** All three tiers work out of the box. Operators
who care about strict convergence flip the flags.

## Error Modes

| Condition | Outcome |
|---|---|
| `pin:` present but malformed YAML | Cycle fails; log error |
| `pin.nixpkgs` present without `channel` | Cycle fails; log error |
| `channel` present, `require_explicit_rev = true` | Cycle fails; log error |
| No `pin:` block, `require_pin = true` | Cycle fails; log error |
| Channel resolution HTTP fails (network, 404, timeout) | Cycle fails; log error |
| Nix download fails at rebuild time | `nixos-rebuild` returns non-zero; standard rebuild-failed handling |

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

**Converged fleet** (Floating tier): all hosts show the same `applied_sha`
and `nixpkgs_channel`; `nixpkgs_rev` may briefly differ during the drift
window after a channel update.

**Misconfiguration:** hosts showing `pin_mode = loose` when the operator
expected pinning — indicates a `main.yaml` missing or incomplete.

## Non-Goals for v0.4

- **Overlays, home-manager, other non-nixpkgs inputs.** The `pin.nixpkgs`
  namespace leaves room for `pin.home-manager`, `pin.overlays.foo`, etc.,
  but v0.4 supports `nixpkgs` only.
- **sha256 verification.** Trust Nix's own narinfo verification.
- **Custom resolver URLs.** channels.nixos.org is hardcoded. Custom nixpkgs
  forks (not on the official channel infrastructure) can only use the Pinned
  tier for now.
- **Binary cache / pre-built closures.** Hosts still evaluate and build.
