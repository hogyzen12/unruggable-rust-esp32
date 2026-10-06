# 2026-06-07 Vector Mainnet Canary Success

This checkpoint records the first successful mainnet deployment and canary
transactions for the Falcon Vector program using the ESP32 hardware signer.

## Deployment

- Program ID: `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`
- Upgrade authority: `FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC`
- Deploy signature: `43BVbyh71mK21jRck6Rt3HQoftSfgq9sMUoQFSYCZU4YnJ1ThieceF86KxsgZJmkfy8XpjBmr1Z1K3ZKTpTAMQr2`
- ProgramData address: `BhtFJPvY9nEnTBYaomdGv2HYsMDkYjivf33ueS4PwZN9`

## Mainnet canaries

### `full-empty`

- Initialize tx: `3V2hEjrPrVCfSoqQ1gUWLy5muqVHwMdoA7ZBvpmoP5PUWDnK45dUFBiao5B9uEUm9GSruS7B7sKdzRxsx9AWUaAj`
- Advance tx: `65MuKQviB3meX9dYNGzpZBGZY5ngH8HNnqauqSqnnawiTRXxJhS9hJBrfi7kWVHc3oUfhNg4tP7M4xs7PoTbhuoq`
- Result: Vector PDA initialized and nonce advanced on mainnet

### `full-withdraw`

- Top-up tx: `5QdqmWMyaCcaQNw3sEpeEUAf1dDw4XNFRcxo5vKwFWFwdxHh53Q4jiSvSoxBwQeAtMfZk2HWNe9J1zL9xKwdrZuS`
- Withdraw tx: `5pxAM6j9ivAJG4XiuDNMNCiWDaUj8ButzwwXHxo8Zhzno1e42XuLTTvzqkgsM4R4yP5RPEkthQBxfepZQJqkesbM`
- Withdraw digest: `90079279cbb82614946b83301dfb8d6e04a072fd6a60a9bf5e8ca40ad9353c1c`
- Result: `advance + passthrough(withdraw)` succeeded on mainnet and the Vector PDA returned to its rent floor

## Notes

- The ESP32 produced the Falcon digest signatures on hardware.
- The outer Solana fee payer remained a local keypair; the SE050 was not used
  for the outer transaction signature.
- The current mainnet-ready path is therefore:
  `local fee payer + ESP32 Falcon Vector authorization`.
