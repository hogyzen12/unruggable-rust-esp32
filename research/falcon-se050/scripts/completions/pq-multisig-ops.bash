_pq_multisig_ops_complete() {
  local cur prev mode
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"
  mode=""

  for ((i=1; i<${#COMP_WORDS[@]}; i++)); do
    if [[ "${COMP_WORDS[i]}" == "--mode" && $((i + 1)) -lt ${#COMP_WORDS[@]} ]]; then
      mode="${COMP_WORDS[i+1]}"
    fi
  done

  local modes="generate-member-fixture initialize-vector init-vault show-vault list-proposals show-proposal create-sol-withdraw create-spl-transfer create-config-update create-replace-member approve revoke cancel accept-pending-member execute close"
  local states="active executed cancelled expired all"
  local queues="all needs-approval executable closable"
  local action_kinds="sol-withdraw spl-transfer config-update replace-member all"
  local common_flags="--mode --rpc-url --fee-payer-file --vector-program-id --vault-program-id --commitment --json --manifest-file --manifest-only --aliases-file --vault --vault-seed --vault-alias --member --member-alias --proposal-id --state --queue --action-kind --member-fixture-file --port --pin -h --help"

  case "$prev" in
    --mode)
      COMPREPLY=( $(compgen -W "$modes" -- "$cur") )
      return
      ;;
    --state)
      COMPREPLY=( $(compgen -W "$states" -- "$cur") )
      return
      ;;
    --queue)
      COMPREPLY=( $(compgen -W "$queues" -- "$cur") )
      return
      ;;
    --action-kind)
      COMPREPLY=( $(compgen -W "$action_kinds" -- "$cur") )
      return
      ;;
    --fee-payer-file|--aliases-file|--member-fixture-file|--manifest-file|--output-file)
      COMPREPLY=( $(compgen -f -- "$cur") )
      return
      ;;
  esac

  local mode_flags=""
  case "$mode" in
    generate-member-fixture)
      mode_flags="--output-file --overwrite"
      ;;
    initialize-vector)
      mode_flags="--member-fixture-file --port --pin --read-timeout-ms --action-timeout-ms --open-wait-ms"
      ;;
    init-vault)
      mode_flags="--vault-members --threshold --proposal-ttl-slots --timelock-slots"
      ;;
    show-vault)
      mode_flags=""
      ;;
    list-proposals)
      mode_flags="--state --queue --action-kind"
      ;;
    show-proposal|approve|revoke|cancel|execute|close)
      mode_flags="--proposal-id"
      ;;
    create-sol-withdraw)
      mode_flags="--receiver --lamports --proposal-offset-slots --not-before-slot --expires-at-slot"
      ;;
    create-spl-transfer)
      mode_flags="--spl-mint --spl-destination --spl-amount --spl-decimals --proposal-offset-slots --not-before-slot --expires-at-slot"
      ;;
    create-config-update)
      mode_flags="--threshold --proposal-ttl-slots --timelock-slots --proposal-offset-slots --not-before-slot --expires-at-slot"
      ;;
    create-replace-member)
      mode_flags="--replace-member-index --new-member-pda --new-member-alias --proposal-offset-slots --not-before-slot --expires-at-slot"
      ;;
    accept-pending-member)
      mode_flags=""
      ;;
  esac

  COMPREPLY=( $(compgen -W "$common_flags $mode_flags" -- "$cur") )
}

complete -F _pq_multisig_ops_complete ./scripts/run_pq_multisig_ops.sh
complete -F _pq_multisig_ops_complete pq-multisig-ops
