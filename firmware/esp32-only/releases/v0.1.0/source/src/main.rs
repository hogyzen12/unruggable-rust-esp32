use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::hal::uart::UartDriver;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys::ESP_ERR_TIMEOUT;
use esp_idf_sys::{esp_deep_sleep_start, rtc_gpio_isolate};
use pbkdf2::pbkdf2_hmac_array;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use subtle::ConstantTimeEq;

mod twofa;

const FW_VER: &str = env!("CARGO_PKG_VERSION");

const MAX_CMD_LEN: usize = 4028;
const MAX_SIGN_PAYLOAD_BYTES: usize = 2048;
const MAX_AUTH_ATTEMPTS: u8 = 10;
const UNLOCK_WINDOW_SECS: u64 = 420; // 7 minutes session unlock

const BUTTON_POLL_MS: u32 = 50;
const BUTTON_CONFIRM_MS: u32 = 2000;
const BUTTON_ACTION_TIMEOUT_MS: u32 = 30_000;
const OTP_UNLOCK_BUTTON_TIMEOUT_MS: u32 = 8_000;
const FACTORY_WIPE_HOLD_MS: u32 = 10_000;

const KEY_SOLANA: &str = "solana_key";
const KEY_SOLANA_PUBKEY: &str = "solana_pubkey";
const KEY_AUTH_MODE: &str = "auth_mode";
const KEY_AUTH_FINALIZED: &str = "auth_finalized";
const KEY_AUTH_FAIL_COUNT: &str = "auth_fail_count";
const KEY_AUTH_LOCKED: &str = "auth_locked";
const KEY_PIN_SALT: &str = "pin_salt";
const KEY_PIN_HASH: &str = "pin_hash";
// Keep PIN verification responsive on ESP32 so unlock completes within typical
// host app timeouts and does not trip task watchdog.
const PIN_KDF_ITERS: u32 = 10_000;
const ALLOW_NONE_MODE_SETUP: bool = false;
const ALLOW_OTP_MODE_SETUP: bool = false;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum AuthMode {
    Unset,
    None,
    Pin,
    Otp,
}

impl AuthMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::None,
            2 => Self::Pin,
            3 => Self::Otp,
            _ => Self::Unset,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Unset => 0,
            Self::None => 1,
            Self::Pin => 2,
            Self::Otp => 3,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "UNSET",
            Self::None => "NONE",
            Self::Pin => "PIN",
            Self::Otp => "OTP",
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum KeyState {
    Uninitialized,
    Ready,
    Fault,
}

impl KeyState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Uninitialized => "UNINITIALIZED",
            Self::Ready => "READY",
            Self::Fault => "FAULT",
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct AuthState {
    mode: AuthMode,
    finalized: bool,
    fail_count: u8,
    locked: bool,
}

impl AuthState {
    fn retries_left(self) -> u8 {
        MAX_AUTH_ATTEMPTS.saturating_sub(self.fail_count)
    }
}

fn nvs_set_u8(nvs: &mut EspNvs<NvsDefault>, key: &str, value: u8) -> anyhow::Result<()> {
    nvs.set_raw(key, &[value])?;
    Ok(())
}

fn nvs_get_u8(nvs: &mut EspNvs<NvsDefault>, key: &str) -> anyhow::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    match nvs.get_raw(key, &mut buf)? {
        Some(v) if v.len() == 1 => Ok(Some(buf[0])),
        _ => Ok(None),
    }
}

fn nvs_set_array<const N: usize>(
    nvs: &mut EspNvs<NvsDefault>,
    key: &str,
    value: &[u8; N],
) -> anyhow::Result<()> {
    nvs.set_raw(key, value)?;
    Ok(())
}

fn nvs_get_array<const N: usize>(
    nvs: &mut EspNvs<NvsDefault>,
    key: &str,
) -> anyhow::Result<Option<[u8; N]>> {
    let mut buf = [0u8; N];
    match nvs.get_raw(key, &mut buf)? {
        Some(v) if v.len() == N => Ok(Some(buf)),
        _ => Ok(None),
    }
}

