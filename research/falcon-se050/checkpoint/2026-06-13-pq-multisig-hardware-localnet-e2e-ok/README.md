# PQ Multisig Hardware Localnet E2E OK

Date: 2026-06-13

## Summary

The first **real hardware-backed FalconOnly multisig member flow** is now
working end to end on localnet.

Topology:

- member 0: real serial hardware Falcon member
- member 1: fixture Falcon member
- member 2: fixture Falcon member
- threshold: `2-of-3`

The hardware member was used in both roles that matter for production:

1. hardware member created a proposal through Vector passthrough, a fixture
   member approved it, and the vault executed it
2. a fixture member created a proposal, the hardware member approved it
   through the same Falcon/Vector path, and the vault executed it

## Verified Run

Command:

```bash
./scripts/run_pq_multisig_ops_localnet_hardware_quickstart.sh \
  --port /dev/cu.usbmodem1101 \
  --no-build
```

## Final Vault

- Vault PDA: `6iJdUkjTJQR2mmNas5wnWQSZSdHnwgVBgHkmtYnpLr9A`
- Hardware member PDA: `Bfo45QwXEvnGzGjScYq6fri4315UyYJYNu3rQctanaov`
- Fixture member 1 PDA: `35YB99irdTUEKSkeRKTpDxrAiB6593TwtDYpjfDVssHN`
- Fixture member 2 PDA: `7hqRdrErqfNp5kWo1HMWHhmSZKvPTTZTjnSvJY87egLB`
- Threshold: `2`
- Member count: `3`
- Config version: `0`
- Next proposal id: `2`

## Proposal 0

Hardware-created SOL withdraw:

- Create tx: `5PcPfnQjRZKWBbJukc2gvvHGQrsQSmTE7mSchRiAVaQeWNW8EaEcziirfTmMFLRuocNoAAaBjkVzsDPfCvX8TmDe`
- Approve tx (fixture member 1): `rzRhtHxyUhpjcEnQLhL61h278R6Xcc1tc25joUSSh1K8GXR67CPwMg2Q5n1bCRNAF8SjcixL55JaKrQD1pXPd2M`
- Execute tx: `2vHWyUaGADDtDEkSpeuMbUfry42var88nvRiK1xuha9cRJ1DzdFautkPKj6ZqMME76vYnq3f81rkshS6uivedS6Q`

## Proposal 1

Fixture-created SOL withdraw with hardware approval:

- Create tx: `aQ967fvCunJA3MXNoxwvTsU7i3sSm7CN9zY9uwjFQGgik1kZQu6gUyy4PoBQhfuZmGiiwQBFihrxcxEWZ1eQ1MS`
- Approve tx (hardware member): `KtToL7xYiSgCGwCagxE4sja9XrGtu28ndHb82Hskji7RRF1ffpwwdRt4P8imZhaQ1rTVD3pDVbKNf2SVddf1zYX`
- Execute tx: `5ysujMLkF3QYZDgCqCjqsy1qUatfZathtbDq7s1im5iSot3ZHscwsVobVxa5SkNXjFZyuB45wgHmsxy1emmzRJnY`

## What This Proves

- the real hardware wallet can act as a FalconOnly vault member
- hardware-backed Falcon approvals survive the full
  `Vector advance + passthrough(multisig::...)` path
- async operator UX works with a real serial device member, not only fixtures
- the remaining work before mainnet is deployment/canaries and ops polish, not
  the core hardware-member protocol path
