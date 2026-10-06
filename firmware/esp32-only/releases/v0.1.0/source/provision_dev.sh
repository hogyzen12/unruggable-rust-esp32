#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="/dev/tty.usbserial-0001"
SKIP_BUILD=0
ERASE_FIRST=1
SKIP_EFUSE_CHECK=0
TARGET_DIR="${PROVISION_TARGET_DIR:-${ROOT_DIR}/target-dev}"
LOG_CSV="${PROVISION_LOG_CSV:-${ROOT_DIR}/logs/dev_flash_log.csv}"
BATCH_TAG="${PROVISION_BATCH_TAG:-}"

usage() {
  cat <<'EOF'
Usage: ./provision_dev.sh [options]

Options:
  --port <serial-port>   Serial port (default: /dev/tty.usbserial-0001)
  --skip-build           Skip cargo build step
  --no-erase             Do not erase flash before writing images
  --skip-efuse-check     Skip pre/post eFuse summary checks (dev-only escape hatch)
  --log-csv <path>       Append successful flashes to CSV (default: logs/dev_flash_log.csv)
  --batch-tag <tag>      Optional batch tag written to the CSV log
  -h, --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)
      PORT="${2:-}"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --no-erase)
      ERASE_FIRST=0
      shift
      ;;
    --skip-efuse-check)
      SKIP_EFUSE_CHECK=1
      shift
      ;;
    --log-csv)
      LOG_CSV="${2:-}"
      shift 2
      ;;
    --batch-tag)
      BATCH_TAG="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

trim_whitespace() {
  printf '%s' "$1" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'
}

require_cmd espflash
require_cmd python3
require_cmd strings
require_cmd rg

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  require_cmd cargo
fi

make_temp_file() {
  local prefix="$1"
  local dir="${TMPDIR:-/tmp}"
  mktemp "${dir%/}/${prefix}.XXXXXX"
}

csv_escape() {
  printf '"%s"' "$(printf '%s' "$1" | sed 's/"/""/g')"
}

parse_board_info_field() {
  local board_info="$1"
  local key="$2"
  printf '%s\n' "$board_info" | awk -v key="$key" '
    index($0, key ":") == 1 {
      print substr($0, length(key) + 3)
      exit
    }
  ' | while IFS= read -r line; do
    trim_whitespace "$line"
  done
}

extract_smoke_response() {
  local smoke_output="$1"
  local cmd="$2"
  printf '%s\n' "$smoke_output" | awk -v prefix="$cmd -> " '
    index($0, prefix) == 1 {
      line = $0
      sub(/^[^>]+ -> /, "", line)
      sub(/ \([0-9.]+ ms\)$/, "", line)
      print line
      exit
    }
  '
}

append_log_row() {
  local board_info="$1"
  local info_resp="$2"
  local pubkey_resp="$3"

  local mac_address chip_type flash_size fw_version auth_mode key_state timestamp_utc log_dir
  mac_address="$(parse_board_info_field "$board_info" "MAC address")"
  chip_type="$(parse_board_info_field "$board_info" "Chip type")"
  flash_size="$(parse_board_info_field "$board_info" "Flash size")"
  fw_version="$(printf '%s\n' "$info_resp" | tr ';' '\n' | awk -F'=' '$1 == "VER" { print $2; exit }')"
  auth_mode="$(printf '%s\n' "$info_resp" | tr ';' '\n' | awk -F'=' '$1 == "AUTH_MODE" { print $2; exit }')"
  key_state="$(printf '%s\n' "$info_resp" | tr ';' '\n' | awk -F'=' '$1 == "KEY_STATE" { print $2; exit }')"
  timestamp_utc="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  if [[ -z "$mac_address" || -z "$chip_type" || -z "$fw_version" || -z "$key_state" ]]; then
    echo "Failed to extract trace fields for logging." >&2
    echo "board-info:" >&2
    printf '%s\n' "$board_info" >&2
    echo "GET_INFO response: $info_resp" >&2
    exit 1
  fi

  log_dir="$(dirname "$LOG_CSV")"
  mkdir -p "$log_dir"
  (
    local lock_dir="${LOG_CSV}.lock"
    while ! mkdir "$lock_dir" 2>/dev/null; do
      sleep 0.05
    done
    trap 'rmdir "$lock_dir" 2>/dev/null || true' EXIT

    if [[ ! -f "$LOG_CSV" ]]; then
      cat >"$LOG_CSV" <<'EOF'
"timestamp_utc","batch_tag","port","mac_address","chip_type","flash_size","firmware_version","auth_mode","key_state","info_response","pubkey_response","status"
EOF
    fi

    {
      csv_escape "$timestamp_utc"
      printf ','
      csv_escape "$BATCH_TAG"
      printf ','
      csv_escape "$PORT"
      printf ','
      csv_escape "$mac_address"
      printf ','
      csv_escape "$chip_type"
      printf ','
      csv_escape "$flash_size"
      printf ','
      csv_escape "$fw_version"
      printf ','
      csv_escape "$auth_mode"
      printf ','
      csv_escape "$key_state"
      printf ','
      csv_escape "$info_resp"
      printf ','
      csv_escape "$pubkey_resp"
      printf ','
      csv_escape "PASS"
      printf '\n'
    } >>"$LOG_CSV"
  )
}

