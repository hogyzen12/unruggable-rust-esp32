#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPS_SCRIPT="$ROOT_DIR/scripts/run_pq_multisig_ops.sh"
HYBRID_HARNESS_SCRIPT="$ROOT_DIR/scripts/run_hybrid_harness.sh"

DEFAULT_RPC_URL="https://johna-k3cr1v-fast-mainnet.helius-rpc.com"
DEFAULT_VECTOR_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
DEFAULT_FEE_PAYER="$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json"
DEFAULT_SAMPLE_TX_B64="aGVsbG8="

RPC_URL="${RPC_URL:-$DEFAULT_RPC_URL}"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_VECTOR_PROGRAM_ID}"
VAULT_PROGRAM_ID="${PQ_MULTISIG_VAULT_PROGRAM_ID:-}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$DEFAULT_FEE_PAYER}"
PORT="${ESP_PORT:-}"
PIN="${TEST_PIN:-246810}"
SAMPLE_TX_B64="${SAMPLE_TX_B64:-$DEFAULT_SAMPLE_TX_B64}"
VAULT_SEED="${VAULT_SEED:-falcon-mainnet-canary-v1}"
WITHDRAW_RECEIVER="${WITHDRAW_RECEIVER:-}"
FUND_VAULT_SOL="${FUND_VAULT_SOL:-0.005}"
INITIAL_THRESHOLD="${INITIAL_THRESHOLD:-2}"
INITIAL_TTL_SLOTS="${INITIAL_TTL_SLOTS:-512}"
INITIAL_TIMELOCK_SLOTS="${INITIAL_TIMELOCK_SLOTS:-0}"
FIRST_WITHDRAW_LAMPORTS="${FIRST_WITHDRAW_LAMPORTS:-1000000}"
SECOND_WITHDRAW_LAMPORTS="${SECOND_WITHDRAW_LAMPORTS:-750000}"
PROPOSAL_OFFSET_SLOTS="${PROPOSAL_OFFSET_SLOTS:-128}"
PREPARE_DEVICE=0
PREPARE_INITIAL_WIPE=0

usage() {
  cat <<'EOF'
Usage:
  ./scripts/run_pq_multisig_ops_mainnet_hardware_canary.sh \
    --port /dev/cu.usbmodemXXXX \
    --vault-program-id <pubkey> \
    [options]

Required:
  --port <path>                 Serial device for the real Falcon hardware member
  --vault-program-id <pubkey>   Deployed pq_multisig_vault program id on mainnet

Optional:
  --pin <digits>                Device PIN. Default: 246810
  --rpc-url <url>               RPC endpoint. Default: Helius mainnet
  --fee-payer-file <path>       Fee payer keypair. Default: ./FaLCnr...
  --vector-program-id <pubkey>  Mainnet Vector Falcon program id
  --vault-seed <string>         Vault seed. Default: falcon-mainnet-canary-v1
  --withdraw-receiver <pubkey>  SOL receiver. Default: fee payer pubkey
  --fund-vault-sol <amount>     Initial vault funding in SOL. Default: 0.005
  --first-withdraw-lamports <n> Default: 1000000
  --second-withdraw-lamports <n> Default: 750000
  --proposal-offset-slots <n>   Default: 128
  --prepare-device              Run hybrid harness first and leave device configured
  --prepare-initial-wipe        With --prepare-device, wipe the device first
  --sample-tx-b64 <b64>         Optional sample payload for prepare flow
  -h, --help                    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --vault-program-id) VAULT_PROGRAM_ID="$2"; shift 2 ;;
    --pin) PIN="$2"; shift 2 ;;
    --rpc-url) RPC_URL="$2"; shift 2 ;;
    --fee-payer-file) FEE_PAYER_FILE="$2"; shift 2 ;;
    --vector-program-id) VECTOR_PROGRAM_ID="$2"; shift 2 ;;
    --vault-seed) VAULT_SEED="$2"; shift 2 ;;
    --withdraw-receiver) WITHDRAW_RECEIVER="$2"; shift 2 ;;
    --fund-vault-sol) FUND_VAULT_SOL="$2"; shift 2 ;;
    --first-withdraw-lamports) FIRST_WITHDRAW_LAMPORTS="$2"; shift 2 ;;
    --second-withdraw-lamports) SECOND_WITHDRAW_LAMPORTS="$2"; shift 2 ;;
    --proposal-offset-slots) PROPOSAL_OFFSET_SLOTS="$2"; shift 2 ;;
    --prepare-device) PREPARE_DEVICE=1; shift ;;
    --prepare-initial-wipe) PREPARE_INITIAL_WIPE=1; shift ;;
    --sample-tx-b64) SAMPLE_TX_B64="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ -z "$PORT" ]]; then
  echo "Missing required --port value." >&2
  usage
  exit 1
