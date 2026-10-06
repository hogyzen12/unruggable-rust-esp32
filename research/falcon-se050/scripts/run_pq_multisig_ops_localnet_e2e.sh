#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPS_SCRIPT="$ROOT_DIR/scripts/run_pq_multisig_ops.sh"
DEFAULT_VECTOR_PROGRAM_ID="FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU"
DEFAULT_VAULT_PROGRAM_ID="9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52"
DEFAULT_FEE_PAYER="$ROOT_DIR/FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC.json"

RPC_URL="${VECTOR_RPC_URL:-http://127.0.0.1:8899}"
VECTOR_PROGRAM_ID="${VECTOR_PROGRAM_ID:-$DEFAULT_VECTOR_PROGRAM_ID}"
VAULT_PROGRAM_ID="${PQ_MULTISIG_VAULT_PROGRAM_ID:-$DEFAULT_VAULT_PROGRAM_ID}"
FEE_PAYER_FILE="${FEE_PAYER_FILE:-$DEFAULT_FEE_PAYER}"
VAULT_SEED="${VAULT_SEED:-ops-localnet-v1}"
INITIAL_THRESHOLD="${INITIAL_THRESHOLD:-2}"
INITIAL_TTL_SLOTS="${INITIAL_TTL_SLOTS:-512}"
INITIAL_TIMELOCK_SLOTS="${INITIAL_TIMELOCK_SLOTS:-0}"
UPDATED_THRESHOLD="${UPDATED_THRESHOLD:-3}"
UPDATED_TTL_SLOTS="${UPDATED_TTL_SLOTS:-768}"
UPDATED_TIMELOCK_SLOTS="${UPDATED_TIMELOCK_SLOTS:-4}"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pq-multisig-ops.XXXXXX")"
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
echo "PQ Multisig Ops Localnet E2E"
echo "============================================================"
echo "RPC URL:              $RPC_URL"
echo "Fee payer file:       $FEE_PAYER_FILE"
echo "Vault seed:           $VAULT_SEED"
echo "Temp dir:             $TMP_DIR"
echo "Aliases file:         $ALIASES_FILE"

FEE_PAYER_PUBKEY="$(pubkey_of "$FEE_PAYER_FILE")"
echo
echo "Airdrop fee payer"
solana airdrop 100 "$FEE_PAYER_PUBKEY" -u "$RPC_URL" >/dev/null

echo
echo "Generate local Falcon fixtures"
member0_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member0.json")"
member1_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member1.json")"
member2_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member2.json")"
member3_out="$(run_ops --mode generate-member-fixture --output-file "$TMP_DIR/member3.json")"
echo "$member0_out"
echo "$member1_out"
echo "$member2_out"
echo "$member3_out"

MEMBER0_PDA="$(printf '%s\n' "$member0_out" | extract_field "Vector PDA")"
MEMBER1_PDA="$(printf '%s\n' "$member1_out" | extract_field "Vector PDA")"
MEMBER2_PDA="$(printf '%s\n' "$member2_out" | extract_field "Vector PDA")"
MEMBER3_PDA="$(printf '%s\n' "$member3_out" | extract_field "Vector PDA")"

cat >"$ALIASES_FILE" <<EOF
{
  "vaults": {
    "dev-vault": {
      "seed": "$VAULT_SEED"
    }
  },
  "members": {
    "member0": {
      "vector_pda": "$MEMBER0_PDA",
      "fixture_file": "$TMP_DIR/member0.json"
    },
    "member1": {
      "vector_pda": "$MEMBER1_PDA",
      "fixture_file": "$TMP_DIR/member1.json"
    },
    "member2": {
      "vector_pda": "$MEMBER2_PDA",
      "fixture_file": "$TMP_DIR/member2.json"
    },
    "member3": {
      "vector_pda": "$MEMBER3_PDA",
      "fixture_file": "$TMP_DIR/member3.json"
    }
  }
}
EOF

echo
echo "Initialize member Vector PDAs"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member0.json"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member1.json"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member2.json"
run_ops --mode initialize-vector --member-fixture-file "$TMP_DIR/member3.json"

