# ESP32 Signer Firmware Flow

## 1) Boot and Initialization

```mermaid
flowchart TD
  A[Power on / reset] --> B{Boot button held 10s?}
  B -- Yes --> C[Factory wipe NVS auth + key + OTP]
  C --> D[Send FACTORY_WIPE_DONE]
  B -- No --> E[Continue]
  D --> E
  E --> F[Load or generate Solana key]
  F --> G[Load auth state from NVS]
  G --> H[Main UART command loop]
```

## 2) First-Time Setup (Auth Mode)

```mermaid
flowchart TD
  A[AUTH_MODE=UNSET, FINALIZED=0] --> B{Host SET_MODE command}
  B -- SET_MODE:NONE --> C[Button hold confirm]
  C --> D[Clear PIN + OTP, finalize NONE]
  B -- SET_MODE:PIN:###### --> E[Validate 6-digit PIN]
  E --> F[Button hold confirm]
  F --> G[Store PIN with PBKDF2 hash + salt, finalize PIN]
  B -- SET_MODE:OTP_BEGIN / OTP_BEGIN --> H[Button hold confirm]
  H --> I[Generate OTP secret + return URI/secret]
  I --> J[Host scans QR and sends OTP confirm]
  J --> K{SET_MODE:OTP_CONFIRM / OTP_CONFIRM}
  K -- valid code --> L[Finalize OTP]
  K -- bad code --> M[Increment fail counter]
```

## 3) Session Unlock + Signing

```mermaid
flowchart TD
  A[Device connected] --> B[GET_INFO]
  B --> C{Mode}
  C -- NONE --> D[No unlock required]
  C -- PIN --> E[UNLOCK:PIN:######]
  C -- OTP --> F[UNLOCK:OTP:######]
  F --> P[Button press confirm]
  E --> G{Valid?}
  P --> G
  G -- Yes --> H[UNLOCKED_UNTIL = now + 420s]
  G -- No --> I[register_failure, lock at 10 attempts]
  H --> J[SIGN:base64_payload]
  J --> K[Button press confirm]
  K --> L[SIGNATURE:base64_sig]
  H --> M[EXPORT_PRIVKEY]
  M --> N[Button press confirm]
  N --> O[PRIVKEY:base58 keypair]
```

## 4) Lock, Recovery, and Time Sync

```mermaid
flowchart TD
  A[Failed unlock attempts] --> B{count >= 10}
  B -- Yes --> C[AUTH_LOCKED state]
  B -- No --> D[Retry allowed]

  C --> E{Command}
  E -- allowed --> F[GET_INFO / GET_PUBKEY / GET_TIME / SET_TIME:*]
  E -- blocked --> G[ERROR:AUTH_LOCKED]

  F --> H{SET_TIME in OTP/UNSET mode?}
  H -- No --> I[ERROR:TIME_NOT_ALLOWED]
  H -- Yes --> L[Set time directly]
```

## 5) Security Properties (Current)

- Signing always requires physical button press.
- PIN mode uses salted PBKDF2-HMAC-SHA256 only.
- Auth lockout after 10 failures is persisted in NVS.
- `SET_TIME` is restricted to `UNSET`/`OTP` modes and is host-driven (no button gate).
- `UNLOCK:OTP` requires physical button press confirmation (8s timeout).
- Factory wipe clears Solana key, auth state, PIN material, and OTP secrets.

## 6) Migration Note

- This firmware uses one PIN format only (PBKDF2). Devices that were on older SHA256-only PIN firmware should be factory-wiped once before use.