fi

if [[ -z "$VAULT_PROGRAM_ID" ]]; then
  echo "Missing required --vault-program-id value." >&2
  usage
  exit 1
fi

if [[ ! "$PIN" =~ ^[0-9]{6}$ ]]; then
  echo "--pin must be exactly 6 digits." >&2
  exit 1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pq-multisig-mainnet.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
ALIASES_FILE="$TMP_DIR/aliases.json"

run_ops() {
  "$OPS_SCRIPT" \
    --rpc-url "$RPC_URL" \
    --fee-payer-file "$FEE_PAYER_FILE" \
    --vector-program-id "$VECTOR_PROGRAM_ID" \
    --vault-program-id "$VAULT_PROGRAM_ID" \
    "$@"
}

extract_field() {
  local key="$1"
  awk -F': *' -v key="$key" '$1 == key {print $2; exit}'
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if ! printf '%s\n' "$haystack" | rg -F -q "$needle"; then
    echo "expected output to contain: $needle" >&2
    echo "--- output ---" >&2
    printf '%s\n' "$haystack" >&2
    exit 1
  fi
}

wait_for_slot() {
  local target_slot="$1"
  while true; do
    current_slot="$(solana slot -u "$RPC_URL" | tr -d '[:space:]')"
    if [[ "$current_slot" =~ ^[0-9]+$ ]] && (( current_slot >= target_slot )); then
      return 0
    fi
    sleep 0.5
  done
}

pubkey_of() {
  solana-keygen pubkey "$1"
}

FEE_PAYER_PUBKEY="$(pubkey_of "$FEE_PAYER_FILE")"
if [[ -z "$WITHDRAW_RECEIVER" ]]; then
  WITHDRAW_RECEIVER="$FEE_PAYER_PUBKEY"
fi

echo "============================================================"
echo "PQ Multisig Mainnet Hardware Canary"
echo "============================================================"
echo "RPC URL:              $RPC_URL"
echo "Vector Program:       $VECTOR_PROGRAM_ID"
echo "Vault Program:        $VAULT_PROGRAM_ID"
echo "Fee payer file:       $FEE_PAYER_FILE"
echo "Fee payer pubkey:     $FEE_PAYER_PUBKEY"
echo "Vault seed:           $VAULT_SEED"
echo "Port:                 $PORT"
echo "Withdraw receiver:    $WITHDRAW_RECEIVER"
echo "Fund vault SOL:       $FUND_VAULT_SOL"
echo "Temp dir:             $TMP_DIR"
echo "Aliases file:         $ALIASES_FILE"

if [[ "$PREPARE_INITIAL_WIPE" == "1" && "$PREPARE_DEVICE" != "1" ]]; then
  echo "--prepare-initial-wipe requires --prepare-device" >&2
  exit 1
fi

if [[ "$PREPARE_DEVICE" == "1" ]]; then
  echo
  echo "Prepare hardware member via hybrid harness"
  prepare_args=(
    --port "$PORT"
    --pin "$PIN"
    --sample-tx-b64 "$SAMPLE_TX_B64"
    --no-final-wipe
  )
  if [[ "$PREPARE_INITIAL_WIPE" == "1" ]]; then
    prepare_args+=(--initial-wipe)
  fi
  "$HYBRID_HARNESS_SCRIPT" "${prepare_args[@]}"
fi

echo
echo "Generate fixture Falcon members"
fixture1_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member1.json")"
fixture2_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member2.json")"
echo "$fixture1_out"
echo "$fixture2_out"

