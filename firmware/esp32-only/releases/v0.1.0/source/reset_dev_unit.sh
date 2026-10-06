#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="/dev/tty.usbserial-0001"
SKIP_BUILD=1
BATCH_TAG="reset-dev-unit"
LOG_CSV=""

usage() {
  cat <<'EOF'
Usage: ./reset_dev_unit.sh [options]

Options:
  --port <serial-port>   Serial port (default: /dev/tty.usbserial-0001)
  --build                Rebuild before reflashing instead of reusing the existing artifact
  --batch-tag <tag>      Optional batch tag written to the CSV log
  --log-csv <path>       Optional CSV path for the reflashed blank-state log
  -h, --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    --build)
      SKIP_BUILD=0
      shift
      ;;
    --batch-tag)
      BATCH_TAG="${2:-}"
      shift 2
      ;;
    --log-csv)
      LOG_CSV="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

CMD=("${ROOT_DIR}/provision_dev.sh" "--port" "$PORT" "--skip-efuse-check")
if [[ "$SKIP_BUILD" -eq 1 ]]; then
  CMD+=("--skip-build")
fi
if [[ -n "$BATCH_TAG" ]]; then
  CMD+=("--batch-tag" "$BATCH_TAG")
fi
if [[ -n "$LOG_CSV" ]]; then
  CMD+=("--log-csv" "$LOG_CSV")
fi

echo "This will erase the device, wipe any QA wallet/PIN state, and reflash blank dev firmware."
echo "If espflash stalls at 'Connecting...', hold BOOT, tap EN once, keep holding BOOT until"
echo "'Using flash stub' appears, then release BOOT."
echo

exec "${CMD[@]}"
