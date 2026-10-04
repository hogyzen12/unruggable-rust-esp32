# PCB Diagnostic

Connect one V2 board, then run from this firmware directory:

```bash
./scripts/flash_factory_test.sh --port /dev/cu.usbmodem2101
```

Check the RGBW pattern and border, then press and release the button.

- `PASS_PRE_TROPIC`: PCB passed; Tropic is not fitted.
- `PASS`: all parts, including Tropic, passed.
- `FAIL`: inspect the component results in the serial output.

The test does not provision secure elements. Reset to repeat, then flash the
production firmware when finished.
