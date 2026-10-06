# PQ Multisig Ops Quickstart OK

Date: 2026-06-09

## Goal

Prove that the CLI-first async operator flow is easy to bootstrap for a dev:

1. start a fresh local validator
2. run the full `pq-multisig-ops` lifecycle
3. exercise proposal discovery from the CLI itself
4. prove alias-driven operator ergonomics
5. prove JSON reads and dashboard-handoff manifest support are available in the same CLI surface
6. tear the validator down automatically

## Command

```bash
./scripts/run_pq_multisig_ops_localnet_quickstart.sh --no-build
```

## Verified behavior

- local validator started automatically and reached RPC readiness
- the CLI generated 4 fixture-backed Falcon members
- member Vector PDAs were initialized
- vault init succeeded
- `list-proposals` returned an empty set before any proposals existed
- `list-proposals --json` returned machine-readable output before any proposals existed
- `list-proposals --queue needs-approval` discovered the first active proposal
- `list-proposals --queue needs-approval` worked through `--aliases-file`, `--vault-alias`, and `--member-alias`
- `show-proposal` decoded the proposal payload correctly
- `show-proposal` worked through `--vault-alias`
- `list-proposals --queue executable` surfaced a threshold-approved proposal
- `list-proposals --queue closable` surfaced cancelled / executed proposals
- the CLI alias file was generated and used end to end during the run
- the same CLI surface now exposes `--manifest-only` / `--manifest-file` for later dashboard handoff into the local signer path
- full async lifecycle completed:
  - SOL withdraw
  - revoke + cancel + close
  - SPL transfer
  - config update to `3-of-3`
  - replace-member
  - accept-pending-member
  - rotated-out rejection
  - post-rotation `3-of-3` SOL withdraw
- validator shut down automatically at the end

## Localnet txs

- SOL execute tx: `3o5q41SBhep7RLRzenSkBU34nvptrkjtzooD9CjvtTJ73yrBzi7h2ExRxWLsRwVsaGFERcjbTq6nQ7cFgkcEKM7Y`
- Close tx: `2WUTKWNtM1XaM1kx3vAQY4m9mnv82NsY9z9PARAA2L6ge5kyPvS3xXE2LEi2yyQ3CZE5Rq3R2kfnKRjjeTKjC4P7`
- SPL execute tx: `VDnMwFF5EsMPz6WJ7P97svu8jEqsVDMzssiDqXoeESqugtCCbKfzTTWxWWbXYBaYsGrRwQNcnyV7Q3DgBuFV7Bh`
- Config execute tx: `2gcUUFU5pjZa8zB24L5SV7Y1wS3GoKUTyLS2g8udJGTavQQ9XAA6SAJ4yzUd3pKcFFjE22Agd5AygqcLiNei1saN`
- Replace execute tx: `5db6LKeMiFiwPpSE3DZP2kCAbBcHXs4vGMnW3unmLHmtrcxSvC8E1g4rmgaJrzAHmLvKxPCcGUBgBwEcYLghkTRJ`
- Accept tx: `4PgP2tKMQrFVR6qoo3S5Acp1Q4GJ8scXVERJM5D3k8K6BpyKJmkQaqScAFfX8X99EA84JWTSuCb2MzVYoCXCRZxZ`
- Post-rotation tx: `28aSvVwJvMX8JahvCtcJtxkFYeuB3JLiZT5aFAivPagdzAYJxLHtUGVUo9nxcCGwD6WrRN72woqZZ8sKHFMjYTXQ`

## Final vault state

- threshold: `3`
- member count: `3`
- config version: `2`
- next proposal id: `6`
- timelock slots: `4`
- proposal TTL slots: `768`
- pending member: `none`
