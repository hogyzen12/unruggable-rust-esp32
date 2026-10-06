# Dev Device Firmware Spec (Draft v1)

Date: 2026-02-15
Scope: ESP32 Solana signer dev-device firmware

## 1. Goals

- Keep firmware small, readable, and easy to audit.
- Support blind signing (no display on hardware).
- Current dev-unit batch uses `PIN` setup only (6 digits).
- Legacy `NONE` / `OTP` states may still be readable for backwards compatibility, but new setup commands are disabled.
- Mode is chosen once at first setup and cannot be changed via protocol.
- Keep host protocol strict and minimal.

## 2. Non-Goals (For This Dev Device Batch)

- On-device transaction display or anti-phishing UI (no screen).
- OTA/remote firmware updates.
- Dynamic feature expansion in-field.

## 3. Security Model

- Host computer is not fully trusted.
- Physical button press is trusted user presence.
- Private signing key is generated on-device and never exported.
- Blind signing is an explicit accepted risk for this hardware phase.

## 4. Auth Modes and Rules

- Device starts in `UNSET` mode on first boot after provisioning/reset.
- User sets a 6-digit `PIN` during setup.
- After mode is finalized, protocol rejects any mode-change command with `ERROR:MODE_FINAL`.
- Switching auth mode is not supported.
- Only full physical factory wipe is allowed to re-enter `UNSET`.

## 5. Fixed Security Constants

- `PIN_DIGITS = 6`
- `MAX_AUTH_ATTEMPTS = 10` (persisted across reboots)
- `UNLOCK_WINDOW_SECS = 420` (7-minute session)
- `BUTTON_CONFIRM_MS = 2000` (2-second hold for setup/admin confirms)
- `OTP_UNLOCK_BUTTON_TIMEOUT_MS = 8000` (OTP unlock button-press timeout)
- `BUTTON_SIGN_TIMEOUT_MS = 30000`
- `MAX_CMD_LEN = 4028` bytes
- `MAX_SIGN_PAYLOAD_BYTES = 2048` bytes (decoded payload)
- `OTP_DIGITS = 6`
- `OTP_PERIOD = 30` seconds
- `OTP_WINDOW = +/-1 step`
- `SET_TIME` in `UNSET`/`OTP` modes does not require button confirmation.

## 6. Persistent State (NVS)

- `solana_key` -> 32-byte ed25519 secret seed
- `solana_pubkey` -> 32-byte ed25519 public key fingerprint derived from `solana_key`
- `auth_mode` -> `0=UNSET, 1=NONE, 2=PIN, 3=OTP`
- `auth_finalized` -> `0/1`
- `auth_fail_count` -> `u8`, saturating, persisted
- `auth_locked` -> `0/1` (set when attempts exhausted)
- `pin_salt` -> 16 bytes (PIN mode only)
- `pin_hash` -> 32 bytes (PIN mode only)
- `otp_secret` -> 20 bytes (OTP mode only)
- `otp_last` -> `u64` (OTP mode only)
- `otp_enrolled` -> `0/1` (OTP mode only)

Notes:
- Store only what is needed for the chosen mode.
- Freshly flashed blank units do not create a wallet on boot. Wallet creation is deferred until PIN setup completes.
- If `solana_key` is missing or no longer matches `solana_pubkey`, fail closed and require recovery instead of silently generating a new wallet identity.
- Zero temporary secrets in RAM after use where practical.

## 7. PIN Storage/Verification

- PIN must be exactly 6 ASCII digits.
- Store salted hash, not plaintext:
  - `pin_hash = PBKDF2-HMAC-SHA256(pin_ascii, pin_salt, 10000, 32 bytes)`
- Use constant-time compare for verification.
- On auth failure:
  - increment `auth_fail_count`
  - persist immediately
  - when `auth_fail_count >= MAX_AUTH_ATTEMPTS`, set `auth_locked=1`
- On successful unlock:
  - reset `auth_fail_count=0`

## 8. OTP Enrollment/Verification

- OTP mode setup is two-step:
  1. `SET_MODE:OTP_BEGIN`
  2. `SET_MODE:OTP_CONFIRM:<6digits>`
- `OTP_BEGIN` requires physical confirmation (2s button hold).
- `OTP_BEGIN` returns provisioning data for QR pairing in authenticator apps.
- `OTP_CONFIRM` uses device time only (no host-supplied unix timestamp).
- OTP verifies with replay protection via `otp_last`.
- On success:
  - set `auth_mode=OTP`
  - set `auth_finalized=1`
  - reset fail count

## 9. UART Protocol (Line-Based, Strict)

Format:
- ASCII commands terminated by `\n`
- Unknown/malformed commands return `ERROR:*`
- Reject commands longer than `MAX_CMD_LEN`

Response style (legacy-compatible):
- Success: plain payload (for example `PUBKEY:<base58>`, `SIGNATURE:<b64>`, `PRIVKEY:<base58>`, `MODE_SET:PIN`)
- Failure: `ERROR:<message_or_code>`

### 9.1 Required Commands

- `GET_INFO`
  - `INFO;VER=<semver>;AUTH_MODE=<UNSET|NONE|PIN|OTP>;FINALIZED=<0|1>;LOCKED=<0|1>;RETRIES_LEFT=<n>;KEY_STATE=<UNINITIALIZED|READY|FAULT>`

- `GET_PUBKEY`
  - `PUBKEY:<base58>`
  - Returns `ERROR:WALLET_NOT_INITIALIZED` on a blank unit before PIN setup completes.

- `SET_MODE:PIN:<6digits>`
  - Allowed only when `AUTH_MODE=UNSET`
  - Optional legacy suffix is ignored: `SET_MODE:PIN:<6digits>:<...>`
  - Requires 2s physical hold
  - `MODE_SET:PIN`
  - If the device is blank, this is the moment a brand-new wallet is created on-device.

