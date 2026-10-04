#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_SOURCE="/Users/hogyzen12/coding-project-folders/unruggable-rust-esp32/esp32-solana-signer"
SOURCE_ROOT="${1:-${SOURCE_FIRMWARE_ROOT:-$DEFAULT_SOURCE}}"
DEST_ROOT="$ROOT_DIR/firmware/iphone-ble-signer"

die() {
  echo "error: $*" >&2
  exit 1
}

copy_tree() {
  local src_rel="$1"
  local dest_rel="${2:-$1}"
  local src_path="$SOURCE_ROOT/$src_rel"
  local dest_path="$DEST_ROOT/$dest_rel"

  [[ -d "$src_path" ]] || die "missing source directory: $src_path"
  mkdir -p "$dest_path"
  rsync -a \
    --exclude '.DS_Store' \
    --exclude '.embuild' \
    --exclude '.localnet' \
    --exclude '__pycache__' \
    --exclude 'target' \
    "$src_path/" "$dest_path/"
}

copy_file() {
  local src_rel="$1"
  local dest_rel="${2:-$1}"
  local src_path="$SOURCE_ROOT/$src_rel"
  local dest_path="$DEST_ROOT/$dest_rel"

  [[ -f "$src_path" ]] || die "missing source file: $src_path"
  mkdir -p "$(dirname "$dest_path")"
  cp "$src_path" "$dest_path"
}

[[ -d "$SOURCE_ROOT" ]] || die "source firmware repo not found: $SOURCE_ROOT"
command -v rsync >/dev/null 2>&1 || die "rsync is required"

mkdir -p "$DEST_ROOT"

copy_file ".gitignore"
copy_file "Cargo.toml"
copy_file "Cargo.lock"
copy_file "build.rs"
copy_file "rust-toolchain.toml"
copy_file "sdkconfig.defaults"
copy_file "README.md" "UPSTREAM-README.md"
copy_file "docs/dual-element-board-review.md" "docs/dual-element-board-review.md"
copy_file "scripts/flash_se050_dev.sh" "scripts/flash_se050_dev.sh"

copy_tree ".cargo"
copy_tree "assets"
copy_tree "src"
copy_tree "vendor"

echo "Synced firmware into $DEST_ROOT"
