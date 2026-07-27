#!/usr/bin/env bash
# nixops3 installer
# Usage: curl -fsSL https://raw.githubusercontent.com/waldman/nixops3/master/install.sh | sudo bash -s -- [OPTIONS]
set -euo pipefail

REPO="waldman/nixops3"
INSTALL_DIR="/usr/local/bin"
BINARY="nixops3"

# --- args ---
BUCKET="" REGION="" ROLE="" HOSTNAME="" TABLE="" TTL_DAYS="" ACCESS_KEY="" SECRET_KEY=""

usage() {
  cat <<EOF
Usage: sudo bash install.sh [OPTIONS]

  --bucket         BUCKET    S3 bucket name
  --region         REGION    AWS region (e.g. us-east-1)
  --role           ROLE      Role path (e.g. home/production/generic_node)
  --hostname       NAME      Machine hostname (sets system hostname)
  --table          TABLE     DynamoDB inventory table (optional)
  --ttl-days       DAYS      Inventory TTL in days (optional)
  --access-key-id  KEY       AWS access key ID (optional; omit for instance role)
  --secret-key     SECRET    AWS secret key (required if --access-key-id is set)
EOF
}

while [[ $# -gt 0 ]]; do
  case $1 in
    --bucket)        BUCKET="$2";     shift 2 ;;
    --region)        REGION="$2";     shift 2 ;;
    --role)          ROLE="$2";       shift 2 ;;
    --hostname)      HOSTNAME="$2";   shift 2 ;;
    --table)         TABLE="$2";      shift 2 ;;
    --ttl-days)      TTL_DAYS="$2";   shift 2 ;;
    --access-key-id) ACCESS_KEY="$2"; shift 2 ;;
    --secret-key)    SECRET_KEY="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ -z "$BUCKET" || -z "$REGION" || -z "$ROLE" ]]; then
  echo "Error: --bucket, --region, and --role are required." >&2
  exit 1
fi

if [[ $EUID -ne 0 ]]; then
  echo "Error: must run as root (sudo bash $0 ...)" >&2
  exit 1
fi

# --- download ---
echo "Fetching latest release..."
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

[[ -z "$LATEST" ]] && { echo "Error: could not determine latest release." >&2; exit 1; }

echo "Downloading nixops3 ${LATEST}..."
mkdir -p "$INSTALL_DIR"
curl -fsSL "https://github.com/${REPO}/releases/download/${LATEST}/${BINARY}" \
  -o "$INSTALL_DIR/$BINARY"
chmod +x "$INSTALL_DIR/$BINARY"
echo "Installed: $INSTALL_DIR/$BINARY"

# --- NixOS: ensure root has a nixos channel so nixpkgs is findable ---
if [[ -f /etc/NIXOS ]] && [[ ! -d /nix/var/nix/profiles/per-user/root/channels ]]; then
  NIXOS_VERSION=$(. /etc/os-release 2>/dev/null && printf '%s' "${VERSION_ID:-}")
  if [[ -n "$NIXOS_VERSION" ]]; then
    echo "NixOS: adding nixos-${NIXOS_VERSION} channel for root..."
    nix-channel --add "https://nixos.org/channels/nixos-${NIXOS_VERSION}" nixos
    nix-channel --update nixos
  else
    echo "Warning: NixOS detected but VERSION_ID missing in /etc/os-release." >&2
    echo "  Run manually: nix-channel --add https://nixos.org/channels/nixos-<version> nixos && nix-channel --update" >&2
  fi
fi

# --- build bootstrap args and run ---
ARGS=(bootstrap --bucket "$BUCKET" --region "$REGION" --role "$ROLE")
[[ -n "$HOSTNAME"   ]] && ARGS+=(--hostname "$HOSTNAME")
[[ -n "$TABLE"      ]] && ARGS+=(--table "$TABLE")
[[ -n "$TTL_DAYS"   ]] && ARGS+=(--ttl-days "$TTL_DAYS")
[[ -n "$ACCESS_KEY" ]] && ARGS+=(--access-key-id "$ACCESS_KEY" --secret-key "$SECRET_KEY")

echo "Running bootstrap..."
"$INSTALL_DIR/$BINARY" "${ARGS[@]}"
