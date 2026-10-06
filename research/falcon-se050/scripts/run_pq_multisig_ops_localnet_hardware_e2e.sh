#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPS_SCRIPT="$ROOT_DIR/scripts/run_pq_multisig_ops.sh"
HYBRID_HARNESS_SCRIPT="$ROOT_DIR/scripts/run_hybrid_harness.sh"
DEFAULT_VECTOR_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
DEFAULT_VAULT_PROGRAM_ID="9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52"
DEFAULT_FEE_PAYER="$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json"
DEFAULT_SAMPLE_TX_B64="aGVsbG8="

RPC_URL="${VECTOR_RPC_URL:-http://127.0.0.1:8899}"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_VECTOR_PROGRAM_ID}"
VAULT_PROGRAM_ID="${PQ_MULTISIG_VAULT_PROGRAM_ID:-$DEFAULT_VAULT_PROGRAM_ID}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$DEFAULT_FEE_PAYER}"
VAULT_SEED="${VAULT_SEED:-ops-hardware-localnet-v1}"
PORT="${ESP_PORT:-}"
PIN="${TEST_PIN:-246810}"
SAMPLE_TX_B64="${SAMPLE_TX_B64:-$DEFAULT_SAMPLE_TX_B64}"
INITIAL_THRESHOLD="${INITIAL_THRESHOLD:-2}"
INITIAL_TTL_SLOTS="${INITIAL_TTL_SLOTS:-512}"
INITIAL_TIMELOCK_SLOTS="${INITIAL_TIMELOCK_SLOTS:-0}"
FIRST_WITHDRAW_LAMPORTS="${FIRST_WITHDRAW_LAMPORTS:-1000000}"
SECOND_WITHDRAW_LAMPORTS="${SECOND_WITHDRAW_LAMPORTS:-750000}"
HARDWARE_PROPOSAL_OFFSET_SLOTS="${HARDWARE_PROPOSAL_OFFSET_SLOTS:-128}"
PREPARE_DEVICE=1

print_usage() {
  cat <<'EOF'
Usage:
  ./scripts/run_pq_multisig_ops_localnet_hardware_e2e.sh --port /dev/cu.usbmodemXXXX [options]

Options:
  --port <path>            Serial device for the real Falcon hardware member
  --pin <digits>           Device PIN. Default: 246810
  --sample-tx-b64 <b64>    Optional sample payload for hybrid harness. Default: "hello"
  --skip-device-prepare    Skip the hybrid harness/device readiness pass
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
      PREPARE_DEVICE=0
      shift
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

if [[ ! "$PIN" =~ ^[0-9]{6}$ ]]; then
  echo "--pin must be exactly 6 digits." >&2
  exit 1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pq-multisig-hw-ops.XXXXXX")"
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
    sleep 0.2
  done
}

pubkey_of() {
  solana-keygen pubkey "$1"
}

echo "============================================================"
echo "PQ Multisig Hardware Localnet E2E"
echo "============================================================"
echo "RPC URL:              $RPC_URL"
echo "Fee payer file:       $FEE_PAYER_FILE"
echo "Vault seed:           $VAULT_SEED"
echo "Port:                 $PORT"
echo "Proposal offset:      $HARDWARE_PROPOSAL_OFFSET_SLOTS"
echo "Temp dir:             $TMP_DIR"
echo "Aliases file:         $ALIASES_FILE"

if [[ "$PREPARE_DEVICE" == "1" ]]; then
  echo
  echo "Prepare hardware member via hybrid harness"
  "$HYBRID_HARNESS_SCRIPT" \
    --port "$PORT" \
    --pin "$PIN" \
    --sample-tx-b64 "$SAMPLE_TX_B64" \
    --no-final-wipe
fi

FEE_PAYER_PUBKEY="$(pubkey_of "$FEE_PAYER_FILE")"

echo
echo "Airdrop fee payer"
solana airdrop 100 "$FEE_PAYER_PUBKEY" -u "$RPC_URL" >/dev/null

echo
echo "Generate fixture Falcon members"
fixture1_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member1.json")"
fixture2_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member2.json")"
echo "$fixture1_out"
echo "$fixture2_out"

FIXTURE1_PDA="$(printf '%s\n' "$fixture1_out" | extract_field "Vector PDA")"
FIXTURE2_PDA="$(printf '%s\n' "$fixture2_out" | extract_field "Vector PDA")"

echo
echo "Initialize hardware Vector PDA"
hardware_init_out="$(
  run_ops \
    --mode initialize-vector \
    --port "$PORT" \
    --pin "$PIN"
)"
echo "$hardware_init_out"
HARDWARE_MEMBER_PDA="$(printf '%s\n' "$hardware_init_out" | extract_field "Vector PDA")"

