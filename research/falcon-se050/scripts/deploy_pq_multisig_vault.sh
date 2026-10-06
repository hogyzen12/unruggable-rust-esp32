#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_SCRIPT="$ROOT_DIR/scripts/build_pq_multisig_vault_program.sh"
PROGRAM_SO="${PQ_MULTISIG_VAULT_SO:-$ROOT_DIR/pq-multisig-vault/target/deploy/pq_multisig_vault_program.so}"

RPC_URL="${RPC_URL:-https://johna-k3cr1v-fast-mainnet.helius-rpc.com}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json}"
UPGRADE_AUTHORITY_FILE="${UPGRADE_AUTHORITY_FILE:-$FEE_PAYER_FILE}"
PROGRAM_KEYPAIR_FILE="${PROGRAM_KEYPAIR_FILE:-}"
SKIP_BUILD=0

usage() {
  cat <<'EOF'
Usage:
  ./scripts/deploy_pq_multisig_vault.sh \
    --program-keypair-file <path> \
    [options]

Required:
  --program-keypair-file <path>  Program keypair JSON used to derive the vault program id

Optional:
  --rpc-url <url>                RPC endpoint. Default: Helius mainnet
  --fee-payer-file <path>        Fee payer keypair. Default: ./FaLCnr...
  --upgrade-authority-file <p>   Upgrade authority keypair. Default: fee payer
  --program-so <path>            Built SBF artifact. Default: pq-multisig-vault/target/deploy/pq_multisig_vault_program.so
  --skip-build                   Do not rebuild before deploy
  -h, --help                     Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --rpc-url) RPC_URL="$2"; shift 2 ;;
    --fee-payer-file) FEE_PAYER_FILE="$2"; shift 2 ;;
    --upgrade-authority-file) UPGRADE_AUTHORITY_FILE="$2"; shift 2 ;;
    --program-keypair-file) PROGRAM_KEYPAIR_FILE="$2"; shift 2 ;;
    --program-so) PROGRAM_SO="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ -z "$PROGRAM_KEYPAIR_FILE" ]]; then
  echo "Missing required --program-keypair-file" >&2
  usage
  exit 1
fi

if [[ "$SKIP_BUILD" != "1" ]]; then
  "$BUILD_SCRIPT"
fi

if [[ ! -f "$PROGRAM_SO" ]]; then
  echo "Program artifact not found: $PROGRAM_SO" >&2
  exit 1
fi

if [[ ! -f "$PROGRAM_KEYPAIR_FILE" ]]; then
  echo "Program keypair file not found: $PROGRAM_KEYPAIR_FILE" >&2
  exit 1
fi

if [[ ! -f "$FEE_PAYER_FILE" ]]; then
  echo "Fee payer file not found: $FEE_PAYER_FILE" >&2
  exit 1
fi

if [[ ! -f "$UPGRADE_AUTHORITY_FILE" ]]; then
  echo "Upgrade authority file not found: $UPGRADE_AUTHORITY_FILE" >&2
  exit 1
fi

PROGRAM_ID="$(solana-keygen pubkey "$PROGRAM_KEYPAIR_FILE")"
PROGRAM_SIZE_BYTES="$(wc -c < "$PROGRAM_SO" | tr -d '[:space:]')"

echo "============================================================"
echo "Deploy PQ Multisig Vault"
echo "============================================================"
echo "RPC URL:              $RPC_URL"
echo "Program SO:           $PROGRAM_SO"
echo "Program size bytes:   $PROGRAM_SIZE_BYTES"
echo "Program keypair:      $PROGRAM_KEYPAIR_FILE"
echo "Program id:           $PROGRAM_ID"
echo "Fee payer:            $FEE_PAYER_FILE"
echo "Upgrade authority:    $UPGRADE_AUTHORITY_FILE"
echo

solana program deploy \
  --url "$RPC_URL" \
  --fee-payer "$FEE_PAYER_FILE" \
  --upgrade-authority "$UPGRADE_AUTHORITY_FILE" \
  --program-id "$PROGRAM_KEYPAIR_FILE" \
  "$PROGRAM_SO"

echo
echo ">>> Program show"
solana program show --url "$RPC_URL" "$PROGRAM_ID"

