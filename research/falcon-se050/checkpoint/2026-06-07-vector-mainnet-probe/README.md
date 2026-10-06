## Vector mainnet probe

Date: 2026-06-07

Inputs used:
- Device port: `/dev/cu.usbmodem1101`
- RPC: `https://johna-k3cr1v-fast-mainnet.helius-rpc.com`
- Fee payer: `/Users/hogyzen12/.config/solana/worKFoQQH5KzuBnmS3jKKYsJuUi5toCoEp7n4mwRtwa.json`
- Commitment: `confirmed`

Observed results:
- Live status mode worked.
- Device binding and ESP32 Falcon digest signing worked.
- The derived Falcon Vector PDA for the current device was absent on-chain.
- The first real `initialize -> advance` submit attempt failed at transaction simulation with:
  - `Attempt to load a program that does not exist`

Conclusion:
- The host harness is ready.
- The ESP32 Falcon signing path is ready.
- The target mainnet cluster does not currently have the Falcon Vector program ID from this repo deployed.
- Until that program is deployed on the target cluster, the harness cannot complete a real on-chain Vector/Falcon transaction there.
