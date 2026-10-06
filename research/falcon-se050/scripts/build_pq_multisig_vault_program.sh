#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VAULT_DIR="$ROOT_DIR/pq-multisig-vault"

(
  cd "$VAULT_DIR"
  cargo build-sbf --manifest-path "$VAULT_DIR/program/Cargo.toml"
)
