> Historical development commands. For the recovered ESP32 preorder firmware,
> use [the frozen v0.1.0 archive](../../../firmware/esp32-only/releases/v0.1.0/README.md).
> For V2 hardware, use [the dual-element release](../../../firmware/dual-element/README.md).

cargo +esp build
to compile

and then to flash:
espflash flash target/xtensa-esp32-espidf/debug/esp32-solana-signer --port /dev/tty.usbserial-0001