echo
echo "Initialize fixture Vector PDAs"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member1.json"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member2.json"

cat >"$ALIASES_FILE" <<EOF
{
  "vaults": {
    "hardware-vault": {
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
echo "Initialize FalconOnly hardware vault"
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
echo "Discovery smoke: empty proposal list"
empty_list_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
    --state all \
    --queue all \
    --json
)"
echo "$empty_list_out"
assert_contains "$empty_list_out" "\"matches\": 0"

echo
echo "Create recipient wallet"
solana-keygen new --no-bip39-passphrase --silent -o "$TMP_DIR/recipient.json" >/dev/null
RECIPIENT_PUBKEY="$(pubkey_of "$TMP_DIR/recipient.json")"
echo "Recipient:            $RECIPIENT_PUBKEY"

echo
echo "Fund vault with SOL"
solana transfer \
  -u "$RPC_URL" \
  -k "$FEE_PAYER_FILE" \
  --allow-unfunded-recipient \
  "$VAULT_PDA" \
  0.02 >/dev/null

echo
echo "Hardware creates SOL withdraw proposal"
create_hardware_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
    --member-alias hardware \
    --port "$PORT" \
    --pin "$PIN" \
    --proposal-offset-slots "$HARDWARE_PROPOSAL_OFFSET_SLOTS" \
    --receiver "$RECIPIENT_PUBKEY" \
    --lamports "$FIRST_WITHDRAW_LAMPORTS"
)"
echo "$create_hardware_out"
HARDWARE_PROPOSAL_ID="$(printf '%s\n' "$create_hardware_out" | extract_field "Proposal ID")"
HARDWARE_NOT_BEFORE="$(printf '%s\n' "$create_hardware_out" | extract_field "Not-before slot")"
HARDWARE_CREATE_TX="$(printf '%s\n' "$create_hardware_out" | extract_field "Create tx")"

echo
echo "Discovery smoke: fixture approval queue"
fixture_needs_approval_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
    --state active \
    --queue needs-approval \
    --member-alias fixture1
)"
echo "$fixture_needs_approval_out"
assert_contains "$fixture_needs_approval_out" "needs-approval"
assert_contains "$fixture_needs_approval_out" "withdraw ${FIRST_WITHDRAW_LAMPORTS} lamports"

echo
echo "Discovery smoke: show hardware-authored proposal"
hardware_show_out="$(
  run_ops \
    --mode show-proposal \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
    --proposal-id "$HARDWARE_PROPOSAL_ID"
)"
echo "$hardware_show_out"
assert_contains "$hardware_show_out" "Action kind:          4 (SOL withdraw)"
assert_contains "$hardware_show_out" "Decoded lamports:     $FIRST_WITHDRAW_LAMPORTS"

wait_for_slot "$HARDWARE_NOT_BEFORE"
run_ops \
  --mode approve \
  --aliases-file "$ALIASES_FILE" \
  --vault-alias hardware-vault \
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
echo "Fixture creates SOL withdraw proposal"
create_fixture_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
    --member-alias fixture2 \
    --proposal-offset-slots "$HARDWARE_PROPOSAL_OFFSET_SLOTS" \
    --receiver "$RECIPIENT_PUBKEY" \
    --lamports "$SECOND_WITHDRAW_LAMPORTS"
)"
echo "$create_fixture_out"
FIXTURE_PROPOSAL_ID="$(printf '%s\n' "$create_fixture_out" | extract_field "Proposal ID")"
FIXTURE_NOT_BEFORE="$(printf '%s\n' "$create_fixture_out" | extract_field "Not-before slot")"

echo
echo "Discovery smoke: hardware approval queue"
hardware_needs_approval_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias hardware-vault \
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
    --vault-alias hardware-vault \
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
echo "Final vault state"
run_ops --mode show-vault --vault "$VAULT_PDA"

echo
echo "============================================================"
echo "PQ Multisig Hardware Localnet E2E OK"
echo "============================================================"
echo "Vault:                $VAULT_PDA"
echo "Hardware member PDA:  $HARDWARE_MEMBER_PDA"
echo "Fixture1 PDA:         $FIXTURE1_PDA"
echo "Fixture2 PDA:         $FIXTURE2_PDA"
echo "Hardware create tx:   $HARDWARE_CREATE_TX"
echo "Hardware execute tx:  $HARDWARE_EXECUTE_TX"
echo "Hardware approve tx:  $HARDWARE_APPROVE_TX"
echo "Fixture execute tx:   $FIXTURE_EXECUTE_TX"
