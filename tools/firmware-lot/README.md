# MAC-based lot records

The hardware MAC is the only device ID. Firmware v0.3.1 shows it under
Menu → Device info. No labels, serial assignment, operator entry or packing
spreadsheet is needed. Each factory run automatically saves a compact JSON record;
only failures retain verbose diagnostics. The records directory names the lot.

```sh
python3 -m unittest discover -s tools/firmware-lot -v
python3 tools/firmware-lot/lot_report.py \
  --package /path/to/selected-firmware \
  --records /path/to/prize-200-records --lot prize-200-records \
  --expected 200 --output /path/to/new-report-directory
```

The report opens no device. It verifies the selected package and counts each MAC
once using its latest factory attempt. A later failure replaces an earlier pass.
Failed units remain separate from passing units; unidentified connection failures
are retained without inventing a device identity. Exit 0 means exactly the target
number have valid passing station records. It never grants shipping approval.

`units.csv` contains MAC, UTC time, firmware build, PASS/FAIL and error. Retests
retain previous JSON records, without creating extra counted units. For support,
ask for the full MAC and firmware version from Device info, then search the CSV or
MAC-prefixed JSON filenames. Back up that folder with the exact firmware package.
