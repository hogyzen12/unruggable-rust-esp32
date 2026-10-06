# PQ Multisig Vault

Solana workspace for the next primitive above the existing Falcon/Vector
policy wallet.

## Purpose

This workspace is the starting point for a **2-of-3 Falcon-gated multisig
vault**:

- each member is an existing Falcon-gated Vector PDA
- the vault program counts approvals from those member PDAs
- once threshold is met, the vault PDA executes the stored proposal payload
- FalconOnly v1 is enforced on-chain at init, approval, cancel/revoke, and
  pending-member acceptance; hybrid/classical member sets are intentionally
  rejected

## Workspace Layout

- `crates/core/`
  Host-side helpers for:
  - PDA derivation
  - proposal payload serialization
  - action-hash derivation
  - instruction builders
- `program/`
  On-chain vault program scaffold

## Current Scope

Implemented now:

- `initialize_vault`
- `create_proposal_from_member`
- `approve_proposal_from_member`
- `cancel_proposal`
- `close_proposal`
- `execute_proposal`
- `accept_pending_member`
- `SOL withdraw` proposal execution
- `SPL transfer` proposal execution
- `config update` proposal execution
- `replace_member` proposal execution

Current limits:

- the localnet harness now exercises `SOL withdraw`, cancel + close lifecycle,
  vault-owned `SPL transfer`, self-governed `config update`, and pending-member
  rotation end to end
- vault membership is intentionally **Falcon-only** in v1: every initial member
  and accepted replacement member must be a live Vector/Falcon account owned by
  `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`
- zero or duplicate member addresses are rejected during initialization
- `config update` is currently the compact policy-update path for
  `threshold + proposal_ttl_slots + timelock_slots`
- member rotation is currently a two-step
  `replace_member(index, new_member_pda) -> accept_pending_member` path; full
  member-set rewrites are still intentionally out of scope
- `SPL transfer` is bound to the vault's canonical ATA for the mint under the
  standard SPL Token program so the executor cannot choose among multiple
  same-mint source accounts
- `close_proposal` always refunds proposal rent back into the vault PDA, not
  to an arbitrary caller-selected receiver
- generic CPI bundle proposals are intentionally **reserved/disabled** in
  FalconOnly v1 until they can be reviewed and presented safely in the
  operator surface
- no frontend / Squads-like UX exists yet

## Async Operator CLI

The repo now also includes an operator-style host CLI under
`tools/pq-multisig-ops/` plus wrappers:

```bash
./scripts/run_pq_multisig_ops.sh --mode <subcommand> ...
./scripts/run_pq_multisig_ops_localnet_e2e.sh
./scripts/run_pq_multisig_ops_localnet_quickstart.sh
./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh --port /dev/cu.usbmodemXXXX
```

That surface exposes the hardened multisig lifecycle the way a real async vault
operator would use it:

- `initialize-vector`
- `init-vault`
- `show-vault`
- `list-proposals`
- `show-proposal`
- `create-sol-withdraw`
- `create-spl-transfer`
- `create-config-update`
- `create-replace-member`
- `approve`
- `revoke`
- `cancel`
- `close`
- `execute`
- `accept-pending-member`

The CLI supports both fixture-backed Falcon members and serial-device-backed
members, so the async proposal flow is no longer tied to the monolithic localnet
harness.

The quickest dev bootstrap is now:

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh
```

That starts the PQ local validator, runs the CLI-first async multisig E2E, and
shuts the validator down automatically. Use it as the first “does this repo work
on my machine?” onboarding command.

The repo now also has a real hardware-member proof:

```bash
./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh \
  --port /dev/cu.usbmodem1101
```

That flow keeps the vault FalconOnly while replacing one fixture member with a
real serial device member. It proves:

- hardware member can create a proposal through Vector passthrough
- fixture member can approve it and the vault can execute it
- fixture member can create a second proposal
- hardware member can approve it through the same Falcon/Vector path
- the vault can execute the second proposal

For real operator use, the first command is now usually:

```bash
./scripts/run_pq_multisig_ops.sh \
  --mode list-proposals \
  --vault-seed pq-mainnet-demo \
  --state active \
  --queue needs-approval \
  --member <member_vector_pda>
```

That scans the vault's proposal PDAs directly from chain, derives the effective
proposal queue (`needs-approval`, `executable`, `closable`, `stale-config`,
etc.), and lets an operator move from discovery to `show-proposal`, `approve`,
`execute`, or `close` without already knowing the proposal id.

For a more usable operator setup, the CLI also supports:

- `--json` on read flows such as `show-vault`, `list-proposals`, and `show-proposal`
- `--manifest-only` / `--manifest-file` so a dashboard can hand off to the local signer CLI
- `--aliases-file`, `--vault-alias`, `--member-alias`, and `--new-member-alias`

Start from:

`[docs/pq-multisig-ops.aliases.example.json](/Users/hogyzen12/coding-project-folders/unruggable-rust-esp32/esp32-solana-signer/docs/pq-multisig-ops.aliases.example.json)`

The operator UX now also includes:

- checked-in bash/zsh completions via `scripts/completions/`
- `./scripts/install_pq_multisig_ops_completions.sh`
- a thin local dashboard via `./scripts/run_pq_multisig_dashboard.sh`

The dashboard stays intentionally narrow: it reads vault/proposal state from the
same CLI `--json` surface and prepares action manifests through
`--manifest-only`, but leaves actual Falcon signing and transaction submission in
the local operator CLI path.

## Localnet Harness

The repo now includes a dedicated localnet harness under
`tools/pq-multisig-harness/` plus validator wrappers:

```bash
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json
./scripts/run_pq_multisig_localnet_harness.sh
```

That flow provisions 3 synthetic Falcon/Vector member PDAs plus one replacement
candidate, initializes a `2-of-3` vault PDA, then runs the hardened async
lifecycle end to end:

- a vault-owned `SOL withdraw`
- a cancelled-and-closed proposal
- a vault-owned `SPL transfer`
- a self-governed `config update`
- a pending-member `replace_member` plus `accept_pending_member`

It then proves the rotated-in member can create the next `3-of-3` proposal
after the config update, while the rotated-out member is rejected on approval.
Before the happy-path vault init, the operator E2E also proves FalconOnly v1 by
attempting a hybrid/classical init and confirming it is rejected.

## Validation

Host-target compile check:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-ops/Cargo.toml
```

