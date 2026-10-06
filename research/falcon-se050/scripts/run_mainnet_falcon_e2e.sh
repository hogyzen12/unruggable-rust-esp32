#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$ROOT_DIR/scripts/run_vector_onchain_harness.sh"

PORT="${PORT:-/dev/cu.usbmodem1101}"
PIN="${PIN:-246810}"
RPC_URL="${RPC_URL:-https://johna-k3cr1v-fast-mainnet.helius-rpc.com}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json}"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU}"
WITHDRAW_RECEIVER="${WITHDRAW_RECEIVER:-}"
WITHDRAW_LAMPORTS="${WITHDRAW_LAMPORTS:-1420000}"
STAKE_VOTE_ACCOUNT="${STAKE_VOTE_ACCOUNT:-}"
STAKE_LAMPORTS="${STAKE_LAMPORTS:-10000000}"
SWAP_INPUT_MINT="${SWAP_INPUT_MINT:-EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v}"
SWAP_OUTPUT_MINT="${SWAP_OUTPUT_MINT:-So11111111111111111111111111111111111111112}"
SWAP_AMOUNT_IN="${SWAP_AMOUNT_IN:-100000}"
SWAP_MIN_OUT="${SWAP_MIN_OUT:-1300000}"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/run_mainnet_falcon_e2e.sh \
    --withdraw-receiver <pubkey> \
    --stake-vote-account <pubkey> \
    [options]

Required:
  --withdraw-receiver <pk>   Receiver for the Falcon SOL send canary
  --stake-vote-account <pk>  Vote account for the Falcon stake canary

Optional:
  --port <path>              Serial device. Default: /dev/cu.usbmodem1101
  --pin <digits>             Device PIN. Default: 246810
  --rpc-url <url>            RPC endpoint
  --fee-payer-file <path>    Local relayer keypair
  --vector-program-id <pk>   Vector Falcon program id
  --withdraw-lamports <n>    Default: 1420000
  --stake-lamports <n>       Default: 10000000
  --swap-input-mint <pk>     Default: USDC
  --swap-output-mint <pk>    Default: WSOL
  --swap-amount-in <n>       Default: 100000
  --swap-min-out <n>         Default: 1300000
  -h, --help                 Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --pin) PIN="$2"; shift 2 ;;
    --rpc-url) RPC_URL="$2"; shift 2 ;;
    --fee-payer-file) FEE_PAYER_FILE="$2"; shift 2 ;;
    --vector-program-id) VECTOR_PROGRAM_ID="$2"; shift 2 ;;
    --withdraw-receiver) WITHDRAW_RECEIVER="$2"; shift 2 ;;
    --withdraw-lamports) WITHDRAW_LAMPORTS="$2"; shift 2 ;;
    --stake-vote-account) STAKE_VOTE_ACCOUNT="$2"; shift 2 ;;
    --stake-lamports) STAKE_LAMPORTS="$2"; shift 2 ;;
    --swap-input-mint) SWAP_INPUT_MINT="$2"; shift 2 ;;
    --swap-output-mint) SWAP_OUTPUT_MINT="$2"; shift 2 ;;
    --swap-amount-in) SWAP_AMOUNT_IN="$2"; shift 2 ;;
    --swap-min-out) SWAP_MIN_OUT="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ -z "$WITHDRAW_RECEIVER" ]]; then
  echo "Missing required --withdraw-receiver" >&2
  usage
  exit 1
fi

if [[ -z "$STAKE_VOTE_ACCOUNT" ]]; then
  echo "Missing required --stake-vote-account" >&2
  usage
  exit 1
fi

common_args=(
  --port "$PORT"
  --pin "$PIN"
  --rpc-url "$RPC_URL"
  --fee-payer-file "$FEE_PAYER_FILE"
  --vector-program-id "$VECTOR_PROGRAM_ID"
)

echo "============================================================"
echo "Falcon Mainnet E2E"
echo "============================================================"
echo "Port:                 $PORT"
echo "RPC URL:              $RPC_URL"
echo "Vector Program:       $VECTOR_PROGRAM_ID"
echo "Withdraw Receiver:    $WITHDRAW_RECEIVER"
echo "Stake Vote Account:   $STAKE_VOTE_ACCOUNT"
echo "Swap Input Mint:      $SWAP_INPUT_MINT"
echo "Swap Output Mint:     $SWAP_OUTPUT_MINT"
echo

echo ">>> Falcon send"
"$HARNESS" \
  "${common_args[@]}" \
  --mode full-withdraw \
  --withdraw-lamports "$WITHDRAW_LAMPORTS" \
  --withdraw-receiver "$WITHDRAW_RECEIVER"

echo
echo ">>> Falcon stake"
"$HARNESS" \
  "${common_args[@]}" \
  --mode full-stake-delegate \
  --stake-vote-account "$STAKE_VOTE_ACCOUNT" \
  --stake-lamports "$STAKE_LAMPORTS"

echo
echo ">>> Falcon swap"
"$HARNESS" \
  "${common_args[@]}" \
  --mode full-raydium-cpmm-swap \
  --swap-input-mint "$SWAP_INPUT_MINT" \
  --swap-output-mint "$SWAP_OUTPUT_MINT" \
  --swap-amount-in "$SWAP_AMOUNT_IN" \
  --swap-min-out "$SWAP_MIN_OUT" \
  --swap-unwrap-sol \
  --no-compute-budget
