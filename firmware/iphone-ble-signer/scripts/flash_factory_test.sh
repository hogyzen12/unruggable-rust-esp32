#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -eq 0 ]]; then
  cat <<'EOF'
Usage:
  ./scripts/flash_factory_test.sh --port /dev/cu.usbmodemXXXX [flash options]

Builds the non-destructive factory PCB diagnostic for the Unruggable V2 dual-
element board, flashes it, and opens the serial monitor. Extra options are
passed to flash_se050_dev.sh (for example --release or display pin overrides).
EOF
  exit 2
fi

exec "$SCRIPT_DIR/flash_se050_dev.sh" \
  --board-profile dual-element-se050 \
  --bin peripheral_probe \
  --monitor \
  "$@"