fn load_auth_state(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<AuthState> {
    let mode = AuthMode::from_u8(nvs_get_u8(nvs, KEY_AUTH_MODE)?.unwrap_or(0));
    let finalized = nvs_get_u8(nvs, KEY_AUTH_FINALIZED)?.unwrap_or(0) == 1;
    let fail_count = nvs_get_u8(nvs, KEY_AUTH_FAIL_COUNT)?
        .unwrap_or(0)
        .min(MAX_AUTH_ATTEMPTS);
    let locked =
        nvs_get_u8(nvs, KEY_AUTH_LOCKED)?.unwrap_or(0) == 1 || fail_count >= MAX_AUTH_ATTEMPTS;

    Ok(AuthState {
        mode,
        finalized,
        fail_count,
        locked,
    })
}

fn clear_failures(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<()> {
    nvs_set_u8(nvs, KEY_AUTH_FAIL_COUNT, 0)?;
    nvs_set_u8(nvs, KEY_AUTH_LOCKED, 0)?;
    Ok(())
}

fn register_failure(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<bool> {
    let mut count = nvs_get_u8(nvs, KEY_AUTH_FAIL_COUNT)?.unwrap_or(0);
    count = count.saturating_add(1);
    nvs_set_u8(nvs, KEY_AUTH_FAIL_COUNT, count)?;
    if count >= MAX_AUTH_ATTEMPTS {
        nvs_set_u8(nvs, KEY_AUTH_LOCKED, 1)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn set_mode_finalized(nvs: &mut EspNvs<NvsDefault>, mode: AuthMode) -> anyhow::Result<()> {
    nvs_set_u8(nvs, KEY_AUTH_MODE, mode.as_u8())?;
    nvs_set_u8(nvs, KEY_AUTH_FINALIZED, 1)?;
    clear_failures(nvs)?;
    Ok(())
}

fn has_initialized_state(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<bool> {
    let state = load_auth_state(nvs)?;
    Ok(
        nvs_get_array::<32>(nvs, KEY_SOLANA_PUBKEY)?.is_some()
            || state.mode != AuthMode::Unset
            || state.finalized
            || state.fail_count != 0
            || state.locked
            || nvs_get_array::<16>(nvs, KEY_PIN_SALT)?.is_some()
            || nvs_get_array::<32>(nvs, KEY_PIN_HASH)?.is_some()
            || nvs_get_array::<{ twofa::OTP_BYTES }>(nvs, "otp_secret")?.is_some(),
    )
}

fn load_existing_key(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<Option<SigningKey>> {
    let key_bytes = nvs_get_array::<32>(nvs, KEY_SOLANA)?;
    let stored_pubkey = nvs_get_array::<32>(nvs, KEY_SOLANA_PUBKEY)?;

    match key_bytes {
        Some(key_bytes) => {
            let signing_key = SigningKey::from_bytes(&key_bytes);
            let derived_pubkey = signing_key.verifying_key().to_bytes();

            if let Some(expected_pubkey) = stored_pubkey {
                if !bool::from(derived_pubkey.ct_eq(&expected_pubkey)) {
                    anyhow::bail!("stored signer key does not match persisted pubkey");
                }
            } else {
                // Backfill pubkey metadata for units that were initialized before
                // keystore consistency checks were added.
                nvs_set_array(nvs, KEY_SOLANA_PUBKEY, &derived_pubkey)?;
            }

            Ok(Some(signing_key))
        }
        None => {
            if stored_pubkey.is_some() || has_initialized_state(nvs)? {
                anyhow::bail!("signer key missing from initialized device");
            }
            Ok(None)
        }
    }
}

fn generate_and_store_key(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<SigningKey> {
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let derived_pubkey = signing_key.verifying_key().to_bytes();
    nvs.set_raw(KEY_SOLANA, &signing_key.to_bytes())?;
    nvs_set_array(nvs, KEY_SOLANA_PUBKEY, &derived_pubkey)?;
    Ok(signing_key)
}

fn key_runtime(signing_key: SigningKey) -> (Option<SigningKey>, Option<String>, KeyState) {
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let pubkey_base58 = bs58::encode(verifying_key.to_bytes()).into_string();
    (Some(signing_key), Some(pubkey_base58), KeyState::Ready)
}

fn export_keypair_base58(signing_key: &SigningKey) -> String {
    let mut keypair = [0u8; 64];
    keypair[..32].copy_from_slice(&signing_key.to_bytes());
    keypair[32..].copy_from_slice(&signing_key.verifying_key().to_bytes());
    bs58::encode(keypair).into_string()
}

fn derive_pin_hash(pin: &str, salt: &[u8; 16]) -> [u8; 32] {
    pbkdf2_hmac_array::<Sha256, 32>(pin.as_bytes(), salt, PIN_KDF_ITERS)
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == 6 && pin.chars().all(|c| c.is_ascii_digit())
}

fn store_pin(nvs: &mut EspNvs<NvsDefault>, pin: &str) -> anyhow::Result<()> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let pin_hash = derive_pin_hash(pin, &salt);
    nvs_set_array(nvs, KEY_PIN_SALT, &salt)?;
    nvs_set_array(nvs, KEY_PIN_HASH, &pin_hash)?;
    Ok(())
}

fn verify_pin(nvs: &mut EspNvs<NvsDefault>, pin: &str) -> anyhow::Result<bool> {
    let salt = match nvs_get_array::<16>(nvs, KEY_PIN_SALT)? {
        Some(v) => v,
        None => return Ok(false),
    };
    let stored = match nvs_get_array::<32>(nvs, KEY_PIN_HASH)? {
        Some(v) => v,
        None => return Ok(false),
    };
    let calc = derive_pin_hash(pin, &salt);
    Ok(calc.ct_eq(&stored).into())
}

fn clear_pin(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<()> {
    let _ = nvs.remove(KEY_PIN_SALT)?;
    let _ = nvs.remove(KEY_PIN_HASH)?;
    Ok(())
}

fn factory_wipe(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<()> {
    for key in [
        KEY_SOLANA,
        KEY_SOLANA_PUBKEY,
        KEY_AUTH_MODE,
        KEY_AUTH_FINALIZED,
        KEY_AUTH_FAIL_COUNT,
        KEY_AUTH_LOCKED,
        KEY_PIN_SALT,
        KEY_PIN_HASH,
    ] {
        let _ = nvs.remove(key)?;
    }
    twofa::TwoFa::clear(nvs)?;
    Ok(())
}

fn has_pending_otp(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<bool> {
    let secret_exists = nvs_get_array::<{ twofa::OTP_BYTES }>(nvs, "otp_secret")?.is_some();
    let enrolled = twofa::TwoFa::is_enrolled(nvs)?;
    Ok(secret_exists && !enrolled)
}

fn wait_for_button_hold_with_timeout(
    timeout_ms: u32,
    hold_ms: u32,
    poll_ms: u32,
    mut is_low: impl FnMut() -> bool,
) -> bool {
    let mut waited = 0u32;
    let mut held = 0u32;

    while waited < timeout_ms {
        if is_low() {
            held = held.saturating_add(poll_ms);
            if held >= hold_ms {
                return true;
            }
        } else {
            held = 0;
        }

        esp_idf_svc::hal::delay::FreeRtos::delay_ms(poll_ms);
        waited = waited.saturating_add(poll_ms);
    }
    false
}

fn wait_for_button_press(timeout_ms: u32, poll_ms: u32, mut is_low: impl FnMut() -> bool) -> bool {
    let mut waited = 0u32;
    while waited < timeout_ms {
        if is_low() {
            return true;
        }
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(poll_ms);
        waited = waited.saturating_add(poll_ms);
    }
    false
}

fn send_line(uart: &mut UartDriver, line: &str) -> anyhow::Result<()> {
    let mut response = String::with_capacity(line.len() + 1);
    response.push_str(line);
    response.push('\n');
    let bytes = response.as_bytes();
    let mut written = 0usize;
    while written < bytes.len() {
        written += uart.write(&bytes[written..])?;
    }
    Ok(())
}

fn send_ok(uart: &mut UartDriver, data: &str) -> anyhow::Result<()> {
    send_line(uart, data)
}

fn send_error(uart: &mut UartDriver, code: &str) -> anyhow::Result<()> {
    send_line(uart, &format!("ERROR:{code}"))
}

fn command_allowed_when_locked(cmd: &str) -> bool {
    matches!(cmd, "GET_INFO" | "GET_PUBKEY" | "GET_TIME")
        || cmd.starts_with("SET_TIME:")
}

fn main() -> anyhow::Result<()> {
    let peripherals = Peripherals::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take()?;
    let mut nvs = EspNvs::new(nvs_partition, "solana_signer", true)?;

    let mut uart = UartDriver::new(
        peripherals.uart0,
        peripherals.pins.gpio1,
        peripherals.pins.gpio3,
        Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,
        Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,
        &Default::default(),
    )?;

    let mut button = PinDriver::input(peripherals.pins.gpio0)?;
    button.set_pull(Pull::Up)?;

    let mut led = PinDriver::output(peripherals.pins.gpio2)?;
    led.set_low()?;

    // Physical-only recovery path.
    if button.is_low()
        && wait_for_button_hold_with_timeout(
            FACTORY_WIPE_HOLD_MS,
            FACTORY_WIPE_HOLD_MS,
            BUTTON_POLL_MS,
            || button.is_low(),
        )
    {
        factory_wipe(&mut nvs)?;
        send_ok(&mut uart, "FACTORY_WIPE_DONE")?;
        for _ in 0..3 {
            led.set_high()?;
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
            led.set_low()?;
            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
        }
    }

    let (mut signing_key, mut pubkey_base58, mut key_state) = match load_existing_key(&mut nvs) {
        Ok(Some(signing_key)) => key_runtime(signing_key),
        Ok(None) => (None, None, KeyState::Uninitialized),
        Err(_) => (None, None, KeyState::Fault),
    };

    led.set_high()?;
    esp_idf_svc::hal::delay::FreeRtos::delay_ms(200);
    led.set_low()?;

    let mut unlocked_until: u64 = 0;

    let mut buffer = String::new();
    let mut line_too_long = false;

    loop {
        let mut byte = [0u8; 1];
        match uart.read(&mut byte, 1000) {
            Ok(1) => {
                let ch = byte[0] as char;
                if ch == '\r' {
                    continue;
                }

                if ch == '\n' {
                    if line_too_long {
                        send_error(&mut uart, "Command too long")?;
                        buffer.clear();
                        line_too_long = false;
                        continue;
                    }

                    let input = buffer.trim();
                    if input.is_empty() {
                        buffer.clear();
                        continue;
                    }

                    if key_state == KeyState::Fault
                        && !matches!(input, "GET_INFO" | "GET_TIME" | "SHUTDOWN")
                    {
                        send_error(&mut uart, "KEYSTORE_CORRUPT")?;
                        buffer.clear();
                        continue;
                    }

                    let state = load_auth_state(&mut nvs)?;
                    if state.locked && !command_allowed_when_locked(input) {
                        send_error(&mut uart, "AUTH_LOCKED")?;
                        buffer.clear();
                        continue;
                    }

                    if input == "GET_INFO" {
                        let info = format!(
                            "INFO;VER={};AUTH_MODE={};FINALIZED={};LOCKED={};RETRIES_LEFT={};KEY_STATE={}",
                            FW_VER,
                            state.mode.as_str(),
                            if state.finalized { 1 } else { 0 },
                            if state.locked { 1 } else { 0 },
                            state.retries_left(),
                            key_state.as_str()
                        );
                        send_ok(&mut uart, &info)?;
                    } else if input == "GET_TIME" {
                        let now = twofa::TwoFa::device_unix_time();
                        send_ok(&mut uart, &format!("TIME:{now}"))?;
                    } else if let Some(raw_unix) = input.strip_prefix("SET_TIME:") {
                        let unix_secs = match raw_unix.parse::<u64>() {
                            Ok(v) => v,
                            Err(_) => {
                                send_error(&mut uart, "BAD_TIME_FORMAT")?;
                                buffer.clear();
                                continue;
                            }
                        };

                        if unix_secs < twofa::MIN_VALID_UNIX_TIME {
                            send_error(&mut uart, "BAD_TIME_VALUE")?;
                            buffer.clear();
                            continue;
                        }

                        if !matches!(state.mode, AuthMode::Unset | AuthMode::Otp) {
                            send_error(&mut uart, "TIME_NOT_ALLOWED")?;
                            buffer.clear();
                            continue;
                        }

                        match twofa::TwoFa::set_device_unix_time(unix_secs) {
                            Ok(()) => send_ok(&mut uart, &format!("TIME_SET:{unix_secs}"))?,
                            Err(_) => send_error(&mut uart, "TIME_SET_FAILED")?,
                        }
                    } else if input == "GET_PUBKEY" {
                        if let Some(pubkey_base58) = pubkey_base58.as_ref() {
                            send_ok(&mut uart, &format!("PUBKEY:{pubkey_base58}"))?;
                        } else if key_state == KeyState::Uninitialized {
                            send_error(&mut uart, "WALLET_NOT_INITIALIZED")?;
                        } else {
                            send_error(&mut uart, "KEYSTORE_CORRUPT")?;
                        }
                    } else if input == "SET_MODE:NONE" {
                        if state.finalized {
                            send_error(&mut uart, "MODE_FINAL")?;
                        } else if !ALLOW_NONE_MODE_SETUP {
                            send_error(&mut uart, "PIN_REQUIRED")?;
                        } else {
                            led.set_high()?;
                            let confirmed = wait_for_button_hold_with_timeout(
                                BUTTON_ACTION_TIMEOUT_MS,
                                BUTTON_CONFIRM_MS,
                                BUTTON_POLL_MS,
                                || button.is_low(),
                            );
                            led.set_low()?;

                            if !confirmed {
                                send_error(&mut uart, "BUTTON_TIMEOUT")?;
                            } else {
                                if key_state == KeyState::Uninitialized {
                                    let new_key = generate_and_store_key(&mut nvs)?;
                                    let runtime = key_runtime(new_key);
                                    signing_key = runtime.0;
                                    pubkey_base58 = runtime.1;
                                    key_state = runtime.2;
                                }
                                clear_pin(&mut nvs)?;
                                twofa::TwoFa::clear(&mut nvs)?;
                                set_mode_finalized(&mut nvs, AuthMode::None)?;
                                unlocked_until = 0;
                                send_ok(&mut uart, "MODE_SET:NONE")?;
                            }
                        }
                    } else if let Some(pin_raw) = input.strip_prefix("SET_MODE:PIN:") {
                        let pin = pin_raw.split(':').next().unwrap_or("");
                        if state.finalized {
                            send_error(&mut uart, "MODE_FINAL")?;
                        } else if !valid_pin(pin) {
                            send_error(&mut uart, "BAD_PIN_FORMAT")?;
                        } else {
                            led.set_high()?;
                            let confirmed = wait_for_button_hold_with_timeout(
                                BUTTON_ACTION_TIMEOUT_MS,
                                BUTTON_CONFIRM_MS,
                                BUTTON_POLL_MS,
                                || button.is_low(),
                            );
                            led.set_low()?;
                            if !confirmed {
                                send_error(&mut uart, "BUTTON_TIMEOUT")?;
                            } else {
                                if key_state == KeyState::Uninitialized {
                                    let new_key = generate_and_store_key(&mut nvs)?;
                                    let runtime = key_runtime(new_key);
                                    signing_key = runtime.0;
                                    pubkey_base58 = runtime.1;
                                    key_state = runtime.2;
                                }
                                store_pin(&mut nvs, pin)?;
                                twofa::TwoFa::clear(&mut nvs)?;
                                set_mode_finalized(&mut nvs, AuthMode::Pin)?;
                                unlocked_until = 0;
                                send_ok(&mut uart, "MODE_SET:PIN")?;
                            }
                        }
                    } else if input == "SET_MODE:OTP_BEGIN" || input == "OTP_BEGIN" {
                        if state.finalized {
                            send_error(&mut uart, "MODE_FINAL")?;
                        } else if !ALLOW_OTP_MODE_SETUP {
                            send_error(&mut uart, "PIN_REQUIRED")?;
                        } else {
                            led.set_high()?;
                            let confirmed = wait_for_button_hold_with_timeout(
                                BUTTON_ACTION_TIMEOUT_MS,
                                BUTTON_CONFIRM_MS,
                                BUTTON_POLL_MS,
                                || button.is_low(),
                            );
                            led.set_low()?;
                            if !confirmed {
                                send_error(&mut uart, "BUTTON_TIMEOUT")?;
                            } else {
                                clear_pin(&mut nvs)?;
                                let secret = twofa::TwoFa::begin(&mut nvs)?;
                                nvs_set_u8(&mut nvs, KEY_AUTH_MODE, AuthMode::Unset.as_u8())?;
                                nvs_set_u8(&mut nvs, KEY_AUTH_FINALIZED, 0)?;
                                let pubkey_base58 = pubkey_base58
                                    .as_deref()
                                    .ok_or_else(|| anyhow::anyhow!("keystore fault"))?;
                                let uri = format!(
                                    "otpauth://totp/Unruggable:{}?secret={}&issuer=Unruggable&algorithm=SHA1&digits={}&period={}",
                                    pubkey_base58,
                                    secret,
                                    twofa::OTP_DIGITS,
                                    twofa::OTP_PERIOD
                                );
                                let payload = format!(
                                    "OTP_SECRET:{};URI={};ALGO=SHA1;DIGITS={};PERIOD={}",
                                    secret,
                                    uri,
                                    twofa::OTP_DIGITS,
                                    twofa::OTP_PERIOD
                                );
                                send_ok(&mut uart, &payload)?;
                            }
                        }
                    } else if let Some(code_raw) = input
                        .strip_prefix("SET_MODE:OTP_CONFIRM:")
                        .or_else(|| input.strip_prefix("OTP_CONFIRM:"))
                    {
                        let code = code_raw.split(':').next().unwrap_or("");
                        let is_legacy = input.starts_with("OTP_CONFIRM:");
                        if state.finalized {
                            send_error(&mut uart, "MODE_FINAL")?;
                        } else if !ALLOW_OTP_MODE_SETUP {
                            send_error(&mut uart, "PIN_REQUIRED")?;
                        } else if !has_pending_otp(&mut nvs)? {
                            send_error(&mut uart, "OTP_NOT_PENDING")?;
                        } else if code.len() != twofa::OTP_DIGITS as usize
                            || !code.chars().all(|c| c.is_ascii_digit())
                        {
                            send_error(&mut uart, "BAD_OTP_FORMAT")?;
                        } else {
                            match twofa::TwoFa::confirm(&mut nvs, code) {
                                Ok(()) => {
                                    set_mode_finalized(&mut nvs, AuthMode::Otp)?;
                                    unlocked_until = 0;
                                    if is_legacy {
                                        send_ok(&mut uart, "OTP_CONFIRMED")?;
                                    } else {
                                        send_ok(&mut uart, "MODE_SET:OTP")?;
                                    }
                                }
                                Err(err) => {
                                    if err.to_string().contains("time not set") {
                                        send_error(&mut uart, "TIME_NOT_SET")?;
                                    } else if register_failure(&mut nvs)? {
                                        send_error(&mut uart, "AUTH_LOCKED")?;
                                    } else {
                                        send_error(&mut uart, "OTP_BAD_CODE")?;
                                    }
                                }
                            }
                        }
                    } else if let Some(pin_raw) = input
                        .strip_prefix("UNLOCK:PIN:")
                        .or_else(|| input.strip_prefix("PIN_UNLOCK:"))
                    {
                        let pin = pin_raw.split(':').next().unwrap_or("");
                        if !state.finalized || state.mode == AuthMode::Unset {
                            send_error(&mut uart, "MODE_UNSET")?;
                        } else if state.mode != AuthMode::Pin {
                            send_error(&mut uart, "AUTH_MODE_MISMATCH")?;
                        } else if !valid_pin(pin) {
                            send_error(&mut uart, "BAD_PIN_FORMAT")?;
                        } else if verify_pin(&mut nvs, pin)? {
                            clear_failures(&mut nvs)?;
                            unlocked_until = twofa::TwoFa::device_unix_time() + UNLOCK_WINDOW_SECS;
                            send_ok(&mut uart, &format!("UNLOCKED_UNTIL:{unlocked_until}"))?;
                        } else if register_failure(&mut nvs)? {
                            send_error(&mut uart, "AUTH_LOCKED")?;
                        } else {
                            send_error(&mut uart, "AUTH_FAILED")?;
                        }
                    } else if let Some(code_raw) = input
                        .strip_prefix("UNLOCK:OTP:")
                        .or_else(|| input.strip_prefix("OTP_UNLOCK:"))
                    {
                        let code = code_raw.split(':').next().unwrap_or("");
                        if !state.finalized || state.mode == AuthMode::Unset {
                            send_error(&mut uart, "MODE_UNSET")?;
                        } else if state.mode != AuthMode::Otp {
                            send_error(&mut uart, "AUTH_MODE_MISMATCH")?;
                        } else if code.len() != twofa::OTP_DIGITS as usize
                            || !code.chars().all(|c| c.is_ascii_digit())
                        {
                            send_error(&mut uart, "BAD_OTP_FORMAT")?;
                        } else {
                            led.set_high()?;
                            let confirmed = wait_for_button_press(
                                OTP_UNLOCK_BUTTON_TIMEOUT_MS,
                                BUTTON_POLL_MS,
                                || button.is_low(),
                            );
                            led.set_low()?;
                            if !confirmed {
                                send_error(&mut uart, "BUTTON_TIMEOUT")?;
                                buffer.clear();
                                continue;
                            }

                            match twofa::TwoFa::unlock(&mut nvs, code, UNLOCK_WINDOW_SECS) {
                                Ok(until) => {
                                    clear_failures(&mut nvs)?;
                                    unlocked_until = until;
                                    send_ok(
                                        &mut uart,
                                        &format!("UNLOCKED_UNTIL:{unlocked_until}"),
                                    )?;
                                }
                                Err(err) => {
                                    if err.to_string().contains("time not set") {
                                        send_error(&mut uart, "TIME_NOT_SET")?;
                                    } else if register_failure(&mut nvs)? {
                                        send_error(&mut uart, "AUTH_LOCKED")?;
                                    } else {
                                        send_error(&mut uart, "AUTH_FAILED")?;
                                    }
                                }
                            }
                        }
                    } else if let Some(payload_b64) = input.strip_prefix("SIGN:") {
                        if !state.finalized || state.mode == AuthMode::Unset {
                            send_error(&mut uart, "MODE_UNSET")?;
                            buffer.clear();
                            continue;
                        }

                        if matches!(state.mode, AuthMode::Pin | AuthMode::Otp) {
                            let now = twofa::TwoFa::device_unix_time();
                            if now > unlocked_until {
                                send_error(&mut uart, "LOCKED")?;
                                buffer.clear();
                                continue;
                            }
                        }

                        let message_bytes =
                            match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
                                Ok(v) => v,
                                Err(_) => {
                                    send_error(&mut uart, "Invalid base64 encoding")?;
                                    buffer.clear();
                                    continue;
                                }
                            };

                        if message_bytes.len() > MAX_SIGN_PAYLOAD_BYTES {
                            send_error(&mut uart, "PAYLOAD_TOO_LARGE")?;
                            buffer.clear();
                            continue;
                        }

                        led.set_high()?;
                        let confirmed =
                            wait_for_button_press(BUTTON_ACTION_TIMEOUT_MS, BUTTON_POLL_MS, || {
                                button.is_low()
                            });
                        led.set_low()?;
                        if !confirmed {
                            send_error(&mut uart, "BUTTON_TIMEOUT")?;
                            buffer.clear();
                            continue;
                        }

                        let signature = signing_key
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("keystore fault"))?
                            .sign(&message_bytes);
                        let encoded_sig =
                            base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
                        send_ok(&mut uart, &format!("SIGNATURE:{encoded_sig}"))?;
                    } else if matches!(input, "EXPORT_PRIVKEY" | "SHOW_PRIVKEY") {
                        if !state.finalized || state.mode == AuthMode::Unset {
                            send_error(&mut uart, "MODE_UNSET")?;
                            buffer.clear();
                            continue;
                        }
                        if state.mode != AuthMode::Pin {
                            send_error(&mut uart, "PIN_REQUIRED")?;
                            buffer.clear();
                            continue;
                        }
                        if key_state == KeyState::Uninitialized {
                            send_error(&mut uart, "WALLET_NOT_INITIALIZED")?;
                            buffer.clear();
                            continue;
                        }

                        let now = twofa::TwoFa::device_unix_time();
                        if now > unlocked_until {
                            send_error(&mut uart, "LOCKED")?;
                            buffer.clear();
                            continue;
                        }

                        led.set_high()?;
                        let confirmed =
                            wait_for_button_press(BUTTON_ACTION_TIMEOUT_MS, BUTTON_POLL_MS, || {
                                button.is_low()
                            });
                        led.set_low()?;
                        if !confirmed {
                            send_error(&mut uart, "BUTTON_TIMEOUT")?;
                            buffer.clear();
                            continue;
                        }

                        let exported = export_keypair_base58(
                            signing_key
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("keystore fault"))?,
                        );
                        send_ok(&mut uart, &format!("PRIVKEY:{exported}"))?;
                    } else if input == "SHUTDOWN" {
                        led.set_high()?;
                        let confirmed = wait_for_button_hold_with_timeout(
                            BUTTON_ACTION_TIMEOUT_MS,
                            BUTTON_CONFIRM_MS,
                            BUTTON_POLL_MS,
                            || button.is_low(),
                        );
                        led.set_low()?;
                        if !confirmed {
                            send_error(&mut uart, "BUTTON_TIMEOUT")?;
                        } else {
                            unsafe {
                                rtc_gpio_isolate(0);
                                rtc_gpio_isolate(2);
                            }
                            send_ok(&mut uart, "SHUTDOWN_OK")?;
                            unsafe {
                                esp_deep_sleep_start();
                            }
                        }
                    } else {
                        send_error(&mut uart, "Unknown command")?;
                    }

                    buffer.clear();
                } else if !line_too_long {
                    if buffer.len() < MAX_CMD_LEN {
                        buffer.push(ch);
                    } else {
                        line_too_long = true;
                    }
                }
            }
            Ok(0) => {}
            Ok(n) => unreachable!("Unexpected read size: {n}"),
            Err(e) => {
                if e.code() != ESP_ERR_TIMEOUT {
                    let _ = send_error(&mut uart, "UART_READ");
                }
            }
        }
    }
}
