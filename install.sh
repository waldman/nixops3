#!/usr/bin/env bash
# nixops3d installer
# Usage: curl -fsSL https://raw.githubusercontent.com/waldman/nixops3/master/install.sh | bash
#        sudo bash install.sh --bucket my-bucket --region us-east-1 --role home/production/webserver
set -euo pipefail

REPO="waldman/nixops3"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/nixops3"
SERVICE_FILE="/etc/systemd/system/nixops3d.service"
BINARY="nixops3d"

# --- args ---
BUCKET="" REGION="" ROLE="" TABLE="" ACCESS_KEY="" SECRET_KEY="" TTL_DAYS=""

usage() {
  cat <<EOF
Usage: sudo bash install.sh [OPTIONS]

OPTIONS (--bucket, --region, and --role are required together to write a live config):
  --bucket         BUCKET    S3 bucket name
  --region         REGION    AWS region (e.g. us-east-1)
  --role           ROLE      Role path in the bucket (e.g. home/production/webserver)
  --table          TABLE     DynamoDB inventory table name (optional)
  --ttl-days       DAYS      Inventory TTL in days (default: 2 × poll interval)
  --access-key-id  KEY       AWS access key ID (optional; omit to use instance role or env)
  --secret-key     SECRET    AWS secret access key (required if --access-key-id is set)

Without options: installs the binary and writes a placeholder config.
You must edit $CONFIG_DIR/nixops3.toml before starting the daemon.
EOF
}

while [[ $# -gt 0 ]]; do
  case $1 in
    --bucket)        BUCKET="$2";     shift 2 ;;
    --region)        REGION="$2";     shift 2 ;;
    --role)          ROLE="$2";       shift 2 ;;
    --table)         TABLE="$2";      shift 2 ;;
    --ttl-days)      TTL_DAYS="$2";   shift 2 ;;
    --access-key-id) ACCESS_KEY="$2"; shift 2 ;;
    --secret-key)    SECRET_KEY="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1"; usage >&2; exit 1 ;;
  esac
done

if [[ -n "$BUCKET" || -n "$REGION" || -n "$ROLE" ]]; then
  if [[ -z "$BUCKET" || -z "$REGION" || -z "$ROLE" ]]; then
    echo "Error: --bucket, --region, and --role must all be provided together." >&2
    exit 1
  fi
fi

if [[ -n "$ACCESS_KEY" && -z "$SECRET_KEY" ]] || [[ -z "$ACCESS_KEY" && -n "$SECRET_KEY" ]]; then
  echo "Error: --access-key-id and --secret-key must be provided together." >&2
  exit 1
fi

if [[ $EUID -ne 0 ]]; then
  echo "Error: must run as root (sudo bash $0 ...)" >&2
  exit 1
fi

ARCH=$(uname -m)
if [[ "$ARCH" != "x86_64" ]]; then
  echo "Error: unsupported architecture '$ARCH' — only x86_64 is supported." >&2
  exit 1
fi

# --- download ---
echo "Fetching latest release..."
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

if [[ -z "$LATEST" ]]; then
  echo "Error: could not determine latest release from GitHub API." >&2
  exit 1
fi

echo "Downloading nixops3d ${LATEST}..."
TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

curl -fsSL "https://github.com/${REPO}/releases/download/${LATEST}/${BINARY}" -o "$TMP"
chmod +x "$TMP"

# --- install binary ---
mkdir -p "$INSTALL_DIR"
install -m 755 "$TMP" "$INSTALL_DIR/$BINARY"
echo "Binary installed: $INSTALL_DIR/$BINARY"

# --- config ---
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
TOML="$CONFIG_DIR/nixops3.toml"

if [[ -n "$BUCKET" ]]; then
  {
    echo "bucket = \"$BUCKET\""
    echo "region = \"$REGION\""
    echo "role   = \"$ROLE\""
    if [[ -n "$ACCESS_KEY" ]]; then
      printf '\n[aws]\naccess_key_id     = "%s"\nsecret_access_key = "%s"\n' \
        "$ACCESS_KEY" "$SECRET_KEY"
    fi
    if [[ -n "$TABLE" ]]; then
      printf '\n[inventory]\nenabled = true\ntable   = "%s"\n' "$TABLE"
      if [[ -n "$TTL_DAYS" ]]; then
        printf 'ttl_secs = %d\n' $((TTL_DAYS * 86400))
      fi
    fi
  } > "$TOML"
  chmod 600 "$TOML"
  echo "Config written: $TOML"
else
  if [[ ! -f "$TOML" ]]; then
    cat > "$TOML" <<'EOF'
# nixops3d configuration — edit before starting the daemon
# Full reference: https://github.com/waldman/nixops3/blob/master/docs/configuration.md

bucket = "your-bucket-name"
region = "us-east-1"
role   = "your/role/path"

# Uncomment to enable fleet inventory (DynamoDB):
# [inventory]
# enabled = true
# table   = "nixops3-inventory"
EOF
    chmod 600 "$TOML"
    echo "Placeholder config written: $TOML"
    echo "  --> Edit it before starting the daemon."
  else
    echo "Config already exists, not overwriting: $TOML"
  fi
fi

# --- systemd unit (skipped on NixOS) ---
IS_NIXOS=0
[[ -f /etc/NIXOS ]] && IS_NIXOS=1

if [[ $IS_NIXOS -eq 1 ]]; then
  cat <<'EOF'

NixOS detected — skipping systemd unit installation.
On NixOS, /etc/systemd/system/ is read-only and managed by nixos-rebuild.

Bootstrap steps:
  1. Run the daemon once manually to apply the S3 config:
       sudo /usr/local/bin/nixops3d
  2. The first cycle runs nixos-rebuild switch, which installs and starts
     the nixops3d service from your S3 role config (profiles/nixops3d.nix).
  3. After nixos-rebuild completes, kill the manual process:
       sudo pkill nixops3d
  4. The service is now owned by NixOS and starts automatically on boot:
       systemctl status nixops3d
EOF
else
  cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=NixOpS3 configuration daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/$BINARY
Restart=on-failure
RestartSec=30s
RuntimeDirectory=nixops3 nixops3/secrets
RuntimeDirectoryMode=0700
StateDirectory=nixops3
StateDirectoryMode=0755

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  echo "Service installed: $SERVICE_FILE"

  if [[ -n "$BUCKET" ]]; then
    systemctl enable --now nixops3d
    echo ""
    echo "nixops3d is running. Follow logs:"
    echo "  journalctl -u nixops3d -f"
  else
    echo ""
    echo "Next steps:"
    echo "  1. Edit $TOML"
    echo "  2. systemctl enable --now nixops3d"
    echo "  3. journalctl -u nixops3d -f"
  fi
fi
