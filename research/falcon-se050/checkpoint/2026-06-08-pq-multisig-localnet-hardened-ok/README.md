# PQ Multisig Localnet Hardened Flow OK

Date: 2026-06-08

## Scope

This checkpoint captures the hardened localnet `pq_multisig_vault` flow after
adding the async lifecycle and safer member rotation model:

- async `create -> approve -> cancel -> close -> execute`
- vault-owned `SOL withdraw`
- vault-owned `SPL transfer` bound to the vault's canonical ATA for the mint
- self-governed `config update`
- pending-member rotation via `replace_member -> accept_pending_member`
- post-rotation `3-of-3` quorum proof where the rotated-out member is rejected

## Validation

Commands:

```bash
cargo check --target aarch64-apple-darwin --manifest-path pq-multisig-vault/Cargo.toml
cargo check --target aarch64-apple-darwin --manifest-path tools/pq-multisig-harness/Cargo.toml
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json
./scripts/run_pq_multisig_localnet_harness.sh
```

The harness self-funds the local fee payer via localhost airdrop when the
validator starts from a clean ledger.

## Passing Run

- SOL withdraw tx: `4E9SjGx7DPuUnhVBVJnbCSPkmFjFuhJAprDEiZaEhYXoJFs9dMySEi7FC7RtCyQLVmJLZrm7yiMrzAwXYoTMbdvd`
- Cancel proposal tx: `2N3fuG58a1NpfRm3ykUAZMcQZQjNcRkvUKFMK7pQ7UsADi4ByPW2xrPbZV3XuRHEo7NCrx1kpnoGsNMLyqsBBVWJ`
- Close cancelled proposal tx: `36Una3Wz25UfUMhVJLU2MdATfmqg7KMEpfbrF1PJ8CRqJBxvp5wg35gNhTDfAyxsRry4BWjv7Qbg9x4yRQnTH3Mv`
- SPL transfer tx: `3E9wnVDkVYktTeD7cEEw8VcyBAESyKgoBVnpPV4zQFFAxzwmyRe1xjubCv2Dx3qvKyXbMD3UEsjsMaVWnpu46XPQ`
- Config update tx: `4nuKqvM2U8eSRE93oWKDnBNVqS76iUcCyKrwyUiuMLdXAhPQoC12eNNYt9Tj1m7njXznHrH5Zc8s5SAFZ6RNnNGR`
- Replace member tx: `2wUBE1GfbwZYTuCyeDRBhLwqHuuZmd9BLVj3S4GAhkQMFKqm9hjsVSxXf3pbThgfrCUqbKWQWuNFXzgxrV8uFhAE`
- Accept pending member tx: `1PArXzgNouUWsZrVc5wfi6K2eSWC686cde9otRUQdPUA7U812W3moiHCYQk9bRZtiKk3VGkEqWuHksejN9TvP8r`
- Post-rotation tx: `2MvEtjufixiH6kwfgNCFNvXUNQbN5bLVmH2WZYmu71TrjDVeXEXC9yQLoycsbbFLntZ4Gx82a5B91bkN5Qn6DLwM`

## Observed State Changes

- cancelled proposal moved to `Cancelled`, then both proposal accounts were
  closed and rent was refunded
- vault SPL source ATA: `1_000_000 -> 750_000`
- recipient SPL destination ATA: `0 -> 250_000`
- vault config: `threshold 2 -> 3`, `proposal_ttl_slots 512 -> 768`,
  `timelock_slots 0 -> 4`
- member index `2` rotated from
  `6b12swavN2nTGfXxBiSGVqaDsFkDJwEc1sd17jytGhnc` to
  `CQjL5xcxCg1FH8BKMYUzjTVaPcSj99ooGNBKsfp3gxQ2`
- the rotated-out member's later approval attempt failed with
  `missing required signature for instruction`
- the rotated-in member originated the next successful `3-of-3` proposal
