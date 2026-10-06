#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VECTOR_DIR="$ROOT_DIR/vector"
VAULT_DIR="$ROOT_DIR/pq-multisig-vault"
DEFAULT_VECTOR_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
DEFAULT_VAULT_PROGRAM_ID="9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_VECTOR_PROGRAM_ID}"
VAULT_PROGRAM_ID="${PQ_MULTISIG_VAULT_PROGRAM_ID:-$DEFAULT_VAULT_PROGRAM_ID}"
VECTOR_PROGRAM_SO="$VECTOR_DIR/target/deploy/vector_falcon512.so"
VAULT_PROGRAM_SO="$VAULT_DIR/target/deploy/pq_multisig_vault_program.so"
LEDGER_DIR="$ROOT_DIR/.localnet/pq-multisig-ledger"
RPC_PORT=8899
FAUCET_PORT=9900
BIND_ADDRESS="127.0.0.1"
MINT_KEYPAIR="${SOLANA_KEYPAIR:-}"
MINT_ADDRESS=""
DO_BUILD=0

print_usage() {
  cat <<'EOF'
Usage:
  ./scripts/start_pq_multisig_local_validator.sh [options]

Options:
  --build                     Build the local Vector Falcon and PQ vault SBF programs first
  --vector-program-id <id>    Vector Falcon program id to load into the local validator
  --vault-program-id <id>     PQ vault program id to load into the local validator
  --ledger-dir <path>         Validator ledger directory
  --rpc-port <port>           RPC port. Default: 8899
  --faucet-port <port>        Faucet port. Default: 9900
  --bind-address <addr>       Bind address. Default: 127.0.0.1
  --mint-keypair <path>       Keypair file whose pubkey receives genesis mint
  --mint-address <base58>     Address that receives genesis mint
  -h, --help                  Show this help text
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --build)
      DO_BUILD=1
      shift
      ;;
    --vector-program-id)
      VECTOR_PROGRAM_ID="$2"
      shift 2
      ;;
    --vault-program-id)
      VAULT_PROGRAM_ID="$2"
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
    --faucet-port)
      FAUCET_PORT="$2"
      shift 2
      ;;
    --bind-address)
      BIND_ADDRESS="$2"
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
  "$ROOT_DIR/scripts/build_pq_multisig_vault_program.sh"
fi

if [[ ! -f "$VECTOR_PROGRAM_SO" ]]; then
  echo "Missing $VECTOR_PROGRAM_SO" >&2
  echo "Build it first with ./scripts/build_vector_falcon_program.sh or rerun with --build." >&2
  exit 1
fi

if [[ ! -f "$VAULT_PROGRAM_SO" ]]; then
  echo "Missing $VAULT_PROGRAM_SO" >&2
  echo "Build it first with ./scripts/build_pq_multisig_vault_program.sh or rerun with --build." >&2
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
  --faucet-port "$FAUCET_PORT"
  --bind-address "$BIND_ADDRESS"
  --bpf-program "$VECTOR_PROGRAM_ID" "$VECTOR_PROGRAM_SO"
  --bpf-program "$VAULT_PROGRAM_ID" "$VAULT_PROGRAM_SO"
  --account SysvarRent111111111111111111111111111111111 "$VECTOR_DIR/fixtures/rent.json"
  --reset
)

if [[ -n "$MINT_ADDRESS" ]]; then
  cmd+=(--mint "$MINT_ADDRESS")
fi

echo "PQ Multisig local validator"
echo "Vector Program ID: $VECTOR_PROGRAM_ID"
echo "Vector Program SO: $VECTOR_PROGRAM_SO"
echo "Vault Program ID:  $VAULT_PROGRAM_ID"
echo "Vault Program SO:  $VAULT_PROGRAM_SO"
echo "Ledger Dir:        $LEDGER_DIR"
echo "Bind Address:      $BIND_ADDRESS"
echo "RPC URL:           http://$BIND_ADDRESS:$RPC_PORT"
echo "WS URL:            ws://$BIND_ADDRESS:$((RPC_PORT + 1))"
echo "Faucet URL:        http://$BIND_ADDRESS:$FAUCET_PORT"
if [[ -n "$MINT_ADDRESS" ]]; then
  echo "Mint Addr:         $MINT_ADDRESS"
fi

exec "${cmd[@]}"
