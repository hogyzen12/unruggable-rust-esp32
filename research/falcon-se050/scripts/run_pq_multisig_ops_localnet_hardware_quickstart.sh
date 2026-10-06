#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VALIDATOR_SCRIPT="$ROOT_DIR/scripts/start_pq_multisig_local_validator.sh"
E2E_SCRIPT="$ROOT_DIR/scripts/run_pq_multisig_ops_localnet_hardware_e2e.sh"
DEFAULT_FEE_PAYER="$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json"

RPC_URL="${VECTOR_RPC_URL:-http://127.0.0.1:8899}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$DEFAULT_FEE_PAYER}"
PORT="${ESP_PORT:-}"
PIN="${TEST_PIN:-246810}"
SAMPLE_TX_B64="${SAMPLE_TX_B64:-aGVsbG8=}"
BUILD_FIRST=1
KEEP_VALIDATOR=0
SKIP_DEVICE_PREPARE=0
LOG_FILE=""

print_usage() {
  cat <<'EOF'
Usage:
  ./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh --port /dev/cu.usbmodemXXXX [options]

Options:
  --port <path>            Serial device for the real Falcon hardware member
  --pin <digits>           Device PIN. Default: 246810
  --sample-tx-b64 <b64>    Optional sample payload for hybrid harness. Default: "hello"
  --skip-device-prepare    Skip the hybrid harness/device readiness pass
  --no-build               Skip rebuilding the local programs before starting validator
  --keep-validator         Leave the validator running after the E2E flow finishes
  --log-file <path>        Write validator logs to the given file
  -h, --help               Show this help text
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="$2"
      shift 2
      ;;
    --pin)
      PIN="$2"
      shift 2
      ;;
    --sample-tx-b64)
      SAMPLE_TX_B64="$2"
      shift 2
      ;;
    --skip-device-prepare)
      SKIP_DEVICE_PREPARE=1
      shift
      ;;
    --no-build)
      BUILD_FIRST=0
      shift
      ;;
    --keep-validator)
      KEEP_VALIDATOR=1
      shift
      ;;
    --log-file)
      LOG_FILE="$2"
      shift 2
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$PORT" ]]; then
  echo "Missing required --port value." >&2
  exit 1
fi

if [[ -z "$LOG_FILE" ]]; then
  LOG_FILE="$(mktemp -t pq-multisig-hardware-validator)"
fi

VALIDATOR_PID=""

cleanup() {
  local status=$?
  if [[ -n "$VALIDATOR_PID" ]] && kill -0 "$VALIDATOR_PID" 2>/dev/null; then
    if [[ "$KEEP_VALIDATOR" == "1" && "$status" == "0" ]]; then
      echo
      echo "Validator still running:"
      echo "PID:                  $VALIDATOR_PID"
      echo "RPC URL:              $RPC_URL"
      echo "Log file:             $LOG_FILE"
    else
      kill "$VALIDATOR_PID" 2>/dev/null || true
      wait "$VALIDATOR_PID" 2>/dev/null || true
    fi
  fi
  if [[ "$status" != "0" ]]; then
    echo
    echo "Hardware quickstart failed. Validator log tail:" >&2
    tail -n 120 "$LOG_FILE" >&2 || true
  fi
  exit "$status"
}
trap cleanup EXIT

wait_for_rpc() {
  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if [[ -n "$VALIDATOR_PID" ]] && ! kill -0 "$VALIDATOR_PID" 2>/dev/null; then
      return 1
    fi
    local current_slot
    current_slot="$(solana slot -u "$RPC_URL" 2>/dev/null | tr -d '[:space:]' || true)"
    if [[ "$current_slot" =~ ^[0-9]+$ ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

echo "============================================================"
echo "PQ Multisig Hardware Localnet Quickstart"
echo "============================================================"
echo "RPC URL:              $RPC_URL"
echo "Fee payer file:       $FEE_PAYER_FILE"
echo "Port:                 $PORT"
echo "Validator log:        $LOG_FILE"

validator_args=(
  --mint-keypair "$FEE_PAYER_FILE"
)
if [[ "$BUILD_FIRST" == "1" ]]; then
  validator_args=(--build "${validator_args[@]}")
fi

"$VALIDATOR_SCRIPT" "${validator_args[@]}" >"$LOG_FILE" 2>&1 &
VALIDATOR_PID=$!

echo "Starting validator..."
if ! wait_for_rpc; then
  echo "validator did not become ready in time" >&2
  exit 1
fi

echo "Validator ready. Running hardware E2E..."
e2e_args=(
  --port "$PORT"
  --pin "$PIN"
  --sample-tx-b64 "$SAMPLE_TX_B64"
)
if [[ "$SKIP_DEVICE_PREPARE" == "1" ]]; then
  e2e_args+=(--skip-device-prepare)
fi
"$E2E_SCRIPT" "${e2e_args[@]}"

echo
echo "Hardware quickstart complete."
