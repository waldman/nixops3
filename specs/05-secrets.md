# Secrets — AWS Secrets Manager Integration

## Purpose

NixOS configuration files stored in S3 must never contain secrets. The daemon fetches secrets from AWS Secrets Manager before each `nixos-rebuild` and writes them to a tmpfs directory. NixOS profiles reference these paths directly.

## Namespace Convention

Secrets are named following the hierarchy:

```
NixOps/<abstraction>/<environment>/<role>/<hostname>/<secret-name>
```

Examples:
```
NixOps/home/production/ada/ada-01.example.internal/openrouter-api-key
NixOps/home/production/ada/ada-01.example.internal/whatsapp-allowed-users
NixOps/home/production/zookeeper/zk-01.example.internal/keystore-password
```

The daemon resolves secrets at two levels:

1. **Role-level** (shared across all hosts in the role):
   `NixOps/<abstraction>/<environment>/<role>/shared/<secret-name>`

2. **Host-level** (specific to one host, overrides role-level):
   `NixOps/<abstraction>/<environment>/<role>/<hostname>/<secret-name>`

If the same `<secret-name>` exists at both levels, the host-level value wins.

## Secrets Discovery

The daemon does not maintain a static list of secrets to fetch. Instead, it:

1. Lists all secrets under `NixOps/<role>/` using `ListSecrets` with a path filter.
2. Lists all secrets under `NixOps/<role>/<hostname>/`.
3. Fetches each discovered secret's current value via `GetSecretValue`.

This allows new secrets to be provisioned in AWS Secrets Manager without daemon changes.

## Local Storage

Secrets are written to `/run/nixops3/secrets/` (tmpfs):

| Path | Mode | Content |
|------|------|---------|
| `/run/nixops3/secrets/<secret-name>` | 0400, root:root | Secret value (UTF-8 string) |

The directory is recreated on each daemon start (tmpfs is cleared on reboot). Secrets are refreshed on every poll cycle before `nixos-rebuild`.

## Usage in .nix Files

```nix
# profiles/hermes.nix
{
  systemd.services.hermes-gateway = {
    serviceConfig = {
      EnvironmentFile = "/run/nixops3/secrets/openrouter-api-key";
    };
  };
}
```

Or read directly:

```nix
let
  apiKey = builtins.readFile /run/nixops3/secrets/openrouter-api-key;
in { ... }
```

Note: `builtins.readFile` at nix eval time reads the file content into the Nix store. This means the secret value will appear in the Nix store and in the system configuration. For sensitive secrets, use `EnvironmentFile` or `LoadCredential` in systemd service definitions instead — these are read at service start time, not at build time.

## IAM Policy Requirements

```json
{
  "Effect": "Allow",
  "Action": [
    "secretsmanager:GetSecretValue",
    "secretsmanager:ListSecrets"
  ],
  "Resource": "arn:aws:secretsmanager:<region>:<account>:secret:NixOps/*"
}
```

A tighter policy restricts each machine to its own secrets:

```json
{
  "Resource": [
    "arn:aws:secretsmanager:<region>:<account>:secret:NixOps/<role>/shared/*",
    "arn:aws:secretsmanager:<region>:<account>:secret:NixOps/<role>/<hostname>/*"
  ]
}
```

## Error Handling

On `GetSecretValue` failure for any secret:
- Log the error to journald with the secret name (not value).
- Continue fetching remaining secrets.
- Do NOT abort the apply cycle — a missing secret will cause `nixos-rebuild` to fail if the profile reads it; that failure is surfaced normally.

On `ListSecrets` failure:
- Log the error.
- Skip secrets fetch entirely for this cycle.
- Do NOT abort the apply cycle.

## Secrets and the Hash

Secret values are NOT included in the hash computation. The hash covers only `.nix` files. A secret rotation in AWS Secrets Manager will not trigger a `nixos-rebuild`. If a profile reads a secret at eval time (`builtins.readFile`), the rebuild must be triggered by a config change (even a no-op comment change) to pick up the new value.

Services using `EnvironmentFile` or `LoadCredential` pick up rotated secrets on their next restart, independently of nixops3.