FIXTURE1_PDA="$(printf '%s\n' "$fixture1_out" | extract_field "Vector PDA")"
FIXTURE2_PDA="$(printf '%s\n' "$fixture2_out" | extract_field "Vector PDA")"

echo
echo "Initialize hardware Vector PDA on mainnet"
hardware_init_out="$(
  run_ops \
    --mode initialize-vector \
    --port "$PORT" \
    --pin "$PIN"
)"
echo "$hardware_init_out"
HARDWARE_MEMBER_PDA="$(printf '%s\n' "$hardware_init_out" | extract_field "Vector PDA")"

echo
echo "Initialize fixture Vector PDAs on mainnet"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member1.json"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member2.json"

cat >"$ALIASES_FILE" <<EOF
{
  "vaults": {
    "mainnet-canary": {
      "seed": "$VAULT_SEED"
    }
  },
  "members": {
    "hardware": {
      "vector_pda": "$HARDWARE_MEMBER_PDA"
    },
    "fixture1": {
      "vector_pda": "$FIXTURE1_PDA",
      "fixture_file": "$TMP_DIR/member1.json"
    },
    "fixture2": {
      "vector_pda": "$FIXTURE2_PDA",
      "fixture_file": "$TMP_DIR/member2.json"
    }
  }
}
EOF

echo
echo "Initialize FalconOnly mainnet vault"
init_vault_out="$(
  run_ops \
    --mode init-vault \
    --vault-seed "$VAULT_SEED" \
    --vault-members "$HARDWARE_MEMBER_PDA,$FIXTURE1_PDA,$FIXTURE2_PDA" \
    --threshold "$INITIAL_THRESHOLD" \
    --proposal-ttl-slots "$INITIAL_TTL_SLOTS" \
    --timelock-slots "$INITIAL_TIMELOCK_SLOTS"
)"
echo "$init_vault_out"
VAULT_PDA="$(printf '%s\n' "$init_vault_out" | extract_field "Vault PDA")"

echo
echo "Fund vault with SOL"
solana transfer \
  -u "$RPC_URL" \
  -k "$FEE_PAYER_FILE" \
  --allow-unfunded-recipient \
  "$VAULT_PDA" \
  "$FUND_VAULT_SOL" >/dev/null

echo
echo "Hardware creates SOL withdraw proposal"
create_hardware_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias mainnet-canary \
    --member-alias hardware \
    --port "$PORT" \
    --pin "$PIN" \
    --proposal-offset-slots "$PROPOSAL_OFFSET_SLOTS" \
    --receiver "$WITHDRAW_RECEIVER" \
    --lamports "$FIRST_WITHDRAW_LAMPORTS"
)"
echo "$create_hardware_out"
HARDWARE_PROPOSAL_ID="$(printf '%s\n' "$create_hardware_out" | extract_field "Proposal ID")"
HARDWARE_NOT_BEFORE="$(printf '%s\n' "$create_hardware_out" | extract_field "Not-before slot")"
HARDWARE_CREATE_TX="$(printf '%s\n' "$create_hardware_out" | extract_field "Create tx")"

echo
echo "Fixture approval queue"
fixture_needs_approval_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias mainnet-canary \
    --state active \
    --queue needs-approval \
    --member-alias fixture1
)"
echo "$fixture_needs_approval_out"
assert_contains "$fixture_needs_approval_out" "needs-approval"
assert_contains "$fixture_needs_approval_out" "withdraw ${FIRST_WITHDRAW_LAMPORTS} lamports"

wait_for_slot "$HARDWARE_NOT_BEFORE"
echo
echo "Fixture approves hardware-authored proposal"
run_ops \
  --mode approve \
  --aliases-file "$ALIASES_FILE" \
  --vault-alias mainnet-canary \
  --member-alias fixture1 \
  --proposal-id "$HARDWARE_PROPOSAL_ID"

echo
echo "Execute hardware-authored proposal"
hardware_execute_out="$(
  run_ops \
    --mode execute \
    --vault "$VAULT_PDA" \
    --proposal-id "$HARDWARE_PROPOSAL_ID"
)"
echo "$hardware_execute_out"
HARDWARE_EXECUTE_TX="$(printf '%s\n' "$hardware_execute_out" | extract_field "Execute tx")"

