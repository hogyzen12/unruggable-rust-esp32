#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VECTOR_DIR="$ROOT_DIR/vector"
DEFAULT_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_PROGRAM_ID}"
PROGRAM_SO="$VECTOR_DIR/target/deploy/vector_falcon512.so"
LEDGER_DIR="$ROOT_DIR/.localnet/vector-falcon-ledger"
RPC_PORT=8899
MINT_KEYPAIR="${SOLANA_KEYPAIR:-}"
MINT_ADDRESS=""
DO_BUILD=0

print_usage() {
  cat <<'EOF'
Usage:
  ./scripts/start_vector_falcon_local_validator.sh [options]

Options:
  --build                  Build the local Vector Falcon SBF program first
  --program-id <base58>    Program id to load into the local validator
  --ledger-dir <path>      Validator ledger directory
  --rpc-port <port>        RPC port. Default: 8899
  --mint-keypair <path>    Keypair file whose pubkey receives genesis mint
  --mint-address <base58>  Address that receives genesis mint
  -h, --help               Show this help text
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --build)
      DO_BUILD=1
      shift
      ;;
    --program-id)
      PROGRAM_ID="$2"
      shift 2
      ;;
    --ledger-dir)
      LEDGER_DIR="$2"
      shift 2
      ;;
    --rpc-port)
      RPC_PORT="$2"
      shift 2
      ;;
    --mint-keypair)
      MINT_KEYPAIR="$2"
      shift 2
      ;;
    --mint-address)
      MINT_ADDRESS="$2"
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

if [[ "$DO_BUILD" == "1" ]]; then
  "$ROOT_DIR/scripts/build_vector_falcon_program.sh"
fi

if [[ ! -f "$PROGRAM_SO" ]]; then
  echo "Missing $PROGRAM_SO" >&2
  echo "Build it first with ./scripts/build_vector_falcon_program.sh or rerun with --build." >&2
  exit 1
fi

if [[ -z "$MINT_ADDRESS" && -n "$MINT_KEYPAIR" ]]; then
  MINT_ADDRESS="$(solana-keygen pubkey "$MINT_KEYPAIR")"
fi

mkdir -p "$LEDGER_DIR"

cmd=(
  solana-test-validator
  --ledger "$LEDGER_DIR"
  --rpc-port "$RPC_PORT"
  --bpf-program "$PROGRAM_ID" "$PROGRAM_SO"
  --account SysvarRent111111111111111111111111111111111 "$VECTOR_DIR/fixtures/rent.json"
  --reset
)

if [[ -n "$MINT_ADDRESS" ]]; then
  cmd+=(--mint "$MINT_ADDRESS")
fi

echo "Vector Falcon local validator"
echo "Program ID:  $PROGRAM_ID"
echo "Program SO:  $PROGRAM_SO"
echo "Ledger Dir:  $LEDGER_DIR"
echo "RPC URL:     http://127.0.0.1:$RPC_PORT"
echo "WS URL:      ws://127.0.0.1:$((RPC_PORT + 1))"
if [[ -n "$MINT_ADDRESS" ]]; then
  echo "Mint Addr:   $MINT_ADDRESS"
fi

exec "${cmd[@]}"
