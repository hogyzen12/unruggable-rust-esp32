# Historical development archive

The active firmware is [Unruggable V2](../README.md). The directories below
preserve the older public development material, moved without changing their
code. The previous paths remain available in Git history and on the retained
`master` branch at `48c2189`.

| Directory | Preserved material |
| --- | --- |
| [legacy-development/esp32-solana-signer/](legacy-development/esp32-solana-signer/) | Older ESP32 development crate; predates the recovered preorder firmware |
| [legacy-development/twofa/](legacy-development/twofa/) | Earlier authentication experiments |
| [legacy-development/solana-transaction-builder/](legacy-development/solana-transaction-builder/) | Transaction-building helpers and examples |
| [legacy-development/squads-quickstart-ts/](legacy-development/squads-quickstart-ts/) | Historical Squads example project |

The original preorder firmware has its own immutable
[v0.1.0 archive](../firmware/esp32-only/README.md), with flashable binaries and
matching recovered source. Use that archive when supporting preorder owners.

The later SE050/Falcon, Vector and multisig work is preserved separately on
[`research/falcon-se050`](https://github.com/hogyzen12/unruggable-rust-esp32/tree/research/falcon-se050).
Its checkpoint notes record historical tests. Local keys, raw device records,
full checkpoint payloads and build caches remain in the original development
folders; they are not part of the public archive.