echo
echo "Close hardware-authored proposal"
hardware_close_out="$(
  run_ops \
    --mode close \
    --vault "$VAULT_PDA" \
    --proposal-id "$HARDWARE_PROPOSAL_ID"
)"
echo "$hardware_close_out"
HARDWARE_CLOSE_TX="$(printf '%s\n' "$hardware_close_out" | extract_field "Close tx")"

echo
echo "Fixture creates SOL withdraw proposal"
create_fixture_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias mainnet-canary \
    --member-alias fixture2 \
    --proposal-offset-slots "$PROPOSAL_OFFSET_SLOTS" \
    --receiver "$WITHDRAW_RECEIVER" \
    --lamports "$SECOND_WITHDRAW_LAMPORTS"
)"
echo "$create_fixture_out"
FIXTURE_PROPOSAL_ID="$(printf '%s\n' "$create_fixture_out" | extract_field "Proposal ID")"
FIXTURE_NOT_BEFORE="$(printf '%s\n' "$create_fixture_out" | extract_field "Not-before slot")"
FIXTURE_CREATE_TX="$(printf '%s\n' "$create_fixture_out" | extract_field "Create tx")"

echo
echo "Hardware approval queue"
hardware_needs_approval_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias mainnet-canary \
    --state active \
    --queue needs-approval \
    --member-alias hardware \
    --port "$PORT" \
    --pin "$PIN"
)"
echo "$hardware_needs_approval_out"
assert_contains "$hardware_needs_approval_out" "needs-approval"
assert_contains "$hardware_needs_approval_out" "withdraw ${SECOND_WITHDRAW_LAMPORTS} lamports"

wait_for_slot "$FIXTURE_NOT_BEFORE"
echo
echo "Hardware approves fixture-authored proposal"
hardware_approve_out="$(
  run_ops \
    --mode approve \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias mainnet-canary \
    --member-alias hardware \
    --port "$PORT" \
    --pin "$PIN" \
    --proposal-id "$FIXTURE_PROPOSAL_ID"
)"
echo "$hardware_approve_out"
HARDWARE_APPROVE_TX="$(printf '%s\n' "$hardware_approve_out" | extract_field "Approve tx")"

echo
echo "Execute fixture-authored proposal"
fixture_execute_out="$(
  run_ops \
    --mode execute \
    --vault "$VAULT_PDA" \
    --proposal-id "$FIXTURE_PROPOSAL_ID"
)"
echo "$fixture_execute_out"
FIXTURE_EXECUTE_TX="$(printf '%s\n' "$fixture_execute_out" | extract_field "Execute tx")"

echo
echo "Close fixture-authored proposal"
fixture_close_out="$(
  run_ops \
    --mode close \
    --vault "$VAULT_PDA" \
    --proposal-id "$FIXTURE_PROPOSAL_ID"
)"
echo "$fixture_close_out"
FIXTURE_CLOSE_TX="$(printf '%s\n' "$fixture_close_out" | extract_field "Close tx")"

echo
echo "Final vault state"
run_ops --mode show-vault --vault "$VAULT_PDA"

echo
echo "============================================================"
echo "PQ Multisig Mainnet Hardware Canary OK"
echo "============================================================"
echo "Vault:                $VAULT_PDA"
echo "Hardware member PDA:  $HARDWARE_MEMBER_PDA"
echo "Fixture1 PDA:         $FIXTURE1_PDA"
echo "Fixture2 PDA:         $FIXTURE2_PDA"
echo "Hardware create tx:   $HARDWARE_CREATE_TX"
echo "Hardware execute tx:  $HARDWARE_EXECUTE_TX"
echo "Hardware close tx:    $HARDWARE_CLOSE_TX"
echo "Fixture create tx:    $FIXTURE_CREATE_TX"
echo "Hardware approve tx:  $HARDWARE_APPROVE_TX"
echo "Fixture execute tx:   $FIXTURE_EXECUTE_TX"
echo "Fixture close tx:     $FIXTURE_CLOSE_TX"

