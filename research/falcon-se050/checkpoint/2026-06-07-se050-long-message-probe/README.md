## SE050 long-message probe

Date: 2026-06-07

Goal:
- Remove the blocker for `SE050 outer tx signature + Falcon Vector authorization`
- Specifically, test whether the active SE050 Ed25519 path can be made to sign Vector-sized Solana messages by switching back toward the archived "big sign" transport settings.

Change tested:
- Removed the `set_ifs(i2c, 128)` call from `src/se050.rs`
- Increased chained T=1 transport chunk size from `128` to `254`

Observed result on real hardware (`/dev/cu.usbmodem1101`):
- `PQC_*` commands still worked.
- Plain `SIGN` on a ~1000-byte payload still failed with:
  - `ERROR:Sign failed: READ_ERR:ESP_FAIL (error code -1)`
- `run_vector_onchain_harness.sh --mode probe-device-payer` still failed on both dummy Vector `initialize` and dummy Vector `advance` outer messages with the same SE050 signing error.
- The modified build also emitted a backtrace immediately after `SET_MODE:PIN`, so it introduced a regression unrelated to the actual Vector blocker.

Conclusion:
- The simple "254-byte chunks + no IFS override" transport change is not sufficient on this hardware/firmware.
- Keep the repo on the prior transport behavior and treat true device-payer Vector support as still blocked on deeper SE050 long-message signing work.
- The immediate practical path remains:
  - local funded fee payer for the outer Solana transaction
  - ESP32 Falcon signing for the Vector digest
