#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
RPC_URL="${VECTOR_RPC_URL:-http://127.0.0.1:8899}"
PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_PROGRAM_ID}"

exec "$ROOT_DIR/scripts/run_vector_onchain_harness.sh" \
  --rpc-url "$RPC_URL" \
  --vector-program-id "$PROGRAM_ID" \
  "$@"