- `SET_MODE:OTP_BEGIN`
  - Disabled for the current dev-unit batch
  - Returns `ERROR:PIN_REQUIRED`

- `SET_MODE:OTP_CONFIRM:<6digits>`
  - Allowed only when pending OTP enrollment and `AUTH_MODE=UNSET`
  - Uses device time only
  - `MODE_SET:OTP`

- `UNLOCK:PIN:<6digits>`
  - Valid only in PIN mode and when not locked
  - Optional legacy suffix is ignored: `UNLOCK:PIN:<6digits>:<...>`
  - `UNLOCKED_UNTIL:<unix>`

- `UNLOCK:OTP:<6digits>`
  - Valid only in OTP mode and when not locked
  - Requires button press confirmation (8s timeout)
  - `UNLOCKED_UNTIL:<unix>`

- `SIGN:<base64_payload>`
  - If mode is `PIN`/`OTP`, requires active unlock window
  - Requires button press confirmation (with timeout)
  - `SIGNATURE:<base64_signature>`

- `EXPORT_PRIVKEY` / `SHOW_PRIVKEY`
  - Valid only in `PIN` mode after a successful `UNLOCK:PIN`
  - Requires a fresh physical button press, like signing
  - Returns `PRIVKEY:<base58>`
  - `PRIVKEY` is base58-encoded 64-byte Solana keypair material: `secret32 || pubkey32`
  - Returns `ERROR:WALLET_NOT_INITIALIZED` on a blank unit before PIN setup completes

- `SHUTDOWN`
  - Requires 2s physical hold
  - `SHUTDOWN_OK`

Legacy aliases supported:
- `OTP_BEGIN` -> same behavior as `SET_MODE:OTP_BEGIN`
- `OTP_CONFIRM:<code>[:unix]` -> same behavior as `SET_MODE:OTP_CONFIRM:<code>` (unix ignored)
- `OTP_UNLOCK:<code>[:unix]` -> same behavior as `UNLOCK:OTP:<code>` (unix ignored)
- `PIN_UNLOCK:<pin>[:unix]` -> same behavior as `UNLOCK:PIN:<pin>` (suffix ignored)

### 9.2 Error Codes

- `ERROR:MODE_FINAL`
- `ERROR:MODE_UNSET`
- `ERROR:AUTH_MODE_MISMATCH`
- `ERROR:AUTH_FAILED`
- `ERROR:AUTH_LOCKED`
- `ERROR:BAD_PIN_FORMAT`
- `ERROR:BAD_OTP_FORMAT`
- `ERROR:OTP_NOT_PENDING`
- `ERROR:OTP_BAD_CODE`
- `ERROR:PAYLOAD_TOO_LARGE`
- `ERROR:BUTTON_TIMEOUT`
- `ERROR:LOCKED`
- `ERROR:KEYSTORE_CORRUPT`
- `ERROR:WALLET_NOT_INITIALIZED`
- `ERROR:PIN_REQUIRED`

Legacy error strings retained for older hosts:
- `ERROR:Invalid base64 encoding`
- `ERROR:Unknown command`
- `ERROR:Command too long`

## 10. State Machine

- `BOOT`
  - load/generate signer key
  - load auth state
  - transition to `READY`

- `READY`
  - accepts info/setup/unlock/sign/admin commands

- `UNLOCKED` (time-limited session)
  - only affects sign authorization for PIN/OTP modes
  - auto-relock when `now > unlocked_until`

- `LOCKED_OUT`
  - entered when `auth_fail_count >= MAX_AUTH_ATTEMPTS`
  - only non-sensitive commands allowed (`GET_INFO`, `GET_PUBKEY`, `GET_TIME`, `SET_TIME:*` with mode checks)

- `DEEP_SLEEP`
  - entered via `SHUTDOWN`

## 11. Physical Recovery / Factory Wipe

- No protocol command may change auth mode once finalized.
- Recovery path is physical-only:
  - On boot, hold button >=10s to trigger factory wipe.
  - Wipe NVS namespace for auth + key material (`solana_key` included).
  - Return to `UNSET`.
- Consequence of factory wipe:
  - Existing signing key is destroyed.
  - Device will generate a new keypair on next boot.
  - Public key/wallet identity changes.

## 12. Clean-Code Implementation Plan (No Bloat)

Target module split:
- `main.rs` -> wiring + main loop only
- `protocol.rs` -> parse/serialize command/response enums
- `auth.rs` -> auth mode logic, unlock checks, fail counters
- `twofa.rs` -> OTP internals only
- `storage.rs` -> NVS read/write helpers, typed keys
- `signer.rs` -> key load/generate + signing
- `ui.rs` -> LED/button patterns and hold/timeouts

Rules:
- Prefer fixed-size buffers over unbounded `String` growth.
- Keep each command handler small and side-effect explicit.
- Use comments for rationale, not line-by-line narration.
- Keep `unsafe` isolated and wrapped in safe helper functions.
- Return explicit errors; avoid silent fallthrough behavior.

## 13. Acceptance Criteria

- Freshly flashed blank units report `KEY_STATE=UNINITIALIZED` until PIN setup completes.
- Device generates a new wallet only when PIN setup completes on a blank unit.
- PIN is exactly 6 digits; lockout occurs after 10 failed attempts across reboots.
- Private-key backup export requires both a valid PIN unlock window and a fresh physical button press.
- Signing requires physical button confirmation and times out cleanly.
- Oversized/malformed input is rejected safely without memory growth.
- Success responses are plain payloads (for example `PUBKEY:*`, `SIGNATURE:*`, `MODE_SET:*`) and failures use `ERROR:*`.
- Firmware compiles in release mode with no warnings in active code paths.
