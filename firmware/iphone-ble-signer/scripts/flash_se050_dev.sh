#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_TRIPLE="xtensa-esp32s3-espidf"
BIN_NAME="esp32-solana-signer"
IDF_VERSION="v5.2.3"

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
  --display-offset-x <px>
  --display-offset-y <px>
  --display-spi-hz <hz>
  --display-color-order <rgb|bgr>
  --display-inverted <true|false>
  --display-rotate-180 <true|false>
                Optional display calibration overrides.
  --button-pin <gpio>
                Optional user-button GPIO override.
  --no-button   Disable the user button for this firmware build.
  --bin <name>
                Cargo bin target to build/flash. Defaults to `esp32-solana-signer`.
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
DISPLAY_OFFSET_X=""
DISPLAY_OFFSET_Y=""
DISPLAY_SPI_HZ=""
DISPLAY_COLOR_ORDER=""
DISPLAY_INVERTED=""
DISPLAY_ROTATE_180=""
USER_BUTTON_PIN=""
NO_BUTTON=0

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
    --display-offset-x)
      DISPLAY_OFFSET_X="${2:-}"
      shift 2
      ;;
    --display-offset-y)
      DISPLAY_OFFSET_Y="${2:-}"
      shift 2
      ;;
    --display-spi-hz)
      DISPLAY_SPI_HZ="${2:-}"
      shift 2
      ;;
    --display-color-order)
      DISPLAY_COLOR_ORDER="${2:-}"
      shift 2
      ;;
    --display-inverted)
      DISPLAY_INVERTED="${2:-}"
      shift 2
      ;;
    --display-rotate-180)
      DISPLAY_ROTATE_180="${2:-}"
      shift 2
      ;;
    --button-pin)
      USER_BUTTON_PIN="${2:-}"
      shift 2
      ;;
    --no-button)
      NO_BUTTON=1
      shift
      ;;
    --bin)
      BIN_NAME="${2:-}"
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

PARTITION_TABLE="$ROOT_DIR/partitions/unified_wallet_4mb.csv"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  BUILD_CMD=(cargo +esp build --locked --bin "$BIN_NAME")
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
    [[ -n "$DISPLAY_OFFSET_X" ]] && export SIGNER_DISPLAY_OFFSET_X="$DISPLAY_OFFSET_X"
    [[ -n "$DISPLAY_OFFSET_Y" ]] && export SIGNER_DISPLAY_OFFSET_Y="$DISPLAY_OFFSET_Y"
    [[ -n "$DISPLAY_SPI_HZ" ]] && export SIGNER_DISPLAY_SPI_HZ="$DISPLAY_SPI_HZ"
    [[ -n "$DISPLAY_COLOR_ORDER" ]] && export SIGNER_DISPLAY_COLOR_ORDER="$DISPLAY_COLOR_ORDER"
    [[ -n "$DISPLAY_INVERTED" ]] && export SIGNER_DISPLAY_INVERTED="$DISPLAY_INVERTED"
    [[ -n "$DISPLAY_ROTATE_180" ]] && export SIGNER_DISPLAY_ROTATE_180="$DISPLAY_ROTATE_180"
    if [[ "$NO_BUTTON" -eq 1 ]]; then
      export SIGNER_USER_BUTTON_PIN="none"
    elif [[ -n "$USER_BUTTON_PIN" ]]; then
      export SIGNER_USER_BUTTON_PIN="$USER_BUTTON_PIN"
    fi
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

BOOTLOADER="$ROOT_DIR/target/$TARGET_TRIPLE/$PROFILE/bootloader.bin"
if [[ ! -f "$BOOTLOADER" ]]; then
  echo "Matching ESP-IDF bootloader not found: $BOOTLOADER. Build the firmware before flashing." >&2
  exit 1
fi
PREPARED_BOOTLOADER="$ROOT_DIR/target/$TARGET_TRIPLE/$PROFILE/bootloader-espflash.bin"
python3 "$ROOT_DIR/scripts/prepare_bootloader.py" "$BOOTLOADER" "$PREPARED_BOOTLOADER"

if [[ "$SKIP_BUILD" -eq 1 ]]; then
  echo "warning: --skip-build reuses the existing artifact at $BIN_PATH." >&2
  echo "warning: board profile, button, and pin override flags only affect the build step; verify the cached artifact was built with the settings you want to flash." >&2
fi

if [[ "$BIN_NAME" == "iphone_ble_wallet" ]]; then
  echo "warning: iphone_ble_wallet is a BLE proof image with a mock pubkey and unsupported PIN/sign flows." >&2
fi

if [[ "$NO_BUTTON" -eq 1 ]]; then
  echo "warning: --no-button disables the hardware confirmation button. The real signer refuses physical-confirmation gates when no button is present." >&2
fi

if [[ "$ERASE" -eq 1 ]]; then
  echo "warning: --erase clears flash, including NVS auth/PIN state. Avoid it unless you intentionally want a reset." >&2
  espflash erase-flash --port "$PORT"
fi

FLASH_CMD=(
  espflash
  flash
  --chip esp32s3
  --before default-reset
  --after hard-reset
  --bootloader "$PREPARED_BOOTLOADER"
  --flash-size 4mb
  --flash-mode dio
  --flash-freq 80mhz
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