echo
echo "FalconOnly negative test: reject hybrid vault init"
set +e
hybrid_init_out="$(
  run_ops \
    --mode init-vault \
    --vault-seed "${VAULT_SEED}-hybrid-reject" \
    --vault-members "$FEE_PAYER_PUBKEY,$MEMBER0_PDA,$MEMBER1_PDA" \
    --threshold "$INITIAL_THRESHOLD" \
    --proposal-ttl-slots "$INITIAL_TTL_SLOTS" \
    --timelock-slots "$INITIAL_TIMELOCK_SLOTS" \
    2>&1
)"
hybrid_init_status=$?
set -e
echo "$hybrid_init_out"
if [[ $hybrid_init_status -eq 0 ]]; then
  echo "expected FalconOnly init to reject a hybrid member set" >&2
  exit 1
fi
assert_contains "$hybrid_init_out" "is owned by"
assert_contains "$hybrid_init_out" "expected $VECTOR_PROGRAM_ID"

echo
echo "Initialize vault"
init_vault_out="$(
  run_ops \
    --mode init-vault \
    --vault-seed "$VAULT_SEED" \
    --vault-members "$MEMBER0_PDA,$MEMBER1_PDA,$MEMBER2_PDA" \
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
    --vault-alias dev-vault \
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
echo "Create SOL withdraw proposal"
create_sol_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --member-fixture-file "$TMP_DIR/member0.json" \
    --vault "$VAULT_PDA" \
    --receiver "$RECIPIENT_PUBKEY" \
    --lamports 1000000
)"
echo "$create_sol_out"
SOL_PROPOSAL_ID="$(printf '%s\n' "$create_sol_out" | extract_field "Proposal ID")"
SOL_NOT_BEFORE="$(printf '%s\n' "$create_sol_out" | extract_field "Not-before slot")"

echo
echo "Discovery smoke: needs-approval queue"
sol_needs_approval_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias dev-vault \
    --state active \
    --queue needs-approval \
    --member-alias member1
)"
echo "$sol_needs_approval_out"
assert_contains "$sol_needs_approval_out" "needs-approval"
assert_contains "$sol_needs_approval_out" "withdraw 1000000 lamports"

echo
echo "Discovery smoke: show proposal"
sol_show_out="$(
  run_ops \
    --mode show-proposal \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias dev-vault \
    --proposal-id "$SOL_PROPOSAL_ID"
)"
echo "$sol_show_out"
assert_contains "$sol_show_out" "Action kind:          4 (SOL withdraw)"
assert_contains "$sol_show_out" "Decoded lamports:     1000000"

wait_for_slot "$SOL_NOT_BEFORE"
run_ops --mode approve --aliases-file "$ALIASES_FILE" --vault-alias dev-vault --member-alias member1 --proposal-id "$SOL_PROPOSAL_ID"

echo
echo "Discovery smoke: executable queue"
sol_executable_out="$(
  run_ops \
    --mode list-proposals \
    --aliases-file "$ALIASES_FILE" \
    --vault-alias dev-vault \
    --state active \
    --queue executable
)"
echo "$sol_executable_out"
assert_contains "$sol_executable_out" "executable"
assert_contains "$sol_executable_out" "withdraw 1000000 lamports"

sol_execute_out="$(run_ops --mode execute --vault "$VAULT_PDA" --proposal-id "$SOL_PROPOSAL_ID")"
echo "$sol_execute_out"
SOL_EXECUTE_TX="$(printf '%s\n' "$sol_execute_out" | extract_field "Execute tx")"

echo
echo "Create revoke/cancel/close proposal"
revocable_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --member-fixture-file "$TMP_DIR/member0.json" \
    --vault "$VAULT_PDA" \
    --receiver "$RECIPIENT_PUBKEY" \
    --lamports 500000
)"
echo "$revocable_out"
REVOCABLE_PROPOSAL_ID="$(printf '%s\n' "$revocable_out" | extract_field "Proposal ID")"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member1.json" --vault "$VAULT_PDA" --proposal-id "$REVOCABLE_PROPOSAL_ID"
run_ops --mode revoke --member-fixture-file "$TMP_DIR/member1.json" --vault "$VAULT_PDA" --proposal-id "$REVOCABLE_PROPOSAL_ID"
run_ops --mode cancel --member-fixture-file "$TMP_DIR/member0.json" --vault "$VAULT_PDA" --proposal-id "$REVOCABLE_PROPOSAL_ID"

