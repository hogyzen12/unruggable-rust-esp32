# ESP32-only — customer preorder firmware v0.1.0

This is the original ESP32 firmware for boards **without SE050, Tropic or a
display**. Wallet keys live in ESP32 NVS; owner setup uses a six-digit PIN and
physical approval uses the BOOT button.

The stable archived baseline is [releases/v0.1.0](releases/v0.1.0/README.md).
It contains the recovered ELF, flashable images, source with locked dependencies,
original non-secure flashing/QA helpers and public evidence summaries.

The operator confirmed this as the firmware used for the 2026 customer preorder
boards. April runs were for SPI; the May and June runs were identified as the
customer preorder cohort. Their 533 successful flash records cover 523 distinct
boards, all reporting v0.1.0 and a blank wallet ready for owner setup.

The source and recovered binary are frozen. The original logs lack binary
hashes, so the archive does not claim a byte-for-byte comparison with every
shipped board. The [provenance notes](releases/v0.1.0/evidence/provenance.md)
separate operator confirmation, historical records and the recovery bench check.

Verify from the repository root:

```sh
python3 firmware/verify.py --family esp32-only
```

Future changes belong in a separate development checkout and a new archive
version. Keep this release as the reference for supporting preorder owners.
