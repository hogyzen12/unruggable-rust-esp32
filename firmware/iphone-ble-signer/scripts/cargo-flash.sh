#!/usr/bin/env bash
set -euo pipefail

# Cargo passes the exact ELF, which also determines the matching SDK bootloader.
ARTIFACT="${1:?Cargo must supply an ELF path}"
shift
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOOTLOADER="$(dirname "$ARTIFACT")/bootloader.bin"
[[ -f "$BOOTLOADER" ]] || { echo "Missing matching bootloader: $BOOTLOADER" >&2; exit 1; }
PREPARED_BOOTLOADER="$(dirname "$ARTIFACT")/bootloader-espflash.bin"
python3 "$SCRIPT_DIR/prepare_bootloader.py" "$BOOTLOADER" "$PREPARED_BOOTLOADER"
exec espflash flash --monitor --chip esp32s3 --before default-reset --after hard-reset \
  --bootloader "$PREPARED_BOOTLOADER" --flash-size 4mb --flash-mode dio --flash-freq 80mhz \
  --partition-table "$SCRIPT_DIR/../partitions/unified_wallet_4mb.csv" "$ARTIFACT" "$@"
