# Falcon / SE050 research branch

This branch preserves the recovered experimental firmware and test work in
[research/falcon-se050/](research/falcon-se050/README.md): SE050-wrapped software
Falcon signing, Vector integration, FalconOnly multisig, host/hardware harnesses,
operator tools and June checkpoint notes.

**The working dual-secure-element firmware is on
[main](https://github.com/hogyzen12/unruggable-rust-esp32/tree/main).** This research
snapshot is separate from the qualified V2 firmware and preorder releases.

```sh
python3 research/falcon-se050/verify-snapshot.py
python3 firmware/verify.py
```

The original local working tree, private keys, raw device records and full
checkpoint payloads remain in place. This branch contains 244 selected source,
configuration, dependency and documentation files with recorded hashes. Its
[provenance](research/falcon-se050/source-provenance.json) identifies the original
working-tree base and the nested upstream repositories.

See the [research README](research/falcon-se050/README.md) before resuming old
hardware or on-chain workflows. Historical test notes are preserved; no new
hardware or on-chain qualification is claimed by this recovery.