if ! python3 -c 'import serial' >/dev/null 2>&1; then
  echo "Missing required Python module: pyserial" >&2
  echo "Install it with: python3 -m pip install pyserial" >&2
  exit 1
fi

ESPEFUSE="${ROOT_DIR}/.embuild/espressif/python_env/idf5.2_py3.14_env/bin/espefuse.py"
if [[ "$SKIP_EFUSE_CHECK" -eq 0 && ! -x "$ESPEFUSE" ]]; then
  echo "espefuse.py not found at expected path:" >&2
  echo "  $ESPEFUSE" >&2
  exit 1
fi

if [[ "$SKIP_EFUSE_CHECK" -eq 0 ]]; then
  echo "Reading eFuse state on ${PORT}..."
  EFUSE_SUMMARY="$("$ESPEFUSE" --port "$PORT" summary)"
  if echo "$EFUSE_SUMMARY" | rg -q "ABS_DONE_0.*= True|ABS_DONE_1.*= True|FLASH_CRYPT_CNT.*= [1-9]|DISABLE_DL_ENCRYPT.*= True|DISABLE_DL_DECRYPT.*= True"; then
    echo "Refusing non-secure provisioning: target appears secure-fused." >&2
    echo "Use a fresh non-fused board for dev testing firmware." >&2
    exit 1
  fi
else
  echo "Skipping eFuse checks on ${PORT} (--skip-efuse-check)."
fi

# Force the non-secure profile for test/dev flashing.
export ESP_IDF_SDKCONFIG_DEFAULTS="sdkconfig.defaults"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "Building non-secure release firmware..."
  (
    cd "$ROOT_DIR"
    CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --target xtensa-esp32-espidf
  )
fi

BUILD_DIR="$(ls -dt "${TARGET_DIR}"/xtensa-esp32-espidf/release/build/esp-idf-sys-*/out/build 2>/dev/null | head -n1 || true)"
if [[ -z "${BUILD_DIR}" ]]; then
  echo "Could not locate ESP-IDF build directory." >&2
  exit 1
fi

SDKCONFIG_PATH="${BUILD_DIR%/build}/sdkconfig"
if [[ ! -f "$SDKCONFIG_PATH" ]]; then
  echo "Missing generated sdkconfig: $SDKCONFIG_PATH" >&2
  exit 1
fi

# Hard fail if secure flags leaked into this build.
if rg -q '^CONFIG_SECURE_BOOT=y$' "$SDKCONFIG_PATH" \
  || rg -q '^CONFIG_SECURE_FLASH_ENC_ENABLED=y$' "$SDKCONFIG_PATH"; then
  echo "Safety check failed: build artifacts are secure-profile outputs." >&2
  echo "Aborting non-secure flash." >&2
  echo "Hint:" >&2
  echo "  unset ESP_IDF_SDKCONFIG_DEFAULTS" >&2
  echo "  cargo clean -p esp-idf-sys" >&2
  echo "  ./provision_dev.sh --port ${PORT}" >&2
  exit 1
fi

APP_ELF="${TARGET_DIR}/xtensa-esp32-espidf/release/esp32-solana-signer"
if [[ ! -f "$APP_ELF" ]]; then
  echo "Missing app ELF: $APP_ELF" >&2
  exit 1
fi

BOOT_BIN="${BUILD_DIR}/bootloader/bootloader.bin"
PART_BIN="${BUILD_DIR}/partition_table/partition-table.bin"
for f in "$BOOT_BIN" "$PART_BIN"; do
  if [[ ! -f "$f" ]]; then
    echo "Missing required binary: $f" >&2
    exit 1
  fi
done

offset_from_flash_args() {
  local path="$1"
  awk 'NF>=2 && $1 ~ /^0x[0-9a-fA-F]+$/ { print $1; exit }' "$path"
}

BOOT_OFFSET="$(offset_from_flash_args "${BUILD_DIR}/flash_bootloader_args")"
PART_OFFSET="$(offset_from_flash_args "${BUILD_DIR}/partition-table-flash_args")"
APP_OFFSET="$(offset_from_flash_args "${BUILD_DIR}/flash_app_args")"
for v in "$BOOT_OFFSET" "$PART_OFFSET" "$APP_OFFSET"; do
  if [[ -z "$v" ]]; then
    echo "Failed to parse flash offsets from ESP-IDF flash args files." >&2
    exit 1
  fi
done

APP_BIN="$(make_temp_file esp32-signer-app-dev)"
echo "Generating app image from final Rust ELF..."
espflash save-image --chip esp32 --flash-mode dio --flash-freq 40mhz --flash-size 4mb "$APP_ELF" "$APP_BIN"