echo
echo "Discovery smoke: closable queue"
closable_out="$(
  run_ops \
    --mode list-proposals \
    --vault "$VAULT_PDA" \
    --state all \
    --queue closable
)"
echo "$closable_out"
assert_contains "$closable_out" "closable"
assert_contains "$closable_out" "withdraw 500000 lamports"

close_out="$(run_ops --mode close --vault "$VAULT_PDA" --proposal-id "$REVOCABLE_PROPOSAL_ID")"
echo "$close_out"
REVOCABLE_CLOSE_TX="$(printf '%s\n' "$close_out" | extract_field "Close tx")"

echo
echo "Prepare SPL fixture"
create_token_out="$(spl-token -u "$RPC_URL" create-token --decimals 6 --fee-payer "$FEE_PAYER_FILE")"
MINT_PUBKEY="$(printf '%s\n' "$create_token_out" | awk '/Creating token/ {print $3}')"
create_source_ata_out="$(spl-token -u "$RPC_URL" create-account "$MINT_PUBKEY" --owner "$VAULT_PDA" --fee-payer "$FEE_PAYER_FILE")"
SOURCE_ATA="$(printf '%s\n' "$create_source_ata_out" | awk '/Creating account/ {print $3}')"
create_dest_ata_out="$(spl-token -u "$RPC_URL" create-account "$MINT_PUBKEY" --owner "$RECIPIENT_PUBKEY" --fee-payer "$FEE_PAYER_FILE")"
DEST_ATA="$(printf '%s\n' "$create_dest_ata_out" | awk '/Creating account/ {print $3}')"
spl-token -u "$RPC_URL" mint "$MINT_PUBKEY" 1 "$SOURCE_ATA" --fee-payer "$FEE_PAYER_FILE" >/dev/null
echo "Mint:                 $MINT_PUBKEY"
echo "Vault source ATA:     $SOURCE_ATA"
echo "Recipient dest ATA:   $DEST_ATA"

echo
echo "Create SPL transfer proposal"
spl_out="$(
  run_ops \
    --mode create-spl-transfer \
    --member-fixture-file "$TMP_DIR/member0.json" \
    --vault "$VAULT_PDA" \
    --spl-mint "$MINT_PUBKEY" \
    --spl-destination "$DEST_ATA" \
    --spl-amount 250000 \
    --spl-decimals 6
)"
echo "$spl_out"
SPL_PROPOSAL_ID="$(printf '%s\n' "$spl_out" | extract_field "Proposal ID")"
SPL_NOT_BEFORE="$(printf '%s\n' "$spl_out" | extract_field "Not-before slot")"
wait_for_slot "$SPL_NOT_BEFORE"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member2.json" --vault "$VAULT_PDA" --proposal-id "$SPL_PROPOSAL_ID"
spl_execute_out="$(run_ops --mode execute --vault "$VAULT_PDA" --proposal-id "$SPL_PROPOSAL_ID")"
echo "$spl_execute_out"
SPL_EXECUTE_TX="$(printf '%s\n' "$spl_execute_out" | extract_field "Execute tx")"

echo
echo "Create config update proposal"
config_out="$(
  run_ops \
    --mode create-config-update \
    --member-fixture-file "$TMP_DIR/member1.json" \
    --vault "$VAULT_PDA" \
    --threshold "$UPDATED_THRESHOLD" \
    --proposal-ttl-slots "$UPDATED_TTL_SLOTS" \
    --timelock-slots "$UPDATED_TIMELOCK_SLOTS"
)"
echo "$config_out"
CONFIG_PROPOSAL_ID="$(printf '%s\n' "$config_out" | extract_field "Proposal ID")"
CONFIG_NOT_BEFORE="$(printf '%s\n' "$config_out" | extract_field "Not-before slot")"
wait_for_slot "$CONFIG_NOT_BEFORE"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member2.json" --vault "$VAULT_PDA" --proposal-id "$CONFIG_PROPOSAL_ID"
config_execute_out="$(run_ops --mode execute --vault "$VAULT_PDA" --proposal-id "$CONFIG_PROPOSAL_ID")"
echo "$config_execute_out"
CONFIG_EXECUTE_TX="$(printf '%s\n' "$config_execute_out" | extract_field "Execute tx")"

