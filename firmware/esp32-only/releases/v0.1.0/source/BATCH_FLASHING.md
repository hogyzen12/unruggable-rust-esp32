# Dev Batch Flashing Flow

This is the recommended high-throughput flow for flashing large numbers of non-secure dev boards, such as 50 to 200 units, while keeping the process repeatable and auditable.

Use this flow for dev units only.

- Do not use it for secure provisioning.
- Do not parallelize `./provision_secure.sh`.

## Goals

- Build the firmware once.
- Flash multiple boards in parallel across fixed USB slots.
- Keep one shared trace log plus one per-run summary bundle.
- Make failed boards easy to quarantine and retry.

## Recommended Station Layout

- Use one powered USB hub per station.
- Keep a fixed cable-to-slot mapping and label the slots, for example `A` through `H`.
- Keep one serial-port list per station if auto-discovery is noisy.
- Treat one station run as one batch round, not the entire 200-board job.

For a 200-board run, the efficient pattern is:

1. Validate one firmware artifact.
2. Flash 4 to 8 boards in parallel per round.
3. Swap only the completed boards.
4. Re-run the same station command until the lot is complete.

## One-Time Preflight Per Firmware Revision

Set the validated artifact directory:

```bash
export PROVISION_TARGET_DIR="$PWD/target"
```

Build the release artifact once:

```bash
cargo +esp build --release --target xtensa-esp32-espidf
```

Run one full end-to-end QA board before opening the batch:

```bash
python3 -u qa_e2e_dev.py \
  --port /dev/cu.usbserial-0001 \
  --pin 246810 \
  --yes \
  --skip-build
```

Reset that QA board back to blank stock if it is going into the shippable dev batch:

```bash
./reset_dev_unit.sh --port /dev/cu.usbserial-0001
```

## Station Run Options

### Recommended reliable hub mode

For the dedicated USB-C flashing hub, use the simple terminal station script first. It auto-detects the saved hub slots and flashes them sequentially with full live logs.

List the detected slots:

```bash
python3 -u flash_hub_dev.py --list
```

Flash every detected slot on the hub:

```bash
python3 -u flash_hub_dev.py --all
```

Flash only specific slots:

```bash
python3 -u flash_hub_dev.py --slots 1,3,4
```

Flash and leave each board initialized with a known PIN. The script shows a short cue popup before each manual button step, then continues automatically:

```bash
python3 -u flash_hub_dev.py --all --setup-pin 246810
```

This is the recommended operator path when reliability matters more than raw throughput. It intentionally flashes one slot at a time.

The raw `/dev/cu...` names can change between reconnects. The stable identifiers for this station are the hub slots derived from the saved USB `LOCATION` prefix.

### GUI station mode

Launch the local multi-port GUI:

```bash
python3 -u dev_flash_gui.py --skip-build --skip-efuse-check
```

The GUI wraps `provision_dev_batch.py`, so it uses the same flash logic, shared CSV trace log, and per-run batch summaries. Use the GUI when one operator is cycling boards through a fixed hub and wants live per-port status without typing the round command each time.

The default operator view is intentionally simple:

1. Choose the firmware target from the dropdown.
2. Set or confirm the batch tag.
3. Refresh and select the ports you want to flash.
4. Click `Flash Selected`.

For a dedicated hub station, do one setup pass first:

1. Plug one or more boards into the new hub.
2. Open `Show Advanced`.
3. If the machine has other serial devices, select only the hub boards first.
4. Click `Capture From Visible Ports` to store the hub's USB `LOCATION` prefix.
5. Save the target profile.

After that, the operator view will only show boards connected under that hub path, and it will sort them by derived hub slot/location instead of raw `/dev/cu...` name order.

The GUI reads saved target profiles from `flash_profiles.json`. Each profile can define:

- hub location prefix for a dedicated flashing hub
- firmware project root
- provision script path
- target artifact directory
- build command
- artifact check path
- shared trace CSV path
- default parallelism and default flags
- port prefixes for device discovery

The GUI also has a `Manual Ports` field for cases where a hub-attached device port needs to be added explicitly even if auto-discovery is incomplete.

Use `Show Advanced` only when you need to create or edit a target profile, change the build directory, change the provision script, or adjust serial-port discovery behavior.

