#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VECTOR_DIR="$ROOT_DIR/vector"

(
  cd "$VECTOR_DIR"
  cargo build-sbf --manifest-path "$VECTOR_DIR/programs/falcon512/Cargo.toml"
)
