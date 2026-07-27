#!/usr/bin/env bash
# Pre-tool hook: enforces spec-before-code discipline autonomously.
#
# Before editing any code file in this project, Claude MUST:
#   1. Read the relevant spec in specs/
#   2. Write a spec-ack file: .claude/spec-ack
#      Format: spec: <filename>  (e.g. "spec: 02-daemon.md")
#   3. Then make the code edit — this hook validates and allows it.
#
# The ack is valid for 10 minutes. After that it is stale and Claude
# must re-ack (read the spec again and re-create the file).
#
# Claude writes the ack via Bash — no user action required.
#
# Exit codes:
#   0  — allow the tool call
#   2  — block the tool call

set -euo pipefail

PROJECT_ROOT="/home/leon/fun/waldman/code/nixops3"
SPECS_DIR="$PROJECT_ROOT/specs"
ACK_FILE="$PROJECT_ROOT/.claude/spec-ack"
ACK_MAX_AGE_SECS=600   # 10 minutes

# Parse file_path from the JSON Claude Code sends on stdin
input=$(cat)
file_path=$(python3 -c "
import json, sys
d = json.load(sys.stdin)
print(d.get('file_path', d.get('path', '')))
" <<< "$input" 2>/dev/null || echo "")

# Only enforce on files within this project
[[ -z "$file_path" || "$file_path" != "$PROJECT_ROOT"* ]] && exit 0

# Writes to .claude/ itself (ack file, hooks, settings) are always allowed
[[ "$file_path" == "$PROJECT_ROOT/.claude"* ]] && exit 0

# Writes to specs/ are always allowed
[[ "$file_path" == "$SPECS_DIR"* ]] && exit 0

# Only enforce on code files
ext="${file_path##*.}"
case "$ext" in
  rs|sh|nix|toml) : ;;
  *) exit 0 ;;
esac

# ── Validate spec-ack ────────────────────────────────────────────────────────

block() {
  cat >&2 <<EOF

╔══════════════════════════════════════════════════════════════════╗
║  SPEC-CHECK BLOCKED                                              ║
╠══════════════════════════════════════════════════════════════════╣
║  $1
╠══════════════════════════════════════════════════════════════════╣
║  TO PROCEED:                                                     ║
║  1. Read the relevant spec in specs/                             ║
║  2. Create the ack file before making the code edit:            ║
║                                                                  ║
║     printf 'spec: 02-daemon.md\n' > .claude/spec-ack            ║
║                                                                  ║
║  Specs: $(ls "$SPECS_DIR"/*.md 2>/dev/null | xargs -I{} basename {} | tr '\n' ' ')
╚══════════════════════════════════════════════════════════════════╝
EOF
  exit 2
}

if [[ ! -f "$ACK_FILE" ]]; then
  block "No spec-ack found. Read the spec first.                        "
fi

# Check age
ack_age=$(( $(date +%s) - $(stat -c %Y "$ACK_FILE") ))
if (( ack_age > ACK_MAX_AGE_SECS )); then
  block "spec-ack is stale (${ack_age}s old, max ${ACK_MAX_AGE_SECS}s). Re-read the spec."
fi

# Extract spec reference
spec_ref=$(grep -m1 "^spec:" "$ACK_FILE" | sed 's/spec:[[:space:]]*//' | tr -d '[:space:]')
if [[ -z "$spec_ref" ]]; then
  block "spec-ack has no 'spec: <filename>' line.                       "
fi

# Validate the referenced spec actually exists
if [[ ! -f "$SPECS_DIR/$spec_ref" ]]; then
  block "spec-ack references '$spec_ref' which does not exist in specs/ "
fi

# All checks passed — allow the edit
exit 0
