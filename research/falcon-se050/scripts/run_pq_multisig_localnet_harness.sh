#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_VECTOR_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
DEFAULT_VAULT_PROGRAM_ID="9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52"
DEFAULT_FEE_PAYER="$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json"
RPC_URL="${VECTOR_RPC_URL:-http://127.0.0.1:8899}"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_VECTOR_PROGRAM_ID}"
VAULT_PROGRAM_ID="${PQ_MULTISIG_VAULT_PROGRAM_ID:-$DEFAULT_VAULT_PROGRAM_ID}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$DEFAULT_FEE_PAYER}"

exec cargo +stable run --offline --target aarch64-apple-darwin \
  --manifest-path "$ROOT_DIR/tools/pq-multisig-harness/Cargo.toml" \
  -- \
  --rpc-url "$RPC_URL" \
  --fee-payer-file "$FEE_PAYER_FILE" \
  --vector-program-id "$VECTOR_PROGRAM_ID" \
  --vault-program-id "$VAULT_PROGRAM_ID" \
  "$@"
