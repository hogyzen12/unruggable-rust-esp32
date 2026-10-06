#!/usr/bin/env bash
set -euo pipefail
cat >&2 <<'MESSAGE'
error: this legacy source-copy command has been retired; no files were changed.
Develop V2 in the app monorepo's firmware/iphone-ble-signer directory.
To publish a committed revision, use tools/firmware-handoff/handoff.py in the
unruggable-firmware checkout. See docs/firmware-handoff.md in either repository.
MESSAGE
exit 1
