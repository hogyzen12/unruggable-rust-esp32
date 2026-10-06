# PQ Multisig Localnet 2-of-3 OK

Date: 2026-06-08

## Summary

The first localnet `pq_multisig_vault` canary is working end to end.

The run:

- provisioned 3 synthetic Falcon/Vector member PDAs
- initialized a new `2-of-3` vault PDA
- funded the vault with rent + withdraw amount + buffer
- created a SOL-withdraw proposal from member 0 through
  `vector advance + passthrough(multisig::create_proposal_from_member)`
- approved the same proposal from member 1 through
  `vector advance + passthrough(multisig::approve_proposal_from_member)`
- executed the proposal and moved lamports from the vault PDA to the recipient

## Successful Run

- Vault seed: `localnet-1780905912`
- Vault PDA: `EmZ9X6ZWJnVXExZkqaouNuz56D1pEys4PzZrx4cwvKab`
- Recipient: `rZeHD7BaJJU6oMKqt9p65VHZzkFpAsg134bHDR76fPt`
- Withdraw amount: `1_000_000` lamports

Transactions:

- `Create proposal tx`: `89BmxuYZfDVBsnRoBhZEd7y1V8neZ4YkawFzNfLdrw9n2ypkvSdf9hj4nXMQKLZf2ocwLzZb33MD98GsKeLqvFK`
- `Approve proposal tx`: `2zbe69fp5pEhNrhaRKCbdnDmYZcYbxy5NVa1wbiTNwYSr9AYPyngHYYsGFBjDid6Dskm7gVipyeAvFVqaW5ynG1c`
- `Execute tx`: `3mMMmguz2nZCciLPTQpr28jnCN2MPxj4ki91uLzD7eH9g73jDVK3Kc7no8KFc7HugPVNj3RadJSwKu6HM6SfBzmx`

Proposal / balances:

- Proposal ID: `0`
- Action hash: `f93010affa8c93f72ebdddc5a104c63b9f151db30e0a7180d4692663c3bf1e94`
- Vault balance: `3_727_200 -> 2_727_200`
- Recipient balance: `1_000_000 -> 2_000_000`
- Executed slot: `857`

## Commands

```bash
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json
./scripts/run_pq_multisig_localnet_harness.sh
```

## Notes

- The harness now uses a unique default vault seed per run, so repeated local
  runs do not collide with an existing vault PDA on the same validator ledger.
- The Vector digest path now mirrors the working mainnet harness rules:
  passthrough sub-instruction signer flags are cleared before hashing, and the
  digest uses promoted message-level signer/writable flags plus the current
  instruction footer.
- The vault program now also has explicit `SPL transfer` and `config update`
  proposal kinds, but this checkpoint only exercises the `SOL withdraw` path.
