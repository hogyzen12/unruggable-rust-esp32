#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec cargo +stable run --target aarch64-apple-darwin \
  --manifest-path "$ROOT_DIR/tools/pq-multisig-ops/Cargo.toml" \
  -- \
  "$@"