STRINGS_DUMP="$(make_temp_file esp32-signer-dev-strings)"
cleanup() {
  local path
  for path in "${APP_BIN:-}" "${STRINGS_DUMP:-}"; do
    if [[ -n "$path" ]]; then
      rm -f "$path"
    fi
  done
}
trap cleanup EXIT
strings "$APP_BIN" > "$STRINGS_DUMP"

if ! rg -q "SET_MODE:PIN|UNLOCK:PIN|EXPORT_PRIVKEY|GET_INFO|SIGNATURE:|PRIVKEY:" "$STRINGS_DUMP"; then
  echo "Safety check failed: generated app image does not contain signer protocol markers." >&2
  echo "Aborting without flashing." >&2
  exit 1
fi

echo
echo "Ready to flash non-secure dev firmware to ${PORT}:"
echo "  cargo target dir: ${TARGET_DIR}"
echo "  build dir: ${BUILD_DIR}"
echo "  bootloader: ${BOOT_BIN} @ ${BOOT_OFFSET}"
echo "  partition:  ${PART_BIN} @ ${PART_OFFSET}"
echo "  app image:  ${APP_BIN} @ ${APP_OFFSET}"
if [[ "$ERASE_FIRST" -eq 1 ]]; then
  echo "  mode: erase + full image write"
  echo "  WARNING: erase-flash wipes NVS and will destroy any existing signer key/auth state on this unit."
else
  echo "  mode: full image write (no erase)"
  echo "  NOTE: no-erase preserves existing NVS signer key/auth state on this unit."
fi
echo
echo "Flashing immediately (no manual confirmation prompt)."

if [[ "$ERASE_FIRST" -eq 1 ]]; then
  espflash erase-flash --port "$PORT"
fi

echo "Flashing bootloader @ ${BOOT_OFFSET}"
espflash write-bin --chip esp32 --port "$PORT" "$BOOT_OFFSET" "$BOOT_BIN"

echo "Flashing partition table @ ${PART_OFFSET}"
espflash write-bin --chip esp32 --port "$PORT" "$PART_OFFSET" "$PART_BIN"

echo "Flashing app image @ ${APP_OFFSET}"
espflash write-bin --chip esp32 --port "$PORT" "$APP_OFFSET" "$APP_BIN"

echo "Running post-flash checks..."
if ! espflash reset --port "$PORT" >/dev/null 2>&1; then
  echo "Warning: automatic reset failed; press reset (EN) once now." >&2
fi
sleep 2

if [[ "$SKIP_EFUSE_CHECK" -eq 0 ]]; then
  POST_EFUSE_SUMMARY="$("$ESPEFUSE" --port "$PORT" summary)"
  if echo "$POST_EFUSE_SUMMARY" | rg -q "ABS_DONE_0.*= True|ABS_DONE_1.*= True|FLASH_CRYPT_CNT.*= [1-9]|UART_DOWNLOAD_DIS.*= True"; then
    echo "Post-flash check failed: target is not in expected non-secure dev state." >&2
    echo "$POST_EFUSE_SUMMARY" | rg "ABS_DONE|FLASH_CRYPT_CNT|UART_DOWNLOAD_DIS|DISABLE_DL_ENCRYPT|DISABLE_DL_DECRYPT" >&2 || true
    exit 1
  fi
fi

echo "Running UART smoke test..."
if ! SMOKE_OUTPUT="$(
  python3 "${ROOT_DIR}/protocol_rtt.py" \
  --port "$PORT" \
  --timeout 4.0 \
  --cmd "GET_INFO" \
  --expect-regex '^INFO;VER=.*;AUTH_MODE=UNSET;FINALIZED=0;LOCKED=0;RETRIES_LEFT=10;KEY_STATE=UNINITIALIZED$' \
  --cmd "GET_PUBKEY" \
  --expect-regex '^ERROR:WALLET_NOT_INITIALIZED$'
)"; then
  printf '%s\n' "$SMOKE_OUTPUT"
  exit 1
fi
printf '%s\n' "$SMOKE_OUTPUT"

echo "Reading hardware fingerprint for trace log..."
BOARD_INFO="$(espflash board-info --port "$PORT")"
printf '%s\n' "$BOARD_INFO"

INFO_RESP="$(extract_smoke_response "$SMOKE_OUTPUT" "GET_INFO")"
PUBKEY_RESP="$(extract_smoke_response "$SMOKE_OUTPUT" "GET_PUBKEY")"
append_log_row "$BOARD_INFO" "$INFO_RESP" "$PUBKEY_RESP"

cat <<EOF

Non-secure dev flash complete and health checks passed.
Next checks:
  1. User plugs device in and sets PIN with SET_MODE:PIN:<6digits>
  2. Host calls GET_PUBKEY after MODE_SET:PIN to read the newly created wallet
Trace log appended to:
  ${LOG_CSV}
EOF
