# PQ Multisig Localnet Rotation OK

Date: 2026-06-08

## Scope

This checkpoint now reflects the hardened rotation model for the
`pq_multisig_vault` localnet flow. It extends the earlier proposal set from:

- `2-of-3` SOL withdraw
- vault-owned SPL transfer
- self-governed config update

to also cover:

- pending-member rotation via `replace_member(index, new_member_pda)` followed
  by `accept_pending_member`
- post-rotation quorum proof where the new member creates the next proposal and
  the rotated-out member is rejected on approval

## Validation

Commands:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-harness/Cargo.toml
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh
./scripts/run_pq_multisig_localnet_harness.sh
```

The harness now self-funds the local fee payer via localhost airdrop if the
validator starts from a clean ledger with a zero-balance keypair.

## Passing Run

- SOL withdraw tx: `4W9m224JYcJLJHno3FgqBcnczaitGUqYmcnYMD56qANAmJyDQschWPjPwdovMZ3vgkDqSamapxbzAmy6JtF5R3HU`
- Cancel proposal tx: `5bw98njZ52c6tSNhzgFf8Ww4HL99CvhMogbCfWe9rnzc35SyDScvrzVumhsrFE824c4hV6TNMZmDQFD1Br4V2Ddf`
- Close cancelled proposal tx: `59drixrFdLnp3CQjXkT5jbUXDo8UX9b4ZHQo8nmBxoPZQUXvBNVFmPmd2F3LyeAKj4PGNEY9p9ymijsewPHZv9hN`
- SPL transfer tx: `RGFFgjKhs6XjQj9fCZxjVgT88XVoo5XSvdARa9sb1DCq9hNTtc52shdqKwt1MbEo7xiPjihaU2wCU59pGXQ5XpS`
- Config update tx: `5wXqR6MUh3N2q2Nn17ZaFAGG4cTMXoBxUMFmxUYpUymff5xxTGWxs2g4BzS3UUjtRB4aNQN6jGaXZ5jW5Cj7bJNq`
- Replace member tx: `5bEmmtKC1ZLZg9CxyV2xSV8oGZZYKybB9RYW6uHaz4Ha24aNgESbMRKU17MNuXygQx7exjMnmFmJ64BkicPg7Svh`
- Accept pending member tx: `2BPXXnYxdfuuzwCPN2Nq5f57kgePpy8eR4QCp31vLPFTiU4ZNJ1NWVkHhMqHUpMz2ksy8s7R996fbzWwTLJn9EpV`
- Post-rotation tx: `MXJihD7aUDjrH2gdJdZH76Tw3EGMcxMbnJhwFLK3fT9Hst1uFzJ9zbkDtvUMYau2325SFZgNHsghiJZyznHQUB6`

Observed state changes:

- vault SPL source: `1_000_000 -> 750_000`
- vault config: `threshold 2 -> 3`, `proposal_ttl_slots 512 -> 768`,
  `timelock_slots 0 -> 4`
- member index `2` entered pending replacement from
  `GT3XumxAZTenqJAbQYUMqFH25ehJPnQSx9DuQXgQTMdt` to
  `EK73hF3Hc3EUk4yyzcnAX7sfpWFVPxzjKM6JteJn8QiF`
- the incoming member explicitly accepted membership before the member set was
  rewritten
- rotated-out member approval attempt failed with
  `missing required signature for instruction`
- post-rotation withdraw succeeded with the new member in quorum:
  vault `2_727_200 -> 2_477_200`, recipient `1_000_000 -> 1_250_000`
