#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASH_COMPLETION_FILE="$ROOT_DIR/scripts/completions/pq-multisig-ops.bash"
ZSH_COMPLETION_FILE="$ROOT_DIR/scripts/completions/_pq-multisig-ops"

cat <<EOF
PQ multisig ops completion files are checked in:

  Bash: $BASH_COMPLETION_FILE
  Zsh:  $ZSH_COMPLETION_FILE

To enable them for the current shell session:

  Bash:
    source "$BASH_COMPLETION_FILE"

  Zsh:
    fpath=("$ROOT_DIR/scripts/completions" \$fpath)
    autoload -Uz compinit && compinit

To persist them, add the same lines to your shell rc file.
EOF
