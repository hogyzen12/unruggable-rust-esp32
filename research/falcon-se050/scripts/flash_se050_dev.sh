#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_TRIPLE="xtensa-esp32s3-espidf"
BIN_NAME="esp32-solana-signer"
IDF_VERSION="v5.2.3"
PARTITION_TABLE="$ROOT_DIR/.embuild/espressif/esp-idf/$IDF_VERSION/components/partition_table/partitions_singleapp_large.csv"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/flash_se050_dev.sh --port /dev/cu.usbmodemXXXX [options]

Options:
  --board-profile <name>
                Build for a specific board profile. Supported values:
                `unruggable-v1` (default) or `dual-element-se050`.
  --se050-sda-pin <gpio>
                Optional override for `dual-element-se050`.
  --se050-scl-pin <gpio>
                Optional override for `dual-element-se050`.
  --se050-enable-pin <gpio>
                Optional enable/power GPIO for `dual-element-se050`.
  --display-sck-pin <gpio>
  --display-mosi-pin <gpio>
  --display-cs-pin <gpio>
  --display-dc-pin <gpio>
  --display-rst-pin <gpio>
  --display-bl-pin <gpio>
                Optional dual-element display pin overrides. If any are set,
                all 6 are expected by the firmware build.
  --erase        Erase flash first.
  --release      Build the release artifact instead of debug.
  --skip-build   Reuse the existing artifact in target/.
  --monitor      Attach espflash monitor after flashing.
  -h, --help     Show this help text.
EOF
}

PORT=""
PROFILE="debug"
ERASE=0
SKIP_BUILD=0
MONITOR=0
BOARD_PROFILE="unruggable-v1"
SE050_SDA_PIN=""
SE050_SCL_PIN=""
SE050_ENABLE_PIN=""
DISPLAY_SCK_PIN=""
DISPLAY_MOSI_PIN=""
DISPLAY_CS_PIN=""
DISPLAY_DC_PIN=""
DISPLAY_RST_PIN=""
DISPLAY_BL_PIN=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    --erase)
      ERASE=1
      shift
      ;;
    --board-profile)
      BOARD_PROFILE="${2:-}"
      shift 2
      ;;
    --se050-sda-pin)
      SE050_SDA_PIN="${2:-}"
      shift 2
      ;;
    --se050-scl-pin)
      SE050_SCL_PIN="${2:-}"
      shift 2
      ;;
    --se050-enable-pin)
      SE050_ENABLE_PIN="${2:-}"
      shift 2
      ;;
    --display-sck-pin)
      DISPLAY_SCK_PIN="${2:-}"
      shift 2
      ;;
    --display-mosi-pin)
      DISPLAY_MOSI_PIN="${2:-}"
      shift 2
      ;;
    --display-cs-pin)
      DISPLAY_CS_PIN="${2:-}"
      shift 2
      ;;
    --display-dc-pin)
      DISPLAY_DC_PIN="${2:-}"
      shift 2
      ;;
    --display-rst-pin)
      DISPLAY_RST_PIN="${2:-}"
      shift 2
      ;;
    --display-bl-pin)
      DISPLAY_BL_PIN="${2:-}"
      shift 2
      ;;
    --release)
      PROFILE="release"
      shift
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --monitor)
      MONITOR=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$PORT" ]]; then
  echo "Missing required --port value." >&2
  usage >&2
  exit 1
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  BUILD_CMD=(cargo +esp build --bin "$BIN_NAME")
  if [[ "$PROFILE" == "release" ]]; then
    BUILD_CMD+=(--release)
  fi

  (
    cd "$ROOT_DIR"
    export SIGNER_BOARD_PROFILE="$BOARD_PROFILE"
    [[ -n "$SE050_SDA_PIN" ]] && export SIGNER_SE050_SDA_PIN="$SE050_SDA_PIN"
    [[ -n "$SE050_SCL_PIN" ]] && export SIGNER_SE050_SCL_PIN="$SE050_SCL_PIN"
    [[ -n "$SE050_ENABLE_PIN" ]] && export SIGNER_SE050_ENABLE_PIN="$SE050_ENABLE_PIN"
    [[ -n "$DISPLAY_SCK_PIN" ]] && export SIGNER_DISPLAY_SCK_PIN="$DISPLAY_SCK_PIN"
    [[ -n "$DISPLAY_MOSI_PIN" ]] && export SIGNER_DISPLAY_MOSI_PIN="$DISPLAY_MOSI_PIN"
    [[ -n "$DISPLAY_CS_PIN" ]] && export SIGNER_DISPLAY_CS_PIN="$DISPLAY_CS_PIN"
    [[ -n "$DISPLAY_DC_PIN" ]] && export SIGNER_DISPLAY_DC_PIN="$DISPLAY_DC_PIN"
    [[ -n "$DISPLAY_RST_PIN" ]] && export SIGNER_DISPLAY_RST_PIN="$DISPLAY_RST_PIN"
    [[ -n "$DISPLAY_BL_PIN" ]] && export SIGNER_DISPLAY_BL_PIN="$DISPLAY_BL_PIN"
    echo "Building $BIN_NAME for board profile: $SIGNER_BOARD_PROFILE"
    "${BUILD_CMD[@]}"
  )
fi

BIN_PATH="$ROOT_DIR/target/$TARGET_TRIPLE/$PROFILE/$BIN_NAME"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Artifact not found: $BIN_PATH" >&2
  exit 1
fi

if [[ ! -f "$PARTITION_TABLE" ]]; then
  echo "Partition table not found: $PARTITION_TABLE" >&2
  exit 1
fi

if [[ "$ERASE" -eq 1 ]]; then
  espflash erase-flash --port "$PORT"
fi

FLASH_CMD=(
  espflash
  flash
  --chip esp32s3
  --flash-size 8mb
  --before usb-reset
  --after hard-reset
  --partition-table "$PARTITION_TABLE"
  --port "$PORT"
)

if [[ "$MONITOR" -eq 1 ]]; then
  FLASH_CMD+=(--monitor)
fi

FLASH_CMD+=("$BIN_PATH")

(
  cd "$ROOT_DIR"
  "${FLASH_CMD[@]}"
)
