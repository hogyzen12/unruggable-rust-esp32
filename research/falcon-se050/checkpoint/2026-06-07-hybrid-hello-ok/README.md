Checkpoint: `2026-06-07-hybrid-hello-ok`

State captured:
- SE050 Ed25519 signing works on-device.
- ESP32 FN-DSA/Falcon-512 signing works on-device.
- Host harness verifies `SIGN`, `PQC_INIT`, binding, and `SIGN_HYBRID` for payload `aGVsbG8=` (`hello`).

Hardware test command:

```bash
./scripts/run_hybrid_harness.sh \
  --port /dev/cu.usbmodem1101 \
  --pin <PIN> \
  --sample-tx-b64 aGVsbG8= \
  --no-final-wipe
```

Notes:
- This snapshot includes the worker-thread FN-DSA implementation.
- It is intended as a fallback checkpoint before exploring Solana transaction integration.
