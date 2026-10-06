# PQ Multisig Ops Localnet Async Flow OK

Date: 2026-06-08

## Scope

This checkpoint captures the proposal-style async operator flow for
`pq_multisig_vault` after hardening the lifecycle and switching member rotation
to an acceptance-based model.

Validated in this pass:

- async `create -> approve -> revoke -> cancel -> close -> execute`
- vault-owned `SOL withdraw`
- vault-owned `SPL transfer` bound to the canonical vault ATA for the mint
- self-governed `config update`
- `replace_member(index, new_member_pda) -> accept_pending_member`
- rotated-out rejection on the next approval attempt
- post-rotation `3-of-3` quorum proof with the incoming member originating the
  next proposal

## Validation

Commands:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-ops/Cargo.toml
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json
./scripts/run_pq_multisig_ops_localnet_e2e.sh
```

The wrapper self-funds the local fee payer via localhost airdrop, provisions
four synthetic Falcon members, initializes their Vector PDAs plus one
`pq_multisig_vault`, and then drives the async operator lifecycle through the
same member Vector path used by the production design.

## Passing Run

- Vault PDA: `8CscSjFMhG4kNgyNR5J3nNMzDQLD5TkuzojYqcjsb16Q`
- SOL execute tx: `4fQYrgr5Kjj1PAxYPSme1MR5czvrYk8dMqYqsKd3ENQPcopEXaBuueyg6FU8jekwupxYoZwKuVoWVPwuzVQkfK5N`
- Revoke tx: `5qtzikbpdyjU53icgrLzDevJJQX2oU1pRE8CaiHXzLf1S4Rmv7PGQK3SVkwr4hVBjhBXYswxepjN2cKSSpEcyvFQ`
- Cancel tx: `4XZWpsccsyuVpdiwdaQUAHVQeHWpP4xo3cARSxdFSUV5Z7oQ7ccjnREmaB9FXWuK1HL1vCMUoUzKeuhtFYKRjfaj`
- Close tx: `4ZCjbgMRqErNByVXGQRiSS3JasAJmvqPTgnKTr7vxrJqcS8i5tieHbzYLcgbtBoQdxgc5jmUmWhP7VqaF7B5FrgZ`
- SPL execute tx: `5RtBSmVQo5EWNWBZeRBJyJQLrjoDsiZbQWodUiNrNKsjd1HLnQaurm7dbiqoTyTgemYe16BuN6TfCj7dSrGdXZLf`
- Config execute tx: `4mQCzV495JWkTHHFJ2f8Cyd9HjMX96gwF49N587DfuCvw8LzLUXzNhKMcngJ5BKzAEPGPAA4xyfgUQgxSycFTqcA`
- Replace execute tx: `614EpsnqQCvQdnzF1gPRTu8Txazz9L7uMkWhADrqxCeKPfouJ64gceni2tzCxzUBDqnH4skfijfD5nLEkMyGNAY7`
- Accept tx: `3AVLocSuQbmNMDcLG7EoSjvePFpUJ6CvT9tUgnj8qW5sGPcSpq1JF5KMfSPd2vJV9f2LV4oVMZn4CQMUp8QjRGNi`
- Post-rotation tx: `4UELgz8LMv2kyWiYqsWC66B2oduiaQrR4LKXZGJgr9cr8iZ4yzh6q4T9NvCUe9rKAAjS7ucMNrvQn7WBhSSnmssM`

## Observed State Changes

- the cancelled proposal remained `Active` after `revoke`, moved to
  `Cancelled` after `cancel`, then both proposal accounts were closed
- vault SPL source ATA: `1_000_000 -> 750_000`
- recipient SPL destination ATA: `0 -> 250_000`
- vault config: `threshold 2 -> 3`, `proposal_ttl_slots 512 -> 768`,
  `timelock_slots 0 -> 4`
- member index `2` rotated from
  `FPebhekAjWvF8oLN95veEqAqc1y5cJyvGTF8FYVnCXtk` to
  `95civoqMfXUQbwbodTvePJgaTsNgS9ij71natC84kHTT`
- `accept_pending_member` advanced the vault `config_version` from `1 -> 2`
  and cleared the pending slot
- the rotated-out member's later approval attempt failed as expected
- the rotated-in member originated the next successful `3-of-3` proposal
