#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT=""
PIN="${TEST_PIN:-246810}"
PROFILE="debug"
SKIP_BUILD=0
ERASE=0
WAIT_SECS=3
SAMPLE_TX_B64_VALUE="${SAMPLE_TX_B64:-}"
HARNESS_ARGS=()
FINAL_WIPE=0

usage() {
  cat <<'EOF'
Usage:
  ./scripts/flash_and_hybrid_harness.sh --port /dev/cu.usbmodemXXXX [options]

Options:
  --pin <digits>         6-digit PIN used for setup and testing. Default: 246810
  --sample-tx-b64 <b64>  Optional payload for SIGN and SIGN_HYBRID verification.
  --release              Use the release firmware artifact.
  --skip-build           Reuse the existing firmware artifact in target/.
  --erase                Run espflash erase-flash before flashing.
  --no-erase             Skip espflash erase-flash before flashing.
  --wait-secs <n>        Seconds to wait after flashing before the harness. Default: 3
  --final-wipe           Wipe the device at the end of the harness. Default: preserve state.
  --no-final-wipe        Preserve device state after the harness completes.
  -h, --help             Show this help text.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    --pin)
      PIN="${2:-}"
      shift 2
      ;;
    --sample-tx-b64)
      SAMPLE_TX_B64_VALUE="${2:-}"
      shift 2
      ;;
    --release)
      PROFILE="release"
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --erase)
      ERASE=1
      shift
      ;;
    --no-erase)
      ERASE=0
      shift
      ;;
    --wait-secs)
      WAIT_SECS="${2:-}"
      shift 2
      ;;
    --final-wipe)
      FINAL_WIPE=1
      shift
      ;;
    --no-final-wipe)
      FINAL_WIPE=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$PORT" ]]; then
  echo "Missing required --port value." >&2
  usage >&2
  exit 1
fi

if [[ ! "$PIN" =~ ^[0-9]{6}$ ]]; then
  echo "--pin must be exactly 6 digits." >&2
  exit 1
fi

FLASH_ARGS=(--port "$PORT")
if [[ "$PROFILE" == "release" ]]; then
  FLASH_ARGS+=(--release)
fi
if [[ "$SKIP_BUILD" -eq 1 ]]; then
  FLASH_ARGS+=(--skip-build)
fi
if [[ "$ERASE" -eq 1 ]]; then
  FLASH_ARGS+=(--erase)
fi

"$ROOT_DIR/scripts/flash_se050_dev.sh" "${FLASH_ARGS[@]}"

sleep "$WAIT_SECS"

HARNESS_ARGS+=(--port "$PORT" --pin "$PIN")
if [[ -n "$SAMPLE_TX_B64_VALUE" ]]; then
  HARNESS_ARGS+=(--sample-tx-b64 "$SAMPLE_TX_B64_VALUE")
fi
if [[ "$FINAL_WIPE" -eq 0 ]]; then
  HARNESS_ARGS+=(--no-final-wipe)
fi

"$ROOT_DIR/scripts/run_hybrid_harness.sh" "${HARNESS_ARGS[@]}"
