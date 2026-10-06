# Dual-Element Board Review

This note captures the current state of the newer SE050-first / Tropic-ready
board bring-up work in `esp32-solana-signer`.

## What We Confirmed

- The current signer firmware was originally hardcoded for the **Unruggable V1**
  ESP32-S3 board:
  - SE050 enable: `GPIO21`
  - SE050 I2C: `SDA=GPIO47`, `SCL=GPIO26`
  - TFT: `SCK=41`, `MOSI=40`, `CS=42`, `DC=39`, `RST=38`, `BL=37`
- The newer `tropic-multidevice-breadboard` repo is still a useful
  **architecture** reference for the dual-element model, but it is **not a
  drop-in pin map** for this firmware:
  - that repo targets `xtensa-esp32-espidf` (classic ESP32)
  - the connected hardware here is an **ESP32-S3**
  - its raw GPIO assignments like `SDA=21`, `SCL=22`, `MOSI=23` were only
    useful as a conceptual guide

## V2.0 Schematic-Derived Pin Map

The `Unruggable V2.0/PDF/SCH_Unruggable V2.0.pdf` schematic gives the actual
ESP32-S3 GPIO map for this PCB:

- SE050:
  - `SE_ENA -> IO15`
  - `SE_SCL -> IO21`
  - `SE_SDA -> IO26`
- TFT:
  - `TFT_CS -> IO42`
  - `TFT_SCL -> IO41`
  - `TFT_SDA -> IO40`
  - `TFT_DC -> IO39`
  - `TFT_RES -> IO38`
  - `TFT_BL -> IO37`
- Tropic footprint wiring for future bring-up:
  - `TROPIC_CSN -> IO47`
  - `TROPIC_SCK -> IO33`
  - `TROPIC_MISO -> IO34`
  - `TROPIC_MOSI -> IO48`
- User button:
  - `USER_BTN -> IO5`

## Hardware Evidence From The Fresh Board

- Legacy V1 probe path:
  - no devices found on `GPIO47/GPIO26`
  - `SE050` soft reset on `0x48` failed with `ESP_FAIL`
- Dual-element breadboard reference path:
  - `GPIO22` was rejected by the S3 HAL as an invalid SCL pin
  - the reference display/Tropic SPI pins like `GPIO23` also failed as invalid
    on this board

## Result

The bring-up blocker is no longer the pin map. The firmware now has the actual
Unruggable V2.0 board definition and can be flashed without external pin
arguments.

## Firmware Work Completed

- Added a board-profile layer:
  - `unruggable-v1` (default)
  - `dual-element-se050`
- `GET_INFO` now reports `BOARD=<profile>`
- Added explicit build-time pin overrides for newer boards:
  - `SIGNER_SE050_SDA_PIN`
  - `SIGNER_SE050_SCL_PIN`
  - `SIGNER_SE050_ENABLE_PIN` (optional)
  - `SIGNER_DISPLAY_*` pins (optional)
- Updated `scripts/flash_se050_dev.sh` to accept those overrides directly
- Updated the peripheral probe to use the same board-profile abstraction
- Promoted `dual-element-se050` into a real built-in profile for
  **Unruggable V2.0** with defaults from the released schematic:
  - SE050: `ENA=15`, `SDA=26`, `SCL=21`
  - TFT: `CS=42`, `SCK=41`, `SDA=40`, `DC=39`, `RST=38`, `BL=37`

## Safe Build Shape For A V2.0 Board

```bash
./scripts/flash_se050_dev.sh \
  --port /dev/cu.usbmodemXXXX \
  --board-profile dual-element-se050
```

If you ever spin a variant of the V2.0 PCB and need to override the defaults,
the same script still accepts explicit pin overrides:

```bash
  --se050-sda-pin <gpio> \
  --se050-scl-pin <gpio> \
  [--se050-enable-pin <gpio>] \
  --display-sck-pin <gpio> \
  --display-mosi-pin <gpio> \
  --display-cs-pin <gpio> \
  --display-dc-pin <gpio> \
  --display-rst-pin <gpio> \
  --display-bl-pin <gpio>
```

## Next Step

Flash the probe and signer firmware on the real V2.0 hardware and verify:

- the SE050 responds on `IO26/IO21`
- the `SE_ENA` power-cycle path works on `IO15`
- if populated, the Tropic/TR01 responds on `IO47/IO33/IO34/IO48`
- the TFT comes up on the mapped SPI/control pins
- once both secure elements are confirmed electrically, decide whether Tropic
  should remain a probe-only diagnostic or be integrated into the main signer
  flow
