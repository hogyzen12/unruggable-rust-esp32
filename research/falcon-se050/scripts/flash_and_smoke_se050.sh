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
ALLOW_WIPE=0

usage() {
  cat <<'EOF'
Usage:
  ./scripts/flash_and_smoke_se050.sh --port /dev/cu.usbmodemXXXX [options]

Options:
  --pin <digits>         6-digit PIN used for smoke testing. Default: 246810
  --sample-tx-b64 <b64>  Optional serialized Solana transaction for SIGN test.
  --release              Use the release artifact.
  --skip-build           Reuse the existing artifact in target/.
  --erase                Run espflash erase-flash before flashing.
  --no-erase             Skip espflash erase-flash before flashing.
  --wait-secs <n>        Seconds to wait after flashing before smoke test. Default: 3
  --allow-wipe           Required: this smoke test sends WIPE_KEYS at start and end.
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
    --allow-wipe)
      ALLOW_WIPE=1
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

if [[ "$ALLOW_WIPE" -ne 1 ]]; then
  echo "Refusing to run: scripts/flash_and_smoke_se050.sh is destructive and sends WIPE_KEYS at the start and end." >&2
  echo "Re-run with --allow-wipe if you explicitly want blank-device smoke coverage." >&2
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

if [[ -n "$SAMPLE_TX_B64_VALUE" ]]; then
  ESP_PORT="$PORT" TEST_PIN="$PIN" SAMPLE_TX_B64="$SAMPLE_TX_B64_VALUE" python3 "$ROOT_DIR/test_firmware.py"
else
  ESP_PORT="$PORT" TEST_PIN="$PIN" python3 "$ROOT_DIR/test_firmware.py"
fi
