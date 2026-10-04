# BLE Proof Plan

This is the minimum proof chain for the iPhone-specific BLE firmware fork.

## Goal

Show that the iPhone app can discover, connect to, and exchange real wallet
commands with the hardware wallet over BLE.

## Proof milestones

1. The firmware advertises a stable custom wallet BLE service UUID.
2. The iPhone app can scan and discover the wallet by service UUID.
3. The iPhone app can connect and subscribe to the wallet notify characteristic.
4. The app can round-trip `PING` and receive `PONG`.
5. The app can round-trip `GET_INFO`.
6. The app can round-trip `GET_PUBKEY`.
7. The app can unlock over BLE with `UNLOCK:PIN:<pin>`.
8. The app can sign a real message over BLE with `SIGN:<base64>`.

## First implementation target

The first implementation only needs to prove:

- advertising
- discovery
- connect
- notify subscription
- `PING`
- `GET_INFO`
- `GET_PUBKEY`

That is enough to prove the BLE transport is real before we move on to unlock
and signing.

## Transport shape

- one custom GATT service for the wallet session
- one write characteristic for app -> device command chunks
- one notify characteristic for device -> app response chunks
- newline-delimited command framing preserved from the current serial protocol

## Evidence we want to capture

- app log showing BLE scan result and successful connection
- app log showing `PING -> PONG`
- app log showing parsed `GET_INFO`
- app log showing parsed `GET_PUBKEY`
- optional screen capture from iPhone during the connection flow
