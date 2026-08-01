# Secrets

## Why not in S3

NixOS configuration files stored in S3 must never contain secrets. S3 bucket policies can be broad, CI/CD systems may log file contents, and `.nix` files end up in the Nix store which is world-readable.

Instead, secrets live in AWS Secrets Manager and are fetched to a tmpfs directory (`/run/nixops3/secrets/`) before every `nixos-rebuild`. The tmpfs is cleared on every reboot.

## Namespace convention

Secrets are named following the role hierarchy:

```
NixOps/<role>/shared/<secret-name>       ← shared across all hosts in the role
NixOps/<role>/<hostname>/<secret-name>   ← specific to one host
```

Examples:

```
NixOps/home/production/app/shared/example-api-key
NixOps/home/production/app/app-01.example.internal/example-token
NixOps/home/production/zookeeper/shared/keystore-password
NixOps/home/production/zookeeper/zk-01.example.internal/keystore-password
```

If the same `<secret-name>` exists at both levels, the **host-level value wins**.

## Discovery

The daemon does not maintain a static list of secrets. Each cycle it:

1. Lists all secrets under `NixOps/<role>/shared/`
2. Lists all secrets under `NixOps/<role>/<hostname>/`
3. Fetches each discovered secret via `GetSecretValue`
4. Writes each to `/run/nixops3/secrets/<secret-name>` (mode `0400`, owner `root:root`)

Adding a new secret in AWS Secrets Manager takes effect on the next poll cycle — no daemon changes required.

## Local secret files

| Path | Mode | Content |
|------|------|---------|
| `/run/nixops3/secrets/<name>` | `0400 root:root` | Secret value (UTF-8) |

The directory `/run/nixops3/secrets/` is mode `0700 root:root`.

## Using secrets in .nix files

### Recommended: EnvironmentFile (secrets stay out of Nix store)

```nix
# profiles/my-api-service.nix
{ pkgs, ... }:
{
  systemd.services.my-api = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      ExecStart = "${pkgs.my-api}/bin/my-api";
      EnvironmentFile = "/run/nixops3/secrets/api-key";
    };
  };
}
```

The secret file should contain `KEY=value` lines for `EnvironmentFile`. Alternatively use `LoadCredential` for systemd credentials:

```nix
serviceConfig = {
  LoadCredential = "api-key:/run/nixops3/secrets/api-key";
  ExecStart = "${pkgs.my-api}/bin/my-api --credential-file %d/api-key";
};
```

### For non-systemd use

Some tools read secrets from files directly:

```nix
{ ... }:
{
  services.some-tool = {
    secretFile = "/run/nixops3/secrets/some-tool-password";
  };
}
```

### Avoid: builtins.readFile

```nix
# Don't do this for sensitive values:
let secret = builtins.readFile /run/nixops3/secrets/api-key;
```

`builtins.readFile` copies the secret into the Nix store at eval time. The Nix store is world-readable (`/nix/store/...`). Use this only for non-sensitive values.

## Secrets and the hash

Secret values are **not** included in the hash computation. The hash covers only `.nix` files. Rotating a secret in AWS Secrets Manager does not trigger a `nixos-rebuild`.

Services using `EnvironmentFile` or `LoadCredential` pick up rotated secrets on their next restart. Services that used `builtins.readFile` at eval time require a config change to pick up the new value (even a no-op comment change will do).

## IAM requirements

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

Tighter policy restricting each machine to its own secrets:

```json
{
  "Effect": "Allow",
  "Action": [
    "secretsmanager:GetSecretValue",
    "secretsmanager:ListSecrets"
  ],
  "Resource": [
    "arn:aws:secretsmanager:<region>:<account>:secret:NixOps/<role>/shared/*",
    "arn:aws:secretsmanager:<region>:<account>:secret:NixOps/<role>/<hostname>/*"
  ]
}
```

## Error handling

- **`GetSecretValue` failure for one secret** — logged to journald with the secret name (never the value). Remaining secrets continue to be fetched. The apply cycle continues; if a profile tries to read the missing secret, `nixos-rebuild` will fail and the error will be surfaced normally.
- **`ListSecrets` failure** — logged. Secrets fetch is skipped entirely for this cycle. Apply cycle continues without updated secrets.

Neither error aborts the cycle or blocks config apply.

## Provisioning secrets

Create a secret:

```bash
aws secretsmanager create-secret \
  --name "NixOps/home/production/app/shared/example-api-key" \
  --secret-string "sk-..."
```

Rotate a secret:

```bash
aws secretsmanager put-secret-value \
  --secret-id "NixOps/home/production/app/shared/example-api-key" \
  --secret-string "sk-new..."
```

The new value is picked up on the machine's next poll cycle.