echo
echo "Create replace-member proposal"
replace_out="$(
  run_ops \
    --mode create-replace-member \
    --member-fixture-file "$TMP_DIR/member0.json" \
    --vault "$VAULT_PDA" \
    --replace-member-index 2 \
    --new-member-pda "$MEMBER3_PDA"
)"
echo "$replace_out"
REPLACE_PROPOSAL_ID="$(printf '%s\n' "$replace_out" | extract_field "Proposal ID")"
REPLACE_NOT_BEFORE="$(printf '%s\n' "$replace_out" | extract_field "Not-before slot")"
wait_for_slot "$REPLACE_NOT_BEFORE"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member1.json" --vault "$VAULT_PDA" --proposal-id "$REPLACE_PROPOSAL_ID"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member2.json" --vault "$VAULT_PDA" --proposal-id "$REPLACE_PROPOSAL_ID"
replace_execute_out="$(run_ops --mode execute --vault "$VAULT_PDA" --proposal-id "$REPLACE_PROPOSAL_ID")"
echo "$replace_execute_out"
REPLACE_EXECUTE_TX="$(printf '%s\n' "$replace_execute_out" | extract_field "Execute tx")"

echo
echo "Accept pending member"
accept_out="$(run_ops --mode accept-pending-member --member-fixture-file "$TMP_DIR/member3.json" --vault "$VAULT_PDA")"
echo "$accept_out"
ACCEPT_TX="$(printf '%s\n' "$accept_out" | extract_field "Accept tx")"

echo
echo "Prove rotated-out member is rejected"
if run_ops --mode approve --member-fixture-file "$TMP_DIR/member2.json" --vault "$VAULT_PDA" --proposal-id "$SOL_PROPOSAL_ID" >/dev/null 2>&1; then
  echo "expected rotated-out member approval to fail" >&2
  exit 1
fi
echo "Rotated-out member approval rejected as expected"

echo
echo "Create post-rotation 3-of-3 proposal"
post_rotation_out="$(
  run_ops \
    --mode create-sol-withdraw \
    --member-fixture-file "$TMP_DIR/member3.json" \
    --vault "$VAULT_PDA" \
    --receiver "$RECIPIENT_PUBKEY" \
    --lamports 250000
)"
echo "$post_rotation_out"
POST_PROPOSAL_ID="$(printf '%s\n' "$post_rotation_out" | extract_field "Proposal ID")"
POST_NOT_BEFORE="$(printf '%s\n' "$post_rotation_out" | extract_field "Not-before slot")"
wait_for_slot "$POST_NOT_BEFORE"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member0.json" --vault "$VAULT_PDA" --proposal-id "$POST_PROPOSAL_ID"
run_ops --mode approve --member-fixture-file "$TMP_DIR/member1.json" --vault "$VAULT_PDA" --proposal-id "$POST_PROPOSAL_ID"
post_execute_out="$(run_ops --mode execute --vault "$VAULT_PDA" --proposal-id "$POST_PROPOSAL_ID")"
echo "$post_execute_out"
POST_EXECUTE_TX="$(printf '%s\n' "$post_execute_out" | extract_field "Execute tx")"

echo
echo "Final vault state"
run_ops --mode show-vault --vault "$VAULT_PDA"

echo
echo "============================================================"
echo "PQ Multisig Ops Localnet E2E OK"
echo "============================================================"
echo "Vault:                $VAULT_PDA"
echo "SOL execute tx:       $SOL_EXECUTE_TX"
echo "Close tx:             $REVOCABLE_CLOSE_TX"
echo "SPL execute tx:       $SPL_EXECUTE_TX"
echo "Config execute tx:    $CONFIG_EXECUTE_TX"
echo "Replace execute tx:   $REPLACE_EXECUTE_TX"
echo "Accept tx:            $ACCEPT_TX"
echo "Post-rotation tx:     $POST_EXECUTE_TX"
