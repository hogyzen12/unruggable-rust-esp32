#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="$ROOT_DIR/target/hybrid-harness"
HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"

if [[ -z "$HOST_TARGET" ]]; then
  echo "Failed to determine host target from rustc +stable." >&2
  exit 1
fi

(
  cd "$ROOT_DIR"
  CARGO_TARGET_DIR="$TARGET_DIR" cargo +stable run \
    --target "$HOST_TARGET" \
    --manifest-path "$ROOT_DIR/tools/hybrid-harness/Cargo.toml" \
    -- "$@"
)
