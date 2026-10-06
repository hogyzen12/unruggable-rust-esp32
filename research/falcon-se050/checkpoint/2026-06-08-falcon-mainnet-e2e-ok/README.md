# 2026-06-08 Falcon Mainnet E2E Success

This checkpoint records a full mainnet pass of the three core Falcon-authorized
hardware-wallet flows:

- send SOL
- delegate stake
- swap USDC into native SOL

All three used the ESP32 Falcon signer for the Vector `advance` digest on the
deployed mainnet program:

- Program ID: `FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU`
- Device wallet: `f2T3PEs5GXL8syLNhjFTiYmvnHswDtQ5X5mYPoBAsWz`
- Vector PDA: `AMGjaq9tsTMBn6D5uTGn2nhPMBXXb2R67PasgjfdbG5r`
- Relayer / fee payer: `FaLCnrZRrdD7xAWcFNNMTH3SrwMn5X1ooAiKLbGgqDaC`
- RPC: `https://johna-k3cr1v-fast-mainnet.helius-rpc.com`

## Send

- Receiver: `muMuRjyK84ASfKpDTi23es3XZ5vtHScFnN5vvi1K8hy`
- Top-up tx: `61kfFuR7YeVry5cpGHk4KK3FcwqqxdNh3rPc1ZPJYh6fRaS9Mg4apDZgWuVvtVbd9dBS4x7s74nPG6R5X5E2J9mV`
- Withdraw tx: `3uJ5Zqen6Ni8K1f7pyRbTHmjaxxQcBWWKG52ozeifD9oxuZY7FJMWA7UVmefg1LGvecHHSzcsiM9FqhKQDkhdTNY`
- Advance digest: `b2926344c0758263bc853e59b39de5736e459af32362676f1079bef2c1f2eaf9`
- Amount: `0.001420000 SOL`

## Stake

- Vote account: `unRgBLTLNXdBmenHXNPAg3AMn3KWcV3Mk4eoZBmTrdk`
- Stake account: `8siVMouocX8kpnR7872bKYTcZADrjUaYxBf5KCuvDp44`
- Create/init tx: `29Ki6BxCiYs17zxAkShxbHztRrdifJseisSBhDuz3M64nEW7cZDDxHMZkuTL6VwbhukFyWcdcS6dR3C69LB8u6DK`
- Top-up tx: `3GAiyPxjhE8uXC2RUrQvDsyKEhWAC7b8kCr4YtLiz2AaWnqUFLmJhB3eKQRjDPLJxrQhhL6qiDQiaxQPR3RLyQZT`
- Delegate tx: `2r4zrT4E8hNyh5HcrDVia1UrYZ7stiWiZNRe4HkswpKy5mekHJCPVKSKTrM6g7bnt61FPZakp6LRuDekAsJsS9dS`
- Advance digest: `9a241f4bb5fea4e1e92b163120f64c43be1400fc022aae16308a7268dd90d8d2`
- Delegated stake: `0.007717120 SOL`
- Rent reserve: `0.002282880 SOL`
- Total stake account funding: `0.010000000 SOL`

## Swap

The first wrapper attempt used a `1.0 USDC` swap canary and failed before the
Falcon leg because the hardware wallet only held `0.1858 USDC` at that point.
The final successful canary therefore used `0.1 USDC`.

- Bootstrap tx: `4UiwxqvAoCk6aXMCmwBc2eY9rC8Frgf3bE9HXUcBuGySv2fzXfzSmwo1Er5F9cFxQDZyJFEGaFyt8tMaBFqYFSuV`
- Swap tx: `3hNYQNrL2TGfxHnAhygdLQ5Kis1uoB9MR59VjyQcokqzemWmcdxbTKVhPx2Azu9qTxWoUCU1NnhRXswCNGRQYNaE`
- Advance digest: `575bfe0959ad86526e46e28209ba97ebc0dfd374545f9335cd798704d4c539c0`
- Input: `0.100000 USDC`
- Output: `0.001519443 SOL`
- Recovered WSOL rent: `0.002039280 SOL`

## Wrapper

The repeatable wrapper is:

`./scripts/run_mainnet_falcon_e2e.sh --withdraw-receiver <pubkey> --stake-vote-account <vote_account>`

Current conservative defaults:

- withdraw: `1420000` lamports
- stake: `10000000` lamports
- swap: `100000` raw USDC with `1300000` raw SOL minimum output