That is the intended extension point for additional device families like the SE050 screen prototype and the final SE050 + TROPIC production prototype.

### Option A: Explicit fixed ports

Use this when your station always uses the same cable/port mapping.

```bash
python3 -u provision_dev_batch.py \
  --port /dev/cu.usbserial-0001 \
  --port /dev/cu.usbserial-0002 \
  --port /dev/cu.usbserial-0003 \
  --port /dev/cu.usbserial-0004 \
  --parallel 4 \
  --skip-build \
  --skip-efuse-check \
  --batch-tag dev-2026-05-24-r1
```

### Option B: Auto-discover ports

Use this only if the machine does not have unrelated serial devices attached.

```bash
python3 -u provision_dev_batch.py \
  --auto-ports \
  --parallel 4 \
  --skip-build \
  --skip-efuse-check \
  --batch-tag dev-2026-05-24-r1
```

### Option C: Ports file per station

Create a text file with one port per line, for example:

```text
/dev/cu.usbserial-0001
/dev/cu.usbserial-0002
/dev/cu.usbserial-0003
/dev/cu.usbserial-0004
```

Then run:

```bash
python3 -u provision_dev_batch.py \
  --ports-file station-01-ports.txt \
  --parallel 4 \
  --skip-build \
  --skip-efuse-check \
  --batch-tag dev-2026-05-24-r1
```

## What The Batch Runner Does

- Reuses the validated ELF from `PROVISION_TARGET_DIR`.
- Launches one `./provision_dev.sh` worker per selected port.
- Writes the normal shared fleet trace log to `logs/dev_flash_log.csv`.
- Writes a per-run bundle under `logs/batches/<timestamp>-<batch-tag>/`.

Each per-run bundle contains:

- one flash log per port
- `summary.csv`
- `summary.json`
- `run_manifest.json`

## Recommended Repeated Operator Cycle

For each round:

1. Plug fresh blank boards into all station slots.
2. Run `provision_dev_batch.py` for that station.
3. Remove all `PASS` boards.
4. Quarantine any `FAIL` boards for manual retry or inspection.
5. Plug the next set of blank boards into the open slots.
6. Re-run the same station command with a new round tag.

Recommended batch tags:

- `dev-2026-05-24-r1`
- `dev-2026-05-24-r2`
- `dev-2026-05-24-r3`

This keeps the big 200-board job split into smaller traceable rounds.

## QA Cadence During Large Runs

Do not run full QA on every unit.

Recommended cadence:

1. Run one full `qa_e2e_dev.py` board before the batch opens.
2. Run `qa_sample_dev.py` on one sample board every 20 to 25 flashed units.
3. If any sample QA fails, stop the batch and investigate before flashing more boards.

Example sample QA command:

```bash
python3 qa_sample_dev.py --port /dev/cu.usbserial-0001 --pin 246810
```

## Failure Handling

If a board fails during the batch runner:

1. Leave the successful boards moving.
2. Move the failed board to a quarantine pile.
3. Review the per-port log in the run directory.
4. Retry that board manually with `./provision_dev.sh`.
5. If it still fails, inspect cable, USB power, bootloader entry, and board health.

If `espflash` stalls at `Connecting...`:

1. Hold `BOOT`.
2. Tap `EN`.
3. Keep holding `BOOT` until `Using flash stub` appears.
4. Release `BOOT`.

## Throughput Notes

- Parallelism should usually match the number of stable cables at the station.
- Start with `4` workers even if you have more ports.
- Only increase beyond `4` after one clean round with zero contention issues.
- Building once and using `--skip-build` for every round is the main speed win.
- Keeping per-port logs and shared CSV traceability is the main correctness win.

## Command Summary

Build validated artifact:

```bash
export PROVISION_TARGET_DIR="$PWD/target"
cargo +esp build --release --target xtensa-esp32-espidf
```

Run one 4-port batch round:

```bash
python3 -u provision_dev_batch.py \
  --port /dev/cu.usbserial-0001 \
  --port /dev/cu.usbserial-0002 \
  --port /dev/cu.usbserial-0003 \
  --port /dev/cu.usbserial-0004 \
  --parallel 4 \
  --skip-build \
  --skip-efuse-check \
  --batch-tag dev-2026-05-24-r1
```
