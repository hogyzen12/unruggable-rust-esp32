# PQ Multisig Localnet Full Flow OK

Date: 2026-06-08

## Summary

This is the earlier three-proposal milestone. It predates the hardened
cancel/close lifecycle and acceptance-based member rotation that now live in
`checkpoint/2026-06-08-pq-multisig-localnet-hardened-ok/`.

The localnet `pq_multisig_vault` harness now exercises three `2-of-3`
proposal kinds end to end through the Falcon/Vector member path:

- vault-owned `SOL withdraw`
- vault-owned `SPL transfer`
- self-governed `config update`

The key blocker after the first SOL canary was transaction size. The original
`SPL transfer` and `config update` proposal payloads were too large once they
were wrapped inside the member Vector `advance + passthrough(...)` flow. The
compact action encoding now fits inside that envelope:

- `SPL transfer` binds `mint + destination + amount + decimals`, while the
  hardened execution path now resolves the vault's canonical ATA for that mint
  under the standard SPL Token program
- `config update` is now the compact policy-update path for
  `threshold + proposal_ttl_slots + timelock_slots`

## Successful Run

- Vault seed: `localnet-1780908952`
- Vault PDA: `CKdQAhC4MuGZTkP7ZsYg656BvVhVsqhzNvdiZeMcaYdK`
- Recipient: `7L2vSdxHEGwJfjYXGWehgffdtZQ4v331c5jAY9baJ7es`

### SOL Withdraw

- Proposal ID: `0`
- Action hash: `dca6fc3da2c647534da2ebe946879c21e26d784f79c54d84deca9a8c441ef262`
- Withdraw amount: `1_000_000` lamports
- Vault balance: `3_727_200 -> 2_727_200`
- Recipient balance: `1_000_000 -> 2_000_000`
- Execute tx: `2zXZW8ibETXKKsaWSwaWDkaKMKj5f3mpGmdfbYbw22AeEoapVaJvsw1iSKtbcs4bqR4V5PNe88oCUNUEUx7Cpp7A`

### SPL Transfer

- Proposal ID: `1`
- Action hash: `4b75b96fde40ff18cbb8b6ffccaea13a473830a9ebcc0506f28e0af31f1b384b`
- Mint: `riymvt3L8WsHiKhhGWWS74poDacGuNmtSMy5z4QURf7`
- Vault source ATA: `GWLo32dwycNtJ3UpUPSS5GiCFii1xfuTYqzoHQDukmmj`
- Recipient ATA: `CMg4MwuKSCij7PrgfeWkN8czw8QpuaMFjUFmBB7hgX2D`
- Transfer amount: `250_000` raw units
- Source amount: `1_000_000 -> 750_000`
- Destination amount: `0 -> 250_000`
- Execute tx: `y1Rr2DvZ262nC3Z6PQ6bZQPwJ97VNfN2pDT2TrsJirjz9ieYmkYxSNseLqmPbnhuFzgfAEZhhdwovNLrzW7Ehcx`

### Config Update

- Proposal ID: `2`
- Action hash: `7aa92128efcf41fbd1fb7c0db8666ff82f674fa1a72206c0dfba65e832448048`
- Config version: `0 -> 1`
- Threshold: `2 -> 3`
- Proposal TTL: `512 -> 768`
- Timelock slots: `0 -> 4`
- Execute tx: `2yp8m9Wxzs4GH7Q4ftrfDtoqUT2RNMciRFpspJgixB2WTseHSVTBPSeD49PbqudhrMUKHSsCTucGSFkjkFsfMmEZ`

## Commands

```bash
./scripts/build_pq_multisig_vault_program.sh
./scripts/start_pq_multisig_local_validator.sh --build --mint-keypair ~/.config/solana/id.json
./scripts/run_pq_multisig_localnet_harness.sh
```

## Notes

- The harness now validates all three proposal kinds on one localnet run.
- The member approval path is still the same Falcon/Vector flow used for the
  earlier SOL-withdraw canary.
- Member-set rotation is not exercised in this checkpoint; see the hardened
  follow-up checkpoint for the pending-member acceptance model.