Working localnet e2e:

- SOL withdraw tx: `4E9SjGx7DPuUnhVBVJnbCSPkmFjFuhJAprDEiZaEhYXoJFs9dMySEi7FC7RtCyQLVmJLZrm7yiMrzAwXYoTMbdvd`
- Cancel proposal tx: `2N3fuG58a1NpfRm3ykUAZMcQZQjNcRkvUKFMK7pQ7UsADi4ByPW2xrPbZV3XuRHEo7NCrx1kpnoGsNMLyqsBBVWJ`
- Close cancelled proposal tx: `36Una3Wz25UfUMhVJLU2MdATfmqg7KMEpfbrF1PJ8CRqJBxvp5wg35gNhTDfAyxsRry4BWjv7Qbg9x4yRQnTH3Mv`
- SPL transfer tx: `3E9wnVDkVYktTeD7cEEw8VcyBAESyKgoBVnpPV4zQFFAxzwmyRe1xjubCv2Dx3qvKyXbMD3UEsjsMaVWnpu46XPQ`
- Config update tx: `4nuKqvM2U8eSRE93oWKDnBNVqS76iUcCyKrwyUiuMLdXAhPQoC12eNNYt9Tj1m7njXznHrH5Zc8s5SAFZ6RNnNGR`
- Replace member tx: `2wUBE1GfbwZYTuCyeDRBhLwqHuuZmd9BLVj3S4GAhkQMFKqm9hjsVSxXf3pbThgfrCUqbKWQWuNFXzgxrV8uFhAE`
- Accept pending member tx: `1PArXzgNouUWsZrVc5wfi6K2eSWC686cde9otRUQdPUA7U812W3moiHCYQk9bRZtiKk3VGkEqWuHksejN9TvP8r`
- Post-rotation tx: `2MvEtjufixiH6kwfgNCFNvXUNQbN5bLVmH2WZYmu71TrjDVeXEXC9yQLoycsbbFLntZ4Gx82a5B91bkN5Qn6DLwM`

Working async-ops localnet e2e:

- SOL execute tx: `4fQYrgr5Kjj1PAxYPSme1MR5czvrYk8dMqYqsKd3ENQPcopEXaBuueyg6FU8jekwupxYoZwKuVoWVPwuzVQkfK5N`
- Revoke tx: `5qtzikbpdyjU53icgrLzDevJJQX2oU1pRE8CaiHXzLf1S4Rmv7PGQK3SVkwr4hVBjhBXYswxepjN2cKSSpEcyvFQ`
- Cancel tx: `4XZWpsccsyuVpdiwdaQUAHVQeHWpP4xo3cARSxdFSUV5Z7oQ7ccjnREmaB9FXWuK1HL1vCMUoUzKeuhtFYKRjfaj`
- Close tx: `4ZCjbgMRqErNByVXGQRiSS3JasAJmvqPTgnKTr7vxrJqcS8i5tieHbzYLcgbtBoQdxgc5jmUmWhP7VqaF7B5FrgZ`
- SPL execute tx: `5RtBSmVQo5EWNWBZeRBJyJQLrjoDsiZbQWodUiNrNKsjd1HLnQaurm7dbiqoTyTgemYe16BuN6TfCj7dSrGdXZLf`
- Config execute tx: `4mQCzV495JWkTHHFJ2f8Cyd9HjMX96gwF49N587DfuCvw8LzLUXzNhKMcngJ5BKzAEPGPAA4xyfgUQgxSycFTqcA`
- Replace member tx: `614EpsnqQCvQdnzF1gPRTu8Txazz9L7uMkWhADrqxCeKPfouJ64gceni2tzCxzUBDqnH4skfijfD5nLEkMyGNAY7`
- Accept pending member tx: `3AVLocSuQbmNMDcLG7EoSjvePFpUJ6CvT9tUgnj8qW5sGPcSpq1JF5KMfSPd2vJV9f2LV4oVMZn4CQMUp8QjRGNi`
- Post-rotation tx: `4UELgz8LMv2kyWiYqsWC66B2oduiaQrR4LKXZGJgr9cr8iZ4yzh6q4T9NvCUe9rKAAjS7ucMNrvQn7WBhSSnmssM`
