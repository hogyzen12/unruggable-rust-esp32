#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIRMWARE_ROOT="$ROOT_DIR/firmware/iphone-ble-signer"
FLASH_SCRIPT="$FIRMWARE_ROOT/scripts/flash_se050_dev.sh"
UPSTREAM_FIRMWARE_ROOT="/Users/hogyzen12/coding-project-folders/unruggable-rust-esp32/esp32-solana-signer"
UPSTREAM_IDF_ROOT="$UPSTREAM_FIRMWARE_ROOT/.embuild/espressif"
UPSTREAM_IDF_PATH="$UPSTREAM_IDF_ROOT/esp-idf/v5.2.3"
UPSTREAM_PYTHON_ENV="$UPSTREAM_IDF_ROOT/python_env/idf5.2_py3.14_env"
UPSTREAM_XTENSA_BIN="$UPSTREAM_IDF_ROOT/tools/xtensa-esp-elf/esp-13.2.0_20230928/xtensa-esp-elf/bin"
UPSTREAM_ESP_CLANG_BIN="$UPSTREAM_IDF_ROOT/tools/esp-clang/16.0.1-fe4f10a809/esp-clang/bin"
UPSTREAM_CMAKE_BIN="$UPSTREAM_IDF_ROOT/tools/cmake/3.24.0/CMake.app/Contents/bin"
UPSTREAM_NINJA_BIN="$UPSTREAM_IDF_ROOT/tools/ninja/1.11.1"

if [[ ! -f "$FLASH_SCRIPT" ]]; then
  echo "error: vendored firmware is missing. Run ./tools/sync-iphone-firmware.sh first." >&2
  exit 1
fi

if [[ -z "${IDF_PATH:-}" && -d "$UPSTREAM_IDF_PATH" ]]; then
  export IDF_PATH="$UPSTREAM_IDF_PATH"
fi

if [[ -z "${IDF_TOOLS_PATH:-}" && -d "$UPSTREAM_IDF_ROOT" ]]; then
  export IDF_TOOLS_PATH="$UPSTREAM_IDF_ROOT"
fi

if [[ -z "${IDF_PYTHON_ENV_PATH:-}" && -d "$UPSTREAM_PYTHON_ENV" ]]; then
  export IDF_PYTHON_ENV_PATH="$UPSTREAM_PYTHON_ENV"
fi

if [[ -n "${IDF_PATH:-}" ]]; then
  export IDF_PYTHON_CHECK_CONSTRAINTS="${IDF_PYTHON_CHECK_CONSTRAINTS:-no}"
  export ESP_IDF_TOOLS_INSTALL_DIR="${ESP_IDF_TOOLS_INSTALL_DIR:-fromenv}"
fi

for tool_dir in \
  "$UPSTREAM_PYTHON_ENV/bin" \
  "$UPSTREAM_IDF_PATH/tools" \
  "$UPSTREAM_XTENSA_BIN" \
  "$UPSTREAM_ESP_CLANG_BIN" \
  "$UPSTREAM_CMAKE_BIN" \
  "$UPSTREAM_NINJA_BIN"
do
  if [[ -d "$tool_dir" ]]; then
    PATH="$tool_dir:$PATH"
  fi
done
export PATH

exec "$FLASH_SCRIPT" "$@"
