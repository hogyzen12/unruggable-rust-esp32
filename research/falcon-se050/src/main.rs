use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use base64::Engine;
use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey};
use embedded_graphics::pixelcolor::Rgb565;
use esp_idf_hal::task::watchdog::TWDTConfig;
use esp_idf_svc::hal::{
    delay::{FreeRtos, TickType},
    gpio::{AnyIOPin, Input, Output, PinDriver, Pull},
    i2c::{I2cConfig, I2cDriver},
    prelude::{FromValueType, Peripherals},
    usb_serial::{UsbSerialConfig, UsbSerialDriver},
};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use pbkdf2::pbkdf2_hmac;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

mod board;
mod display;
mod falcon;
mod se050;
mod transaction_verify;

use board::active_board_config;
use display::ConnectionState;
use transaction_verify::{TransactionParser, VerificationResults};

const FW_VER: &str = env!("CARGO_PKG_VERSION");

const SE050_I2C_ADDR: u8 = 0x48;
const SE050_KEY_ID: u32 = 0x0010_0001;

const AUTH_UNSET: u8 = 0;
const AUTH_NONE: u8 = 1;
const AUTH_PIN: u8 = 2;
const AUTH_OTP: u8 = 3;
const AUTH_BOTH: u8 = 4; // legacy current-firmware value, coerced to PIN

const MAX_AUTH_ATTEMPTS: u8 = 10;
const MAX_SIGN_PAYLOAD_BYTES: usize = 2048;
const MAX_CMD_LEN: usize = 4028;
const UART_POLL_MS: u64 = 50;
const UNLOCK_WINDOW_SECS: u64 = 420;
const PIN_SALT_BYTES: usize = 16;
const PIN_HASH_BYTES: usize = 32;
const PIN_HASH_ITERS: u32 = 10_000;
const MIN_VALID_UNIX_TIME: u64 = 1_577_836_800; // 2020-01-01
const SESSION_KEY_BYTES: usize = 32;
const SE050_PUBKEY_BYTES: usize = 32;
const SE050_SIG_BYTES: usize = 64;
const PQC_ENC_SALT_BYTES: usize = 16;
const PQC_ENC_NONCE_BYTES: usize = 12;
const PQC_ENC_TAG_BYTES: usize = 16;
const PQC_WORKER_STACK_BYTES: usize = 160 * 1024;
const SE050_POWER_SETTLE_MS: u32 = 20;
const SE050_POWER_CYCLE_LOW_MS: u32 = 20;
const BUTTON_CONFIRM_TIMEOUT_MS: u32 = 30_000;
const BUTTON_CONFIRM_SIGN_HOLD_MS: u32 = 1_500;
const BUTTON_CONFIRM_PQC_INIT_HOLD_MS: u32 = 2_000;
const BUTTON_CONFIRM_GENERATE_HOLD_MS: u32 = 2_000;
const BUTTON_CONFIRM_WIPE_HOLD_MS: u32 = 3_000;
const LOCAL_FLOW_START_HOLD_MS: u32 = 1_200;
const LOCAL_PIN_CONFIRM_HOLD_MS: u32 = 700;
const LOCAL_PIN_IDLE_TIMEOUT_MS: u64 = 60_000;
const LOCAL_PIN_ERROR_SHOW_MS: u32 = 900;
const BUTTON_TAP_DEBOUNCE_MS: u64 = 50;

type DynOutputPin = PinDriver<'static, AnyIOPin, Output>;
type DynInputPin = PinDriver<'static, AnyIOPin, Input>;

const NVS_AUTH_MODE: &str = "auth_mode";
const NVS_AUTH_FINALIZED: &str = "auth_finalized";
const NVS_AUTH_FAIL_COUNT: &str = "auth_fail_count";
const NVS_AUTH_LOCKED: &str = "auth_locked";
const NVS_PIN_SALT: &str = "pin_salt";
const NVS_PIN_HASH: &str = "pin_hash";
const NVS_OTP_SECRET: &str = "otp_secret";
const NVS_OTP_LAST: &str = "otp_last";
const NVS_OTP_ENROLLED: &str = "otp_enrolled";
const NVS_PQC_VKEY: &str = "pqc_vk";
const NVS_PQC_BIND_SIG: &str = "pqc_bind";
const NVS_PQC_SK_CT: &str = "pqc_sk_ct";
const NVS_PQC_SK_SALT: &str = "pqc_salt";
const NVS_PQC_SK_NONCE: &str = "pqc_nonce";
const NVS_PQC_WRAP_VER: &str = "pqc_wv";
const PQC_WRAP_VERSION_PIN_ONLY_V1: u8 = 1;
const PQC_WRAP_VERSION_SE050_PIN_V2: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthMode {
    Unset,
    None,
    Pin,
    Otp,
}

impl AuthMode {
    fn from_raw(raw: u8) -> Self {
        match raw {
            AUTH_NONE => Self::None,
            AUTH_PIN => Self::Pin,
            AUTH_OTP => Self::Otp,
            AUTH_BOTH => Self::Pin,
            _ => Self::Unset,
        }
    }

    fn from_legacy_raw(raw: u8) -> Self {
        match raw {
            0 => Self::None,
            1 => Self::Pin,
            2 => Self::Otp,
            3 => Self::Pin,
            _ => Self::Unset,
        }
    }

    fn as_raw(self) -> u8 {
        match self {
            Self::Unset => AUTH_UNSET,
            Self::None => AUTH_NONE,
            Self::Pin => AUTH_PIN,
            Self::Otp => AUTH_OTP,
        }
    }

    fn as_wire(self) -> &'static str {
        match self {
            Self::Unset => "UNSET",
            Self::None => "NONE",
            Self::Pin => "PIN",
            Self::Otp => "OTP",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyState {
    Uninitialized,
    Ready,
    Fault,
}

impl KeyState {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Uninitialized => "UNINITIALIZED",
            Self::Ready => "READY",
            Self::Fault => "FAULT",
        }
    }
}

#[derive(Clone, Copy, Debug)]
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

    fn requires_unlock_window(self) -> bool {
        self.finalized && matches!(self.mode, AuthMode::Pin | AuthMode::Otp)
    }
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            mode: AuthMode::Unset,
            finalized: false,
            fail_count: 0,
            locked: false,
        }
    }
}

struct SeState {
    session: Option<se050::Session>,
    pubkey_raw: Option<[u8; SE050_PUBKEY_BYTES]>,
    pubkey_base58: Option<String>,
    key_state: KeyState,
    last_err: Option<String>,
}

struct PqcState {
    verifying_key: Option<Vec<u8>>,
    verifying_key_b64: Option<String>,
    binding_sig_b64: Option<String>,
    encrypted_signing_key: Option<Vec<u8>>,
    enc_salt: Option<[u8; PQC_ENC_SALT_BYTES]>,
    enc_nonce: Option<[u8; PQC_ENC_NONCE_BYTES]>,
    wrap_version: u8,
    key_state: KeyState,
    last_err: Option<String>,
}

#[derive(Default)]
struct ButtonTracker {
    last_pressed: bool,
    press_started_at: Option<u64>,
    hold_emitted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonEvent {
    None,
    Tap,
    Hold,
}

#[derive(Clone, Copy, Debug)]
enum LocalPinMode {
    Setup,
    Confirm([u8; 6]),
    Unlock,
}

struct LocalPinFlow {
    mode: LocalPinMode,
    digits: [u8; 6],
    cursor: usize,
    last_activity_ms: u64,
}

fn settle_se050_power() {
    FreeRtos::delay_ms(SE050_POWER_SETTLE_MS);
}

fn setup_se050_power(enable_pin: Option<i32>) -> Result<Option<DynOutputPin>> {
    let Some(enable_pin) = enable_pin else {
        settle_se050_power();
        return Ok(None);
    };

    let mut se_enable = PinDriver::output(unsafe { AnyIOPin::new(enable_pin) })?;
    se_enable.set_high()?;
    settle_se050_power();
    Ok(Some(se_enable))
}

fn power_cycle_se050(se_enable: &mut Option<DynOutputPin>) -> Result<()> {
    if let Some(se_enable) = se_enable.as_mut() {
        se_enable.set_low()?;
        FreeRtos::delay_ms(SE050_POWER_CYCLE_LOW_MS);
        se_enable.set_high()?;
    }
    settle_se050_power();
    Ok(())
}

fn setup_user_button(button_pin: Option<i32>) -> Result<Option<DynInputPin>> {
    let Some(button_pin) = button_pin else {
        return Ok(None);
    };

    let mut button = PinDriver::input(unsafe { AnyIOPin::new(button_pin) })?;
    button.set_pull(Pull::Up)?;
    Ok(Some(button))
}

impl Default for PqcState {
    fn default() -> Self {
        Self {
            verifying_key: None,
            verifying_key_b64: None,
            binding_sig_b64: None,
            encrypted_signing_key: None,
            enc_salt: None,
            enc_nonce: None,
            wrap_version: PQC_WRAP_VERSION_SE050_PIN_V2,
            key_state: KeyState::Uninitialized,
            last_err: None,
        }
    }
}

fn main() -> Result<()> {
    esp_idf_sys::link_patches();
    unsafe {
        esp_idf_sys::esp_log_level_set(b"*\0".as_ptr(), esp_idf_sys::esp_log_level_t_ESP_LOG_NONE);
    }

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;
    let board = active_board_config();

    println!(
        "ℹ️  Firmware board profile: {} ({})",
        board.profile.as_wire(),
        board.description
    );

    let nvs_partition = EspDefaultNvsPartition::take()?;
    let mut nvs = EspNvs::new(nvs_partition, "signer", true)?;

    let mut uart = UsbSerialDriver::new(
        peripherals.usb_serial,
        pins.gpio19,
        pins.gpio20,
        &UsbSerialConfig::new()
            .rx_buffer_size((MAX_CMD_LEN + 256) as _)
            .tx_buffer_size(2048),
    )?;

    let mut se_enable = setup_se050_power(board.se050.enable)?;

    let mut i2c = I2cDriver::new(
        peripherals.i2c0,
        unsafe { AnyIOPin::new(board.se050.sda) },
        unsafe { AnyIOPin::new(board.se050.scl) },
        &I2cConfig::new().baudrate((100_u32).kHz().into()),
    )?;

    let mut screen = match board.display {
        Some(display_pins) => match display::Display::new(peripherals.spi2, display_pins) {
            Ok(mut display) => {
                let _ = display.show_lines("BOOT", &["Starting..."]);
                Some(display)
            }
            Err(err) => {
                send_response(&mut uart, &format!("WARN:TFT init failed: {err:?}"))?;
                None
            }
        },
        None => {
            send_response(
                &mut uart,
                &format!("INFO:display disabled for {}", board.profile.as_wire()),
            )?;
            None
        }
    };
    let mut user_button = setup_user_button(board.user_button)?;

    let mut se_state = init_se050(&mut i2c, &mut screen);
    let mut pqc_state = load_pqc_state(&mut nvs)?;
    let mut auth_state = load_auth_state(&mut nvs)?;
    let mut unlocked_until: u64 = 0;
    let mut session_pin_key: Option<[u8; SESSION_KEY_BYTES]> = None;
    let mut time_override: Option<u64> = None;
    let mut button_tracker = ButtonTracker::default();
    let mut local_pin_flow: Option<LocalPinFlow> = None;

    render_home_screen(
        &mut screen,
        &se_state,
        auth_state,
        unlocked_until,
        time_override,
    );

    let mut buffer: Vec<u8> = Vec::with_capacity(MAX_CMD_LEN + 64);

    loop {
        if session_pin_key.is_some()
            && auth_state.requires_unlock_window()
            && current_unix(time_override) > unlocked_until
        {
            clear_session_pin_key(&mut session_pin_key);
        }

        if let Some(display) = screen.as_mut() {
            let _ = display.check_idle();
        }

        let mut byte = [0u8; 1];
        match uart.read(&mut byte, TickType::new_millis(UART_POLL_MS).ticks()) {
            Ok(1) => {
                if let Some(display) = screen.as_mut() {
                    display.update_activity();
                }

                let b = byte[0];
                if b == b'\r' {
                    continue;
                }

                if b != b'\n' {
                    if buffer.len() >= MAX_CMD_LEN {
                        send_response(&mut uart, "ERROR:Command too long")?;
                        buffer.clear();
                    } else {
                        buffer.push(b);
                    }
                    continue;
                }

                let input = match std::str::from_utf8(&buffer) {
                    Ok(value) => value.trim().to_string(),
                    Err(_) => {
                        send_response(&mut uart, "ERROR:Invalid UTF-8")?;
                        buffer.clear();
                        continue;
                    }
                };
                buffer.clear();

                if input.is_empty() {
                    continue;
                }

                if local_pin_flow.is_some() {
                    local_pin_flow = None;
                    reset_button_tracker(&mut button_tracker);
                    render_home_screen(
                        &mut screen,
                        &se_state,
                        auth_state,
                        unlocked_until,
                        time_override,
                    );
                }

                if input == "PING" {
                    send_response(&mut uart, "PONG")?;
                    continue;
                }

                if auth_state.locked && !command_allowed_when_locked(&input) {
                    send_response(&mut uart, "ERROR:AUTH_LOCKED")?;
                    continue;
                }

                if se_state.key_state == KeyState::Fault && !command_allowed_when_faulted(&input) {
                    send_response(&mut uart, "ERROR:KEYSTORE_CORRUPT")?;
                    continue;
                }

                if input == "GET_INFO" {
                    let info = format!(
                        "INFO;VER={};BOARD={};AUTH_MODE={};FINALIZED={};LOCKED={};RETRIES_LEFT={};KEY_STATE={};PQC_STATE={}",
                        FW_VER,
                        board.profile.as_wire(),
                        auth_state.mode.as_wire(),
                        bool_to_wire(auth_state.finalized),
                        bool_to_wire(auth_state.locked),
                        auth_state.retries_left(),
                        se_state.key_state.as_wire(),
                        pqc_state.key_state.as_wire(),
                    );
                    send_response(&mut uart, &info)?;
                } else if input == "BUTTON_STATUS" {
                    send_response(
                        &mut uart,
                        &format!("BUTTON:{}", button_state_wire(&user_button)),
                    )?;
                } else if input == "GET_TIME" {
                    let now = current_unix(time_override);
                    send_response(&mut uart, &format!("TIME:{now}"))?;
                } else if let Some(raw_unix) = input.strip_prefix("SET_TIME:") {
                    let unix = match raw_unix.trim().parse::<u64>() {
                        Ok(value) => value,
                        Err(_) => {
                            send_response(&mut uart, "ERROR:BAD_TIME_FORMAT")?;
                            continue;
                        }
                    };

                    if unix < MIN_VALID_UNIX_TIME {
                        send_response(&mut uart, "ERROR:BAD_TIME_VALUE")?;
                        continue;
                    }

                    if !matches!(auth_state.mode, AuthMode::Unset | AuthMode::Otp) {
                        send_response(&mut uart, "ERROR:TIME_NOT_ALLOWED")?;
                        continue;
                    }

                    time_override = Some(unix);
                    send_response(&mut uart, &format!("TIME_SET:{unix}"))?;
                } else if input == "GET_PUBKEY" {
                    match se_state.key_state {
                        KeyState::Ready => {
                            let pubkey = se_state
                                .pubkey_base58
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing runtime pubkey"))?;
                            send_response(&mut uart, &format!("PUBKEY:{pubkey}"))?;
                        }
                        KeyState::Uninitialized => {
                            send_response(&mut uart, "ERROR:WALLET_NOT_INITIALIZED")?;
                        }
                        KeyState::Fault => {
                            send_response(&mut uart, "ERROR:KEYSTORE_CORRUPT")?;
                        }
                    }
                } else if input == "SET_MODE:NONE"
                    || input == "SET_MODE:OTP_BEGIN"
                    || input == "OTP_BEGIN"
                    || input.starts_with("SET_MODE:OTP_CONFIRM:")
                    || input.starts_with("OTP_CONFIRM:")
                {
                    send_response(&mut uart, "ERROR:PIN_REQUIRED")?;
                } else if input.starts_with("SET_AUTH:") {
                    let Some(pin) = parse_legacy_set_auth_pin(&input) else {
                        send_response(&mut uart, "ERROR:PIN_REQUIRED")?;
                        continue;
                    };
                    handle_set_mode_pin(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut nvs,
                        &mut se_state,
                        &mut auth_state,
                        &mut unlocked_until,
                        &mut session_pin_key,
                        pin,
                    )?;
                } else if let Some(pin_raw) = input.strip_prefix("SET_MODE:PIN:") {
                    let pin = pin_raw.split(':').next().unwrap_or("").trim();
                    handle_set_mode_pin(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut nvs,
                        &mut se_state,
                        &mut auth_state,
                        &mut unlocked_until,
                        &mut session_pin_key,
                        pin,
                    )?;
                } else if let Some(pin_raw) = input
                    .strip_prefix("UNLOCK:PIN:")
                    .or_else(|| input.strip_prefix("PIN_UNLOCK:"))
                    .or_else(|| input.strip_prefix("UNLOCK_PIN:"))
                {
                    let pin = pin_raw.split(':').next().unwrap_or("").trim();
                    handle_unlock_pin(
                        &mut uart,
                        &mut screen,
                        &se_state,
                        &mut nvs,
                        &mut auth_state,
                        &mut unlocked_until,
                        &mut session_pin_key,
                        time_override,
                        pin,
                    )?;
                } else if input.starts_with("UNLOCK:OTP:") || input.starts_with("OTP_UNLOCK:") {
                    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
                        send_response(&mut uart, "ERROR:MODE_UNSET")?;
                    } else {
                        send_response(&mut uart, "ERROR:AUTH_MODE_MISMATCH")?;
                    }
                } else if input == "GENERATE" {
                    handle_generate(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut nvs,
                        &mut se_state,
                        auth_state,
                        &mut unlocked_until,
                        time_override,
                    )?;
                } else if matches!(
                    input.as_str(),
                    "WIPE_KEYS" | "FACTORY_WIPE" | "RESET_DEVICE"
                ) {
                    handle_wipe(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut nvs,
                        &mut se_state,
                        &mut pqc_state,
                        &mut auth_state,
                        &mut unlocked_until,
                        &mut session_pin_key,
                        &mut time_override,
                    )?;
                } else if input == "SE050_STATUS" {
                    let status = match se_state.key_state {
                        KeyState::Ready => "SE050_OK".to_string(),
                        KeyState::Uninitialized => "SE050_BLANK".to_string(),
                        KeyState::Fault => match &se_state.last_err {
                            Some(err) => format!("SE050_ERR:{err}"),
                            None => "SE050_ERR:UNKNOWN".to_string(),
                        },
                    };
                    send_response(&mut uart, &status)?;
                } else if input == "SE050_REINIT" {
                    power_cycle_se050(&mut se_enable)?;
                    se_state = init_se050(&mut i2c, &mut screen);
                    let status = match se_state.key_state {
                        KeyState::Ready => "SE050_REINIT_OK".to_string(),
                        KeyState::Uninitialized => "SE050_REINIT_BLANK".to_string(),
                        KeyState::Fault => match &se_state.last_err {
                            Some(err) => format!("SE050_REINIT_ERR:{err}"),
                            None => "SE050_REINIT_ERR:UNKNOWN".to_string(),
                        },
                    };
                    send_response(&mut uart, &status)?;
                    render_home_screen(
                        &mut screen,
                        &se_state,
                        auth_state,
                        unlocked_until,
                        time_override,
                    );
                } else if input == "PQC_STATUS" {
                    let status = match pqc_state.key_state {
                        KeyState::Ready => "PQC_OK".to_string(),
                        KeyState::Uninitialized => "PQC_BLANK".to_string(),
                        KeyState::Fault => match &pqc_state.last_err {
                            Some(err) => format!("PQC_ERR:{err}"),
                            None => "PQC_ERR:UNKNOWN".to_string(),
                        },
                    };
                    send_response(&mut uart, &status)?;
                } else if input == "PQC_GET_PUBKEY" {
                    match pqc_state.key_state {
                        KeyState::Ready => {
                            let pubkey = pqc_state
                                .verifying_key_b64
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing PQC pubkey"))?;
                            send_response(&mut uart, &format!("PQC_PUBKEY:{pubkey}"))?;
                        }
                        KeyState::Uninitialized => {
                            send_response(&mut uart, "ERROR:PQC_NOT_INITIALIZED")?;
                        }
                        KeyState::Fault => {
                            send_response(&mut uart, "ERROR:PQC_FAULT")?;
                        }
                    }
                } else if input == "PQC_GET_BINDING" {
                    match pqc_state.key_state {
                        KeyState::Ready => {
                            let binding_sig = pqc_state
                                .binding_sig_b64
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing PQC binding"))?;
                            send_response(&mut uart, &format!("PQC_BINDING_SIG:{binding_sig}"))?;
                        }
                        KeyState::Uninitialized => {
                            send_response(&mut uart, "ERROR:PQC_NOT_INITIALIZED")?;
                        }
                        KeyState::Fault => {
                            send_response(&mut uart, "ERROR:PQC_FAULT")?;
                        }
                    }
                } else if input == "PQC_INIT" {
                    handle_pqc_init(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut nvs,
                        &mut se_state,
                        &mut pqc_state,
                        auth_state,
                        unlocked_until,
                        &session_pin_key,
                        time_override,
                    )?;
                } else if let Some(payload_b64) = input.strip_prefix("SIGN:") {
                    handle_sign(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut se_state,
                        auth_state,
                        unlocked_until,
                        time_override,
                        payload_b64,
                    )?;
                } else if let Some(payload_b64) = input.strip_prefix("SIGN_HYBRID:") {
                    handle_sign_hybrid(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut se_state,
                        &mut pqc_state,
                        auth_state,
                        unlocked_until,
                        &session_pin_key,
                        time_override,
                        payload_b64,
                    )?;
                } else if let Some(payload_b64) = input.strip_prefix("PQC_SIGN_VECTOR_DIGEST:") {
                    handle_pqc_sign_vector_digest(
                        &mut uart,
                        &mut screen,
                        &mut user_button,
                        &mut i2c,
                        &mut se_state,
                        &mut pqc_state,
                        auth_state,
                        unlocked_until,
                        &session_pin_key,
                        time_override,
                        payload_b64,
                    )?;
                } else if matches!(input.as_str(), "EXPORT_PRIVKEY" | "SHOW_PRIVKEY") {
                    send_response(&mut uart, "ERROR:UNSUPPORTED")?;
                } else if input == "SHUTDOWN" {
                    send_response(&mut uart, "SHUTDOWN_OK")?;
                } else {
                    send_response(&mut uart, "ERROR:Unknown command")?;
                }
            }
            Err(err) if err.code() == esp_idf_sys::ESP_ERR_TIMEOUT => {
                process_local_button_ui(
                    &mut screen,
                    &mut user_button,
                    &mut button_tracker,
                    &mut local_pin_flow,
                    &mut i2c,
                    &mut nvs,
                    &mut se_state,
                    &mut auth_state,
                    &mut unlocked_until,
                    &mut session_pin_key,
                    time_override,
                )?;
            }
            Err(err) => return Err(anyhow!("UART read error: {err:?}")),
            _ => {}
        }
    }
}

fn show_action_prompt(
    screen: &mut Option<display::Display>,
    title: &str,
    lines: &[&str],
) -> Result<()> {
    if let Some(display) = screen.as_mut() {
        display.show_lines(title, lines)?;
    }
    Ok(())
}

fn monotonic_ms() -> u64 {
    (unsafe { esp_idf_sys::esp_timer_get_time() } as u64) / 1000
}

fn button_is_pressed(user_button: &DynInputPin) -> bool {
    user_button.is_low()
}

fn button_state_wire(user_button: &Option<DynInputPin>) -> &'static str {
    match user_button.as_ref() {
        None => "UNAVAILABLE",
        Some(button) if button_is_pressed(button) => "PRESSED",
        Some(_) => "RELEASED",
    }
}

fn reset_button_tracker(tracker: &mut ButtonTracker) {
    *tracker = ButtonTracker::default();
}

fn poll_button_event(
    user_button: &Option<DynInputPin>,
    tracker: &mut ButtonTracker,
    hold_ms: u32,
) -> ButtonEvent {
    let Some(button) = user_button.as_ref() else {
        return ButtonEvent::None;
    };

    let now = monotonic_ms();
    let pressed = button_is_pressed(button);

    if pressed && !tracker.last_pressed {
        tracker.last_pressed = true;
        tracker.press_started_at = Some(now);
        tracker.hold_emitted = false;
        return ButtonEvent::None;
    }

    if pressed && tracker.last_pressed {
        if !tracker.hold_emitted
            && tracker
                .press_started_at
                .is_some_and(|started| now.saturating_sub(started) >= hold_ms as u64)
        {
            tracker.hold_emitted = true;
            return ButtonEvent::Hold;
        }
        return ButtonEvent::None;
    }

    if !pressed && tracker.last_pressed {
        let started = tracker.press_started_at.take();
        let hold_emitted = tracker.hold_emitted;
        tracker.last_pressed = false;
        tracker.hold_emitted = false;
        if !hold_emitted
            && started.is_some_and(|started_at| now.saturating_sub(started_at) >= BUTTON_TAP_DEBOUNCE_MS)
        {
            return ButtonEvent::Tap;
        }
    }

    ButtonEvent::None
}

fn local_pin_title(mode: LocalPinMode) -> &'static str {
    match mode {
        LocalPinMode::Setup => "SET DEVICE PIN",
        LocalPinMode::Confirm(_) => "CONFIRM PIN",
        LocalPinMode::Unlock => "UNLOCK WALLET",
    }
}

fn local_pin_subtitle(mode: LocalPinMode) -> &'static str {
    match mode {
        LocalPinMode::Setup => "Tap to cycle digits",
        LocalPinMode::Confirm(_) => "Re-enter the same 6 digits",
        LocalPinMode::Unlock => "Enter PIN without host input",
    }
}

fn local_pin_phase(cursor: usize) -> String {
    format!("Digit {} of 6", cursor + 1)
}

fn show_local_pin_flow(screen: &mut Option<display::Display>, flow: &LocalPinFlow) {
    let Some(display) = screen.as_mut() else {
        return;
    };

    let _ = display.show_pin_entry(
        local_pin_title(flow.mode),
        local_pin_subtitle(flow.mode),
        &flow.digits,
        flow.cursor,
        &local_pin_phase(flow.cursor),
    );
}

fn pin_digits_to_string(digits: &[u8; 6]) -> String {
    digits.iter().map(|digit| char::from(b'0' + *digit)).collect()
}

fn can_start_local_setup(auth_state: AuthState, se_state: &SeState) -> bool {
    matches!(auth_state.mode, AuthMode::Unset)
        || (!auth_state.finalized && auth_state.mode == AuthMode::Pin)
        || (auth_state.finalized
            && auth_state.mode == AuthMode::Pin
            && se_state.key_state == KeyState::Uninitialized)
}

fn can_start_local_unlock(
    auth_state: AuthState,
    se_state: &SeState,
    unlocked_until: u64,
    time_override: Option<u64>,
) -> bool {
    auth_state.finalized
        && auth_state.mode == AuthMode::Pin
        && !auth_state.locked
        && se_state.key_state == KeyState::Ready
        && current_unix(time_override) > unlocked_until
}

fn confirm_with_button<F>(
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    mut redraw_base: F,
    title: &str,
    subtitle: &str,
    hold_ms: u32,
    timeout_ms: u32,
) -> Result<bool>
where
    F: FnMut(&mut Option<display::Display>) -> Result<()>,
{
    if user_button.is_none() {
        return Ok(true);
    }

    redraw_base(screen)?;

    let deadline = monotonic_ms().saturating_add(timeout_ms as u64);
    let mut hold_started_at: Option<u64> = None;
    let mut showing_progress = false;
    let mut waiting_for_release = button_state_wire(user_button) == "PRESSED";

    loop {
        let now = monotonic_ms();
        let pressed = button_is_pressed(
            user_button
                .as_ref()
                .ok_or_else(|| anyhow!("missing user button"))?,
        );

        if waiting_for_release {
            if pressed {
                if let Some(display) = screen.as_mut() {
                    let _ = display.show_lines(title, &["Release button", "then hold to confirm"]);
                    display.update_activity();
                }
            } else {
                waiting_for_release = false;
                redraw_base(screen)?;
            }
        } else if pressed {
            let started_at = *hold_started_at.get_or_insert(now);
            let held_ms = now.saturating_sub(started_at).min(hold_ms as u64) as u32;
            if let Some(display) = screen.as_mut() {
                let _ = display.show_hold_action_progress(
                    title,
                    subtitle,
                    held_ms,
                    hold_ms,
                    Rgb565::new(0, 63, 0),
                );
                display.update_activity();
            }
            showing_progress = true;

            if held_ms >= hold_ms {
                return Ok(true);
            }
        } else {
            hold_started_at = None;
            if showing_progress {
                redraw_base(screen)?;
                showing_progress = false;
            }
        }

        if now >= deadline {
            if let Some(display) = screen.as_mut() {
                let _ = display.show_lines("CANCELLED", &["Confirmation timed out"]);
            }
            FreeRtos::delay_ms(500);
            return Ok(false);
        }

        FreeRtos::delay_ms(20);
    }
}

fn start_local_pin_flow(mode: LocalPinMode) -> LocalPinFlow {
    LocalPinFlow {
        mode,
        digits: [0; 6],
        cursor: 0,
        last_activity_ms: monotonic_ms(),
    }
}

fn finalize_local_pin_setup(
    screen: &mut Option<display::Display>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
    digits: &[u8; 6],
) -> Result<()> {
    let pin = pin_digits_to_string(digits);
    ensure_key_material(se_state, i2c, screen)?;
    clear_pin_hash(nvs)?;
    clear_otp_state(nvs)?;
    store_pin(nvs, &pin)?;
    set_mode_finalized(nvs, AuthMode::Pin)?;

    *auth_state = load_auth_state(nvs)?;
    *unlocked_until = 0;
    clear_session_pin_key(session_pin_key);

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("PIN SET", &["Wallet created", "Host PIN remains supported"]);
    }
    FreeRtos::delay_ms(LOCAL_PIN_ERROR_SHOW_MS);
    render_home_screen(screen, se_state, *auth_state, *unlocked_until, time_override);
    Ok(())
}

fn finalize_local_pin_unlock(
    screen: &mut Option<display::Display>,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &SeState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
    digits: &[u8; 6],
) -> Result<()> {
    let pin = pin_digits_to_string(digits);
    if verify_pin_hash(nvs, &pin)? {
        clear_auth_failures(nvs, auth_state)?;
        *unlocked_until = current_unix(time_override) + UNLOCK_WINDOW_SECS;
        store_session_pin_key(session_pin_key, derive_session_pin_key(&pin));
        if let Some(display) = screen.as_mut() {
            let _ = display.show_lines("UNLOCKED", &["PIN accepted"]);
        }
        FreeRtos::delay_ms(LOCAL_PIN_ERROR_SHOW_MS);
    } else {
        register_auth_failure(nvs, auth_state)?;
        if auth_state.locked {
            *unlocked_until = 0;
            clear_session_pin_key(session_pin_key);
            if let Some(display) = screen.as_mut() {
                let _ = display.show_lines("LOCKED", &["Too many attempts"]);
            }
        } else if let Some(display) = screen.as_mut() {
            let retries = auth_state.retries_left().to_string();
            let _ = display.show_lines("WRONG PIN", &["Try again", retries.as_str()]);
        }
        FreeRtos::delay_ms(LOCAL_PIN_ERROR_SHOW_MS);
    }

    render_home_screen(screen, se_state, *auth_state, *unlocked_until, time_override);
    Ok(())
}

fn process_local_button_ui(
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    tracker: &mut ButtonTracker,
    local_pin_flow: &mut Option<LocalPinFlow>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
) -> Result<()> {
    if user_button.is_none() {
        return Ok(());
    }

    if let Some(mut flow) = local_pin_flow.take() {
        if monotonic_ms().saturating_sub(flow.last_activity_ms) > LOCAL_PIN_IDLE_TIMEOUT_MS {
            if let Some(display) = screen.as_mut() {
                let _ = display.show_lines("PIN ENTRY", &["Timed out"]);
            }
            FreeRtos::delay_ms(LOCAL_PIN_ERROR_SHOW_MS);
            render_home_screen(screen, se_state, *auth_state, *unlocked_until, time_override);
            reset_button_tracker(tracker);
            return Ok(());
        }

        match poll_button_event(user_button, tracker, LOCAL_PIN_CONFIRM_HOLD_MS) {
            ButtonEvent::Tap => {
                flow.digits[flow.cursor] = (flow.digits[flow.cursor] + 1) % 10;
                flow.last_activity_ms = monotonic_ms();
                show_local_pin_flow(screen, &flow);
                *local_pin_flow = Some(flow);
            }
            ButtonEvent::Hold => {
                flow.last_activity_ms = monotonic_ms();
                if flow.cursor < 5 {
                    flow.cursor += 1;
                    show_local_pin_flow(screen, &flow);
                    *local_pin_flow = Some(flow);
                } else {
                    match flow.mode {
                        LocalPinMode::Setup => {
                            let first = flow.digits;
                            let next = LocalPinFlow {
                                mode: LocalPinMode::Confirm(first),
                                digits: [0; 6],
                                cursor: 0,
                                last_activity_ms: monotonic_ms(),
                            };
                            show_local_pin_flow(screen, &next);
                            *local_pin_flow = Some(next);
                        }
                        LocalPinMode::Confirm(first) => {
                            if first != flow.digits {
                                if let Some(display) = screen.as_mut() {
                                    let _ = display.show_lines("PIN MISMATCH", &["Start again"]);
                                }
                                FreeRtos::delay_ms(LOCAL_PIN_ERROR_SHOW_MS);
                                let next = start_local_pin_flow(LocalPinMode::Setup);
                                show_local_pin_flow(screen, &next);
                                *local_pin_flow = Some(next);
                            } else {
                                finalize_local_pin_setup(
                                    screen,
                                    i2c,
                                    nvs,
                                    se_state,
                                    auth_state,
                                    unlocked_until,
                                    session_pin_key,
                                    time_override,
                                    &flow.digits,
                                )?;
                                reset_button_tracker(tracker);
                            }
                        }
                        LocalPinMode::Unlock => {
                            finalize_local_pin_unlock(
                                screen,
                                nvs,
                                se_state,
                                auth_state,
                                unlocked_until,
                                session_pin_key,
                                time_override,
                                &flow.digits,
                            )?;
                            reset_button_tracker(tracker);
                        }
                    }
                }
            }
            ButtonEvent::None => {
                *local_pin_flow = Some(flow);
            }
        }

        return Ok(());
    }

    match poll_button_event(user_button, tracker, LOCAL_FLOW_START_HOLD_MS) {
        ButtonEvent::Tap => {
            render_home_screen(screen, se_state, *auth_state, *unlocked_until, time_override);
        }
        ButtonEvent::Hold => {
            let mode = if can_start_local_setup(*auth_state, se_state) {
                Some(LocalPinMode::Setup)
            } else if can_start_local_unlock(*auth_state, se_state, *unlocked_until, time_override) {
                Some(LocalPinMode::Unlock)
            } else {
                None
            };

            if let Some(mode) = mode {
                let flow = start_local_pin_flow(mode);
                show_local_pin_flow(screen, &flow);
                *local_pin_flow = Some(flow);
            }
        }
        ButtonEvent::None => {}
    }

    Ok(())
}

fn handle_set_mode_pin(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    pin: &str,
) -> Result<()> {
    if auth_state.finalized {
        send_response(uart, "ERROR:MODE_FINAL")?;
        return Ok(());
    }
    if !valid_pin(pin) {
        send_response(uart, "ERROR:BAD_PIN_FORMAT")?;
        return Ok(());
    }

    if !confirm_with_button(
        screen,
        user_button,
        |screen| show_action_prompt(screen, "CREATE WALLET", &["Hold button", "Create new wallet"]),
        "CREATE WALLET",
        "Hold to confirm",
        BUTTON_CONFIRM_GENERATE_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, *auth_state, *unlocked_until, None);
        return Ok(());
    }

    ensure_key_material(se_state, i2c, screen)?;
    clear_pin_hash(nvs)?;
    clear_otp_state(nvs)?;
    store_pin(nvs, pin)?;
    set_mode_finalized(nvs, AuthMode::Pin)?;

    *auth_state = load_auth_state(nvs)?;
    *unlocked_until = 0;
    clear_session_pin_key(session_pin_key);

    send_response(uart, "MODE_SET:PIN")?;
    render_home_screen(screen, se_state, *auth_state, *unlocked_until, None);
    Ok(())
}

fn handle_unlock_pin(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    se_state: &SeState,
    nvs: &mut EspNvs<NvsDefault>,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
    pin: &str,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_response(uart, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode != AuthMode::Pin {
        send_response(uart, "ERROR:AUTH_MODE_MISMATCH")?;
        return Ok(());
    }
    if !valid_pin(pin) {
        send_response(uart, "ERROR:BAD_PIN_FORMAT")?;
        return Ok(());
    }

    if verify_pin_hash(nvs, pin)? {
        clear_auth_failures(nvs, auth_state)?;
        *unlocked_until = current_unix(time_override) + UNLOCK_WINDOW_SECS;
        store_session_pin_key(session_pin_key, derive_session_pin_key(pin));
        send_response(uart, &format!("UNLOCKED_UNTIL:{unlocked_until}"))?;
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            time_override,
        );
    } else {
        register_auth_failure(nvs, auth_state)?;
        if auth_state.locked {
            *unlocked_until = 0;
            clear_session_pin_key(session_pin_key);
        }
        if auth_state.locked {
            send_response(uart, "ERROR:AUTH_LOCKED")?;
        } else {
            send_response(uart, "ERROR:AUTH_FAILED")?;
        }
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            time_override,
        );
    }

    Ok(())
}

fn handle_generate(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    auth_state: AuthState,
    unlocked_until: &mut u64,
    time_override: Option<u64>,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode != AuthMode::Pin {
        send_response(uart, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if se_state.key_state == KeyState::Ready {
        send_response(uart, "ERROR:WALLET_ALREADY_INITIALIZED")?;
        return Ok(());
    }

    if !confirm_with_button(
        screen,
        user_button,
        |screen| show_action_prompt(screen, "CREATE WALLET", &["Hold button", "Create new wallet"]),
        "CREATE WALLET",
        "Hold to confirm",
        BUTTON_CONFIRM_GENERATE_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        return Ok(());
    }

    ensure_key_material(se_state, i2c, screen)?;
    *unlocked_until = 0;
    let pubkey = se_state
        .pubkey_base58
        .as_deref()
        .ok_or_else(|| anyhow!("missing runtime pubkey"))?;

    send_response(uart, "KEY_GENERATED")?;
    send_response(uart, &format!("PUBKEY:{pubkey}"))?;
    let auth_state = load_auth_state(nvs)?;
    render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
    Ok(())
}

fn handle_wipe(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    pqc_state: &mut PqcState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    time_override: &mut Option<u64>,
) -> Result<()> {
    if !confirm_with_button(
        screen,
        user_button,
        |screen| show_action_prompt(screen, "WIPE DEVICE", &["Hold button", "Erase all keys"]),
        "WIPE DEVICE",
        "Hold to erase",
        BUTTON_CONFIRM_WIPE_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            *time_override,
        );
        return Ok(());
    }

    wipe_device(se_state, i2c, screen)?;
    wipe_auth_material(nvs)?;
    wipe_pqc_material(nvs)?;

    *auth_state = AuthState::default();
    *unlocked_until = 0;
    clear_session_pin_key(session_pin_key);
    *time_override = None;
    *pqc_state = PqcState::default();

    send_response(uart, "WIPED")?;
    render_home_screen(
        screen,
        se_state,
        *auth_state,
        *unlocked_until,
        *time_override,
    );
    Ok(())
}

fn handle_sign(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    se_state: &mut SeState,
    auth_state: AuthState,
    unlocked_until: u64,
    time_override: Option<u64>,
    payload_b64: &str,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_response(uart, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode == AuthMode::Otp {
        send_response(uart, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > unlocked_until {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if se_state.key_state == KeyState::Uninitialized {
        send_response(uart, "ERROR:WALLET_NOT_INITIALIZED")?;
        return Ok(());
    }

    let message_bytes = match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
        Ok(bytes) => bytes,
        Err(_) => {
            send_response(uart, "ERROR:Invalid base64 encoding")?;
            return Ok(());
        }
    };
    if message_bytes.len() > MAX_SIGN_PAYLOAD_BYTES {
        send_response(uart, "ERROR:PAYLOAD_TOO_LARGE")?;
        return Ok(());
    }

    show_sign_preview(screen, &message_bytes);
    if !confirm_with_button(
        screen,
        user_button,
        |screen| {
            show_sign_preview(screen, &message_bytes);
            Ok(())
        },
        "HOLD TO SIGN",
        "Review then hold button",
        BUTTON_CONFIRM_SIGN_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
        return Ok(());
    }
    ensure_se050_session(se_state, i2c, screen)?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("SIGNING", &["Ed25519 signature"]);
    }

    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    let signature = match session.sign_ed25519(i2c, &message_bytes) {
        Ok(signature) => signature,
        Err(err) => {
            let _ = session.recover_link(i2c);
            send_response(uart, &format!("ERROR:Sign failed: {err}"))?;
            return Ok(());
        }
    };

    let encoded = base64::engine::general_purpose::STANDARD.encode(&signature);
    send_response(uart, &format!("SIGNATURE:{encoded}"))?;
    let _ = session.drain(i2c);

    render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
    Ok(())
}

fn handle_pqc_init(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    pqc_state: &mut PqcState,
    auth_state: AuthState,
    unlocked_until: u64,
    session_pin_key: &Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_response(uart, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode == AuthMode::Otp {
        send_response(uart, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > unlocked_until {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if session_pin_key.is_none() {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if se_state.key_state == KeyState::Uninitialized {
        send_response(uart, "ERROR:WALLET_NOT_INITIALIZED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Ready {
        send_response(uart, "ERROR:PQC_ALREADY_INITIALIZED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Fault {
        send_response(uart, "ERROR:PQC_FAULT")?;
        return Ok(());
    }

    if !confirm_with_button(
        screen,
        user_button,
        |screen| show_action_prompt(screen, "INIT FALCON", &["Hold button", "Generate Falcon key"]),
        "INIT FALCON",
        "Hold to confirm",
        BUTTON_CONFIRM_PQC_INIT_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
        return Ok(());
    }

    ensure_se050_session(se_state, i2c, screen)?;
    let se_pubkey = se_state
        .pubkey_raw
        .ok_or_else(|| anyhow!("missing SE050 pubkey"))?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("PQC", &["Generating FN-DSA...", "Please wait"]);
    }

    let _watchdog_guard = match extend_task_watchdog(std::time::Duration::from_secs(90)) {
        Ok(guard) => guard,
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };

    let mut key_material = match run_pqc_worker("pqc-keygen", || falcon::generate_keypair()) {
        Ok(material) => material,
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };
    let init_result = (|| -> Result<(Vec<u8>, EncryptedPqcKey, u8)> {
        let binding_message =
            falcon::build_binding_message(&se_pubkey, &key_material.verifying_key);
        let session = se_state
            .session
            .as_mut()
            .ok_or_else(|| anyhow!("missing SE050 session"))?;
        let binding_signature = match session.sign_ed25519(i2c, &binding_message) {
            Ok(signature) => signature,
            Err(err) => {
                let _ = session.recover_link(i2c);
                return Err(anyhow!("PQC_BIND_FAILED:{err}"));
            }
        };
        let _ = session.drain(i2c);
        verify_ed25519_signature(&se_pubkey, &binding_message, &binding_signature)?;

        let (encrypted_signing_key, wrap_version) = encrypt_pqc_signing_key_current(
            &key_material.signing_key,
            se_state,
            i2c,
            screen,
            session_pin_key
                .as_ref()
                .ok_or_else(|| anyhow!("missing PIN session key"))?,
        )?;

        Ok((binding_signature, encrypted_signing_key, wrap_version))
    })();

    match init_result {
        Ok((binding_signature, encrypted_signing_key, wrap_version)) => {
            key_material.signing_key.zeroize();

            nvs.set_raw(NVS_PQC_VKEY, &key_material.verifying_key)?;
            nvs.set_raw(NVS_PQC_BIND_SIG, &binding_signature)?;
            nvs.set_raw(NVS_PQC_SK_CT, &encrypted_signing_key.ciphertext)?;
            nvs.set_raw(NVS_PQC_SK_SALT, &encrypted_signing_key.salt)?;
            nvs.set_raw(NVS_PQC_SK_NONCE, &encrypted_signing_key.nonce)?;
            nvs.set_raw(NVS_PQC_WRAP_VER, &[wrap_version])?;

            *pqc_state = build_pqc_state(
                key_material.verifying_key,
                binding_signature,
                encrypted_signing_key.ciphertext,
                encrypted_signing_key.salt,
                encrypted_signing_key.nonce,
                wrap_version,
            );

            send_response(uart, "PQC_READY")?;
            send_response(
                uart,
                &format!(
                    "PQC_PUBKEY:{}",
                    pqc_state
                        .verifying_key_b64
                        .as_deref()
                        .ok_or_else(|| anyhow!("missing PQC pubkey"))?
                ),
            )?;
            send_response(
                uart,
                &format!(
                    "PQC_BINDING_SIG:{}",
                    pqc_state
                        .binding_sig_b64
                        .as_deref()
                        .ok_or_else(|| anyhow!("missing PQC binding"))?
                ),
            )?;
        }
        Err(err) => {
            key_material.signing_key.zeroize();
            send_response(uart, &format!("ERROR:{err}"))?;
        }
    }

    render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
    Ok(())
}

fn handle_sign_hybrid(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    se_state: &mut SeState,
    pqc_state: &mut PqcState,
    auth_state: AuthState,
    unlocked_until: u64,
    session_pin_key: &Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
    payload_b64: &str,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_response(uart, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode == AuthMode::Otp {
        send_response(uart, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > unlocked_until {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if session_pin_key.is_none() {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if se_state.key_state == KeyState::Uninitialized {
        send_response(uart, "ERROR:WALLET_NOT_INITIALIZED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Uninitialized {
        send_response(uart, "ERROR:PQC_NOT_INITIALIZED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Fault {
        send_response(uart, "ERROR:PQC_FAULT")?;
        return Ok(());
    }

    let Some(message_bytes) = decode_sign_payload(uart, payload_b64)? else {
        return Ok(());
    };

    show_sign_preview(screen, &message_bytes);
    if !confirm_with_button(
        screen,
        user_button,
        |screen| {
            show_sign_preview(screen, &message_bytes);
            Ok(())
        },
        "HYBRID SIGN",
        "Hold to sign both",
        BUTTON_CONFIRM_SIGN_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
        return Ok(());
    }
    ensure_se050_session(se_state, i2c, screen)?;

    let se_pubkey = se_state
        .pubkey_raw
        .ok_or_else(|| anyhow!("missing SE050 pubkey"))?;
    let binding_sig_b64 = pqc_state
        .binding_sig_b64
        .clone()
        .ok_or_else(|| anyhow!("missing PQC binding"))?;
    let verifying_key_b64 = pqc_state
        .verifying_key_b64
        .clone()
        .ok_or_else(|| anyhow!("missing PQC pubkey"))?;
    let encrypted_signing_key = pqc_state
        .encrypted_signing_key
        .clone()
        .ok_or_else(|| anyhow!("missing PQC signing key"))?;
    let enc_salt = pqc_state
        .enc_salt
        .ok_or_else(|| anyhow!("missing PQC encryption salt"))?;
    let enc_nonce = pqc_state
        .enc_nonce
        .ok_or_else(|| anyhow!("missing PQC encryption nonce"))?;
    let wrap_version = pqc_state.wrap_version;
    let session_pin_key = *session_pin_key
        .as_ref()
        .ok_or_else(|| anyhow!("missing PIN session key"))?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("SIGNING", &["SE050 + FN-DSA"]);
    }

    let _watchdog_guard = match extend_task_watchdog(std::time::Duration::from_secs(30)) {
        Ok(guard) => guard,
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };

    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    let ed25519_signature = match session.sign_ed25519(i2c, &message_bytes) {
        Ok(signature) => signature,
        Err(err) => {
            let _ = session.recover_link(i2c);
            send_response(uart, &format!("ERROR:Sign failed: {err}"))?;
            return Ok(());
        }
    };
    let _ = session.drain(i2c);

    let wrap_key = match derive_pqc_encryption_key_with_se050(
        se_state,
        i2c,
        screen,
        &session_pin_key,
        &enc_salt,
    ) {
        Ok(key) => key,
        Err(_err) if wrap_version == PQC_WRAP_VERSION_PIN_ONLY_V1 => {
            derive_pqc_encryption_key_legacy(&session_pin_key, &enc_salt)
        }
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };

    let pqc_message = falcon::build_hybrid_message(&se_pubkey, &message_bytes);
    let pqc_signature = match run_pqc_worker("pqc-sign", move || {
        let mut signing_key = decrypt_pqc_signing_key_with_key(
            &encrypted_signing_key,
            &enc_nonce,
            wrap_key,
        )?;
        let signature = falcon::sign_message(&signing_key, &pqc_message);
        signing_key.zeroize();
        signature
    }) {
        Ok(signature) => signature,
        Err(err) => {
            mark_pqc_fault(pqc_state, format!("WORKER:{err}"));
            send_response(uart, "ERROR:PQC_FAULT")?;
            return Ok(());
        }
    };

    let encoded_ed25519 = base64::engine::general_purpose::STANDARD.encode(&ed25519_signature);
    let encoded_pqc = base64::engine::general_purpose::STANDARD.encode(&pqc_signature);

    send_response(uart, &format!("SIGNATURE:{encoded_ed25519}"))?;
    send_response(uart, &format!("PQC_SIGNATURE:{encoded_pqc}"))?;
    send_response(uart, &format!("PQC_PUBKEY:{verifying_key_b64}"))?;
    send_response(uart, &format!("PQC_BINDING_SIG:{binding_sig_b64}"))?;

    render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
    Ok(())
}

fn handle_pqc_sign_vector_digest(
    uart: &mut UsbSerialDriver,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    se_state: &mut SeState,
    pqc_state: &mut PqcState,
    auth_state: AuthState,
    unlocked_until: u64,
    session_pin_key: &Option<[u8; SESSION_KEY_BYTES]>,
    time_override: Option<u64>,
    payload_b64: &str,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_response(uart, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode == AuthMode::Otp {
        send_response(uart, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > unlocked_until {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if session_pin_key.is_none() {
        send_response(uart, "ERROR:LOCKED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Uninitialized {
        send_response(uart, "ERROR:PQC_NOT_INITIALIZED")?;
        return Ok(());
    }
    if pqc_state.key_state == KeyState::Fault {
        send_response(uart, "ERROR:PQC_FAULT")?;
        return Ok(());
    }

    let Some(digest_bytes) = decode_sign_payload(uart, payload_b64)? else {
        return Ok(());
    };
    if digest_bytes.len() != 32 {
        send_response(uart, "ERROR:VECTOR_DIGEST_LEN")?;
        return Ok(());
    }

    if !confirm_with_button(
        screen,
        user_button,
        |screen| show_action_prompt(screen, "VECTOR SIGN", &["Hold button", "Approve digest"]),
        "VECTOR SIGN",
        "Hold to sign digest",
        BUTTON_CONFIRM_SIGN_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_response(uart, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
        return Ok(());
    }

    let verifying_key = pqc_state
        .verifying_key
        .clone()
        .ok_or_else(|| anyhow!("missing PQC pubkey"))?;
    let verifying_key_b64 = pqc_state
        .verifying_key_b64
        .clone()
        .ok_or_else(|| anyhow!("missing PQC pubkey"))?;
    let encrypted_signing_key = pqc_state
        .encrypted_signing_key
        .clone()
        .ok_or_else(|| anyhow!("missing PQC signing key"))?;
    let enc_salt = pqc_state
        .enc_salt
        .ok_or_else(|| anyhow!("missing PQC encryption salt"))?;
    let enc_nonce = pqc_state
        .enc_nonce
        .ok_or_else(|| anyhow!("missing PQC encryption nonce"))?;
    let wrap_version = pqc_state.wrap_version;
    let session_pin_key = *session_pin_key
        .as_ref()
        .ok_or_else(|| anyhow!("missing PIN session key"))?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("PQC SIGN", &["Vector digest"]);
    }

    let _watchdog_guard = match extend_task_watchdog(std::time::Duration::from_secs(30)) {
        Ok(guard) => guard,
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };

    let wrap_key = match derive_pqc_encryption_key_with_se050(
        se_state,
        i2c,
        screen,
        &session_pin_key,
        &enc_salt,
    ) {
        Ok(key) => key,
        Err(_err) if wrap_version == PQC_WRAP_VERSION_PIN_ONLY_V1 => {
            derive_pqc_encryption_key_legacy(&session_pin_key, &enc_salt)
        }
        Err(err) => {
            send_response(uart, &format!("ERROR:{err}"))?;
            return Ok(());
        }
    };

    let vector_signature = match run_pqc_worker("pqc-vector-sign", move || {
        let mut signing_key = decrypt_pqc_signing_key_with_key(
            &encrypted_signing_key,
            &enc_nonce,
            wrap_key,
        )?;
        let signature = falcon::sign_message_original_falcon(&signing_key, &digest_bytes);
        signing_key.zeroize();
        signature
    }) {
        Ok(signature) => signature,
        Err(err) => {
            mark_pqc_fault(pqc_state, format!("WORKER:{err}"));
            send_response(uart, "ERROR:PQC_FAULT")?;
            return Ok(());
        }
    };

    let vector_identity = falcon::vector_identity(&verifying_key);
    let encoded_signature = base64::engine::general_purpose::STANDARD.encode(&vector_signature);
    let encoded_identity = base64::engine::general_purpose::STANDARD.encode(vector_identity);

    send_response(uart, &format!("PQC_VECTOR_SIGNATURE:{encoded_signature}"))?;
    send_response(uart, &format!("PQC_VECTOR_IDENTITY:{encoded_identity}"))?;
    send_response(uart, &format!("PQC_PUBKEY:{verifying_key_b64}"))?;

    render_home_screen(screen, se_state, auth_state, unlocked_until, time_override);
    Ok(())
}

struct EncryptedPqcKey {
    ciphertext: Vec<u8>,
    salt: [u8; PQC_ENC_SALT_BYTES],
    nonce: [u8; PQC_ENC_NONCE_BYTES],
}

struct TaskWatchdogGuard {
    original: esp_idf_sys::esp_task_wdt_config_t,
    restore: bool,
}

impl Drop for TaskWatchdogGuard {
    fn drop(&mut self) {
        if self.restore {
            let _ = unsafe {
                esp_idf_sys::esp_task_wdt_reconfigure(
                    &self.original as *const esp_idf_sys::esp_task_wdt_config_t,
                )
            };
        }
    }
}

fn extend_task_watchdog(timeout: std::time::Duration) -> Result<TaskWatchdogGuard> {
    let base = TWDTConfig::new();
    let original: esp_idf_sys::esp_task_wdt_config_t = (&base).into();
    if timeout <= base.duration {
        return Ok(TaskWatchdogGuard {
            original,
            restore: false,
        });
    }

    let mut extended = base.clone();
    extended.duration = timeout;
    let extended_raw: esp_idf_sys::esp_task_wdt_config_t = (&extended).into();

    esp_idf_sys::esp!(unsafe {
        esp_idf_sys::esp_task_wdt_reconfigure(
            &extended_raw as *const esp_idf_sys::esp_task_wdt_config_t,
        )
    })
    .map_err(|err| anyhow!("failed to extend task watchdog: {err}"))?;

    Ok(TaskWatchdogGuard {
        original,
        restore: true,
    })
}

fn run_pqc_worker<T, F>(name: &str, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let handle = thread::Builder::new()
        .name(name.to_string())
        .stack_size(PQC_WORKER_STACK_BYTES)
        .spawn(f)
        .map_err(|err| anyhow!("failed to spawn {name}: {err}"))?;

    match handle.join() {
        Ok(result) => result,
        Err(_) => Err(anyhow!("{name} panicked")),
    }
}

fn decode_sign_payload(uart: &mut UsbSerialDriver, payload_b64: &str) -> Result<Option<Vec<u8>>> {
    let message_bytes = match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
        Ok(bytes) => bytes,
        Err(_) => {
            send_response(uart, "ERROR:Invalid base64 encoding")?;
            return Ok(None);
        }
    };
    if message_bytes.len() > MAX_SIGN_PAYLOAD_BYTES {
        send_response(uart, "ERROR:PAYLOAD_TOO_LARGE")?;
        return Ok(None);
    }
    Ok(Some(message_bytes))
}

fn build_pqc_state(
    verifying_key: Vec<u8>,
    binding_sig: Vec<u8>,
    encrypted_signing_key: Vec<u8>,
    enc_salt: [u8; PQC_ENC_SALT_BYTES],
    enc_nonce: [u8; PQC_ENC_NONCE_BYTES],
    wrap_version: u8,
) -> PqcState {
    PqcState {
        verifying_key_b64: Some(base64::engine::general_purpose::STANDARD.encode(&verifying_key)),
        binding_sig_b64: Some(base64::engine::general_purpose::STANDARD.encode(&binding_sig)),
        verifying_key: Some(verifying_key),
        encrypted_signing_key: Some(encrypted_signing_key),
        enc_salt: Some(enc_salt),
        enc_nonce: Some(enc_nonce),
        wrap_version,
        key_state: KeyState::Ready,
        last_err: None,
    }
}

fn load_pqc_state(nvs: &mut EspNvs<NvsDefault>) -> Result<PqcState> {
    let verifying_key = get_blob(nvs, NVS_PQC_VKEY, falcon::verifying_key_len())?;
    let binding_sig = get_blob(nvs, NVS_PQC_BIND_SIG, SE050_SIG_BYTES)?;
    let encrypted_signing_key = get_blob(
        nvs,
        NVS_PQC_SK_CT,
        falcon::signing_key_len() + PQC_ENC_TAG_BYTES,
    )?;
    let enc_salt = get_fixed_blob::<PQC_ENC_SALT_BYTES>(nvs, NVS_PQC_SK_SALT)?;
    let enc_nonce = get_fixed_blob::<PQC_ENC_NONCE_BYTES>(nvs, NVS_PQC_SK_NONCE)?;
    let wrap_version = get_u8_key(nvs, NVS_PQC_WRAP_VER)?;

    match (
        verifying_key,
        binding_sig,
        encrypted_signing_key,
        enc_salt,
        enc_nonce,
    ) {
        (None, None, None, None, None) => Ok(PqcState::default()),
        (
            Some(verifying_key),
            Some(binding_sig),
            Some(encrypted_signing_key),
            Some(enc_salt),
            Some(enc_nonce),
        ) => {
            if verifying_key.len() != falcon::verifying_key_len() {
                return Ok(PqcState {
                    key_state: KeyState::Fault,
                    last_err: Some("LOAD:BAD_VKEY_LEN".to_string()),
                    ..PqcState::default()
                });
            }
            if binding_sig.len() != SE050_SIG_BYTES {
                return Ok(PqcState {
                    key_state: KeyState::Fault,
                    last_err: Some("LOAD:BAD_BIND_LEN".to_string()),
                    ..PqcState::default()
                });
            }
            if encrypted_signing_key.len() != falcon::signing_key_len() + PQC_ENC_TAG_BYTES {
                return Ok(PqcState {
                    key_state: KeyState::Fault,
                    last_err: Some("LOAD:BAD_SK_LEN".to_string()),
                    ..PqcState::default()
                });
            }
            let wrap_version = wrap_version.unwrap_or(PQC_WRAP_VERSION_PIN_ONLY_V1);
            if !matches!(
                wrap_version,
                PQC_WRAP_VERSION_PIN_ONLY_V1 | PQC_WRAP_VERSION_SE050_PIN_V2
            ) {
                return Ok(PqcState {
                    key_state: KeyState::Fault,
                    last_err: Some("LOAD:BAD_WRAP_VER".to_string()),
                    ..PqcState::default()
                });
            }

            Ok(build_pqc_state(
                verifying_key,
                binding_sig,
                encrypted_signing_key,
                enc_salt,
                enc_nonce,
                wrap_version,
            ))
        }
        _ => Ok(PqcState {
            key_state: KeyState::Fault,
            last_err: Some("LOAD:PARTIAL_STATE".to_string()),
            ..PqcState::default()
        }),
    }
}

fn wipe_pqc_material(nvs: &mut EspNvs<NvsDefault>) -> Result<()> {
    let _ = nvs.remove(NVS_PQC_VKEY)?;
    let _ = nvs.remove(NVS_PQC_BIND_SIG)?;
    let _ = nvs.remove(NVS_PQC_SK_CT)?;
    let _ = nvs.remove(NVS_PQC_SK_SALT)?;
    let _ = nvs.remove(NVS_PQC_SK_NONCE)?;
    let _ = nvs.remove(NVS_PQC_WRAP_VER)?;
    Ok(())
}

fn clear_session_pin_key(session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>) {
    if let Some(mut key) = session_pin_key.take() {
        key.zeroize();
    }
}

fn store_session_pin_key(
    session_pin_key: &mut Option<[u8; SESSION_KEY_BYTES]>,
    derived_key: [u8; SESSION_KEY_BYTES],
) {
    clear_session_pin_key(session_pin_key);
    *session_pin_key = Some(derived_key);
}

fn derive_session_pin_key(pin: &str) -> [u8; SESSION_KEY_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"pin-session-v2");
    hasher.update(pin.as_bytes());

    let mut key = [0u8; SESSION_KEY_BYTES];
    key.copy_from_slice(&hasher.finalize());
    key
}

fn derive_pqc_encryption_key_legacy(
    session_pin_key: &[u8; SESSION_KEY_BYTES],
    enc_salt: &[u8; PQC_ENC_SALT_BYTES],
) -> [u8; SESSION_KEY_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"pqc-aes-gcm-v1");
    hasher.update(session_pin_key);
    hasher.update(enc_salt);

    let mut out = [0u8; SESSION_KEY_BYTES];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn derive_pqc_encryption_key_with_se050(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
    session_pin_key: &[u8; SESSION_KEY_BYTES],
    enc_salt: &[u8; PQC_ENC_SALT_BYTES],
) -> Result<[u8; SESSION_KEY_BYTES]> {
    ensure_se050_session(se_state, i2c, screen)?;
    let se_pubkey = se_state
        .pubkey_raw
        .ok_or_else(|| anyhow!("missing SE050 pubkey"))?;
    let wrap_message = falcon::build_wrap_key_message(&se_pubkey);
    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    let mut wrap_signature = match session.sign_ed25519(i2c, &wrap_message) {
        Ok(signature) => signature,
        Err(first_err) => {
            let _ = session.recover_link(i2c);
            session
                .sign_ed25519(i2c, &wrap_message)
                .map_err(|retry_err| anyhow!("PQC_WRAP_DERIVE_FAILED:{first_err};RETRY:{retry_err}"))?
        }
    };
    let _ = session.drain(i2c);
    verify_ed25519_signature(&se_pubkey, &wrap_message, &wrap_signature)?;

    let mut hasher = Sha256::new();
    hasher.update(b"pqc-aes-gcm-v2");
    hasher.update(&wrap_signature);
    hasher.update(session_pin_key);
    hasher.update(enc_salt);

    let mut out = [0u8; SESSION_KEY_BYTES];
    out.copy_from_slice(&hasher.finalize());
    wrap_signature.zeroize();
    Ok(out)
}

fn encrypt_pqc_signing_key_with_key(
    signing_key: &[u8],
    mut enc_key: [u8; SESSION_KEY_BYTES],
    salt: [u8; PQC_ENC_SALT_BYTES],
) -> Result<EncryptedPqcKey> {
    let mut nonce = [0u8; PQC_ENC_NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);

    let cipher =
        Aes256Gcm::new_from_slice(&enc_key).map_err(|_| anyhow!("failed to build AES-GCM key"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), signing_key)
        .map_err(|_| anyhow!("failed to encrypt PQC signing key"))?;
    enc_key.zeroize();

    Ok(EncryptedPqcKey {
        ciphertext,
        salt,
        nonce,
    })
}

fn decrypt_pqc_signing_key_with_key(
    encrypted_signing_key: &[u8],
    enc_nonce: &[u8; PQC_ENC_NONCE_BYTES],
    mut enc_key: [u8; SESSION_KEY_BYTES],
) -> Result<Vec<u8>> {
    let cipher =
        Aes256Gcm::new_from_slice(&enc_key).map_err(|_| anyhow!("failed to build AES-GCM key"))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(enc_nonce), encrypted_signing_key)
        .map_err(|_| anyhow!("failed to decrypt PQC signing key"))?;
    enc_key.zeroize();
    Ok(plaintext)
}

fn encrypt_pqc_signing_key_current(
    signing_key: &[u8],
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
    session_pin_key: &[u8; SESSION_KEY_BYTES],
) -> Result<(EncryptedPqcKey, u8)> {
    let mut salt = [0u8; PQC_ENC_SALT_BYTES];
    OsRng.fill_bytes(&mut salt);
    let enc_key =
        derive_pqc_encryption_key_with_se050(se_state, i2c, screen, session_pin_key, &salt)?;
    let encrypted_key = encrypt_pqc_signing_key_with_key(signing_key, enc_key, salt)?;
    Ok((encrypted_key, PQC_WRAP_VERSION_SE050_PIN_V2))
}

fn verify_ed25519_signature(
    verifying_key: &[u8; SE050_PUBKEY_BYTES],
    message: &[u8],
    signature: &[u8],
) -> Result<()> {
    let signature_bytes: [u8; SE050_SIG_BYTES] = signature
        .try_into()
        .map_err(|_| anyhow!("invalid Ed25519 signature length"))?;
    let verifying_key = Ed25519VerifyingKey::from_bytes(verifying_key)
        .map_err(|err| anyhow!("failed to decode Ed25519 verifying key: {err}"))?;
    let signature = Ed25519Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(message, &signature)
        .map_err(|err| anyhow!("Ed25519 verification failed: {err}"))
}

fn mark_pqc_fault(pqc_state: &mut PqcState, err: String) {
    pqc_state.key_state = KeyState::Fault;
    pqc_state.last_err = Some(err);
}

fn init_se050(i2c: &mut I2cDriver, screen: &mut Option<display::Display>) -> SeState {
    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("SE050", &["Initializing..."]);
    }

    match se050::Session::init(i2c, SE050_I2C_ADDR, SE050_KEY_ID) {
        Ok(session) => {
            let mut state = SeState {
                session: Some(session),
                pubkey_raw: None,
                pubkey_base58: None,
                key_state: KeyState::Uninitialized,
                last_err: None,
            };
            refresh_se050_pubkey(&mut state, i2c);
            state
        }
        Err(err) => SeState {
            session: None,
            pubkey_raw: None,
            pubkey_base58: None,
            key_state: KeyState::Fault,
            last_err: Some(format!("INIT:{err}")),
        },
    }
}

fn ensure_se050_session(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
) -> Result<()> {
    if se_state.session.is_some() && se_state.key_state != KeyState::Fault {
        return Ok(());
    }

    *se_state = init_se050(i2c, screen);
    if se_state.session.is_some() && se_state.key_state != KeyState::Fault {
        return Ok(());
    }

    Err(anyhow!(
        "{}",
        se_state
            .last_err
            .clone()
            .unwrap_or_else(|| "SE050_NOT_READY".to_string())
    ))
}

fn ensure_key_material(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
) -> Result<()> {
    if se_state.key_state == KeyState::Ready {
        return Ok(());
    }

    ensure_se050_session(se_state, i2c, screen)?;
    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("SETUP", &["Creating wallet..."]);
    }

    session.generate_ed25519_keypair(i2c)?;
    refresh_se050_pubkey(se_state, i2c);
    if se_state.key_state != KeyState::Ready {
        return Err(anyhow!("KEYGEN_FAILED"));
    }
    Ok(())
}

fn wipe_device(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
) -> Result<()> {
    ensure_se050_session(se_state, i2c, screen)?;

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("WIPE", &["Clearing device..."]);
    }

    {
        let session = se_state
            .session
            .as_mut()
            .ok_or_else(|| anyhow!("missing SE050 session"))?;

        if let Err(first_err) = session.delete_ed25519_keypair(i2c) {
            let _ = session.recover_link(i2c);
            session
                .delete_ed25519_keypair(i2c)
                .map_err(|second_err| anyhow!("WIPE_FAILED:{first_err};RETRY:{second_err}"))?;
        }
        let _ = session.drain(i2c);
    }

    refresh_se050_pubkey(se_state, i2c);
    if se_state.key_state == KeyState::Fault {
        if let Some(session) = se_state.session.as_mut() {
            let _ = session.recover_link(i2c);
        }
        refresh_se050_pubkey(se_state, i2c);
    }
    if se_state.key_state == KeyState::Ready {
        return Err(anyhow!("WIPE_VERIFY_FAILED"));
    }
    if se_state.key_state == KeyState::Fault {
        return Err(anyhow!(
            "{}",
            se_state
                .last_err
                .clone()
                .unwrap_or_else(|| "WIPE_VERIFY_FAILED".to_string())
        ));
    }
    Ok(())
}

fn render_home_screen(
    screen: &mut Option<display::Display>,
    se_state: &SeState,
    auth_state: AuthState,
    unlocked_until: u64,
    time_override: Option<u64>,
) {
    let Some(display) = screen.as_mut() else {
        return;
    };

    match se_state.key_state {
        KeyState::Fault => {
            let line = se_state
                .last_err
                .as_deref()
                .unwrap_or("Secure element fault");
            let _ = display.show_lines("SE050", &[line]);
        }
        KeyState::Uninitialized => {
            if auth_state.finalized && auth_state.mode == AuthMode::Pin {
                let _ = display.show_lines("SETUP", &["PIN stored", "Hold button to create wallet"]);
            } else {
                let _ = display.show_lines("SETUP", &["Hold button to set PIN", "USB host flow still works"]);
            }
        }
        KeyState::Ready => {
            let pubkey = match se_state.pubkey_base58.as_deref() {
                Some(pubkey) => pubkey,
                None => return,
            };

            let now = current_unix(time_override);
            let (extra, connection) = match auth_state.mode {
                AuthMode::Pin if auth_state.finalized && now <= unlocked_until => {
                    (Some("PIN unlocked"), ConnectionState::Ready)
                }
                AuthMode::Pin if auth_state.finalized => {
                    (Some("Hold button to unlock"), ConnectionState::Locked)
                }
                AuthMode::Pin => (Some("Hold button to finish setup"), ConnectionState::Disconnected),
                AuthMode::Unset => (Some("Hold button to set PIN"), ConnectionState::Disconnected),
                AuthMode::None => (Some("Legacy NONE mode"), ConnectionState::Ready),
                AuthMode::Otp => (Some("Legacy OTP mode"), ConnectionState::Locked),
            };

            let _ = display.show_wallet_ready(pubkey, extra, connection);
        }
    }
}

fn show_sign_preview(screen: &mut Option<display::Display>, message_bytes: &[u8]) {
    let Some(display) = screen.as_mut() else {
        return;
    };

    match preview_verification(message_bytes) {
        Some(verification) => {
            let lines = verification.format_for_display();
            let _ = display.show_verification_results(&lines, verification.overall_safe);
        }
        None => {
            let _ = display.show_lines("SIGN", &["Blind signing", "Raw payload"]);
        }
    }
}

fn preview_verification(message_bytes: &[u8]) -> Option<VerificationResults> {
    let parser = TransactionParser::new(message_bytes.to_vec());
    parser.verify_mvp_transaction().ok()
}

fn refresh_se050_pubkey(se_state: &mut SeState, i2c: &mut I2cDriver) {
    let Some(session) = se_state.session.as_mut() else {
        se_state.pubkey_raw = None;
        se_state.pubkey_base58 = None;
        se_state.key_state = KeyState::Fault;
        return;
    };

    match session.read_ed25519_pubkey(i2c) {
        Ok(pubkey) => {
            se_state.pubkey_raw = Some(pubkey);
            se_state.pubkey_base58 = Some(bs58::encode(pubkey).into_string());
            se_state.key_state = KeyState::Ready;
            se_state.last_err = None;
        }
        Err(err) => {
            se_state.pubkey_raw = None;
            se_state.pubkey_base58 = None;
            if is_missing_se050_key_error(&err) {
                se_state.key_state = KeyState::Uninitialized;
                se_state.last_err = None;
            } else {
                se_state.key_state = KeyState::Fault;
                se_state.last_err = Some(format!("PUBKEY_READ:{err}"));
            }
        }
    }
}

fn is_missing_se050_key_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("READ_OBJECT_SW_") || msg.contains("TLV_41_NOT_FOUND")
}

fn load_auth_state(nvs: &mut EspNvs<NvsDefault>) -> Result<AuthState> {
    let raw_auth_mode = get_u8_key(nvs, NVS_AUTH_MODE)?;
    let finalized_raw = get_bool_flag(nvs, NVS_AUTH_FINALIZED)?;

    let mut mode = raw_auth_mode
        .map(AuthMode::from_raw)
        .unwrap_or(AuthMode::Unset);
    let mut finalized = finalized_raw.unwrap_or(false);

    if finalized_raw.is_none() {
        if let Some(raw) = raw_auth_mode {
            mode = AuthMode::from_legacy_raw(raw);
            finalized = mode != AuthMode::Unset;
            set_auth_mode(nvs, mode)?;
            set_bool_flag(nvs, NVS_AUTH_FINALIZED, finalized)?;
        }
    }

    let fail_count = get_u8_key(nvs, NVS_AUTH_FAIL_COUNT)?
        .unwrap_or(0)
        .min(MAX_AUTH_ATTEMPTS);
    let locked =
        get_bool_flag(nvs, NVS_AUTH_LOCKED)?.unwrap_or(false) || fail_count >= MAX_AUTH_ATTEMPTS;

    Ok(AuthState {
        mode,
        finalized,
        fail_count,
        locked,
    })
}

fn command_allowed_when_locked(cmd: &str) -> bool {
    matches!(
        cmd,
        "GET_INFO"
            | "BUTTON_STATUS"
            | "GET_PUBKEY"
            | "GET_TIME"
            | "PQC_STATUS"
            | "PQC_GET_PUBKEY"
            | "PQC_GET_BINDING"
            | "SE050_STATUS"
            | "SE050_REINIT"
            | "WIPE_KEYS"
            | "FACTORY_WIPE"
            | "RESET_DEVICE"
    ) || cmd.starts_with("SET_TIME:")
}

fn command_allowed_when_faulted(cmd: &str) -> bool {
    command_allowed_when_locked(cmd)
}

fn parse_legacy_set_auth_pin(input: &str) -> Option<&str> {
    let payload = input.strip_prefix("SET_AUTH:")?;
    let mut parts = payload.splitn(2, ':');
    let mode = parts.next()?.trim();
    let pin = parts.next()?.split(':').next()?.trim();

    match mode {
        "1" | "PIN" => Some(pin),
        _ => None,
    }
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == 6 && pin.chars().all(|ch| ch.is_ascii_digit())
}

fn current_unix(time_override: Option<u64>) -> u64 {
    time_override.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })
}

fn bool_to_wire(value: bool) -> u8 {
    if value {
        1
    } else {
        0
    }
}

fn send_response(uart: &mut UsbSerialDriver, response: &str) -> Result<()> {
    let line = format!("{response}\n");
    let bytes = line.as_bytes();
    let mut written = 0usize;
    let mut idle_tries = 0u32;

    while written < bytes.len() {
        let count = uart.write(&bytes[written..], TickType::new_millis(250).ticks())?;
        if count == 0 {
            idle_tries = idle_tries.saturating_add(1);
            if idle_tries > 200 {
                break;
            }
            FreeRtos::delay_ms(5);
            continue;
        }

        idle_tries = 0;
        written += count;
    }

    if written != bytes.len() {
        return Err(anyhow!("UART short write: {written}/{}", bytes.len()));
    }

    Ok(())
}

fn get_blob(nvs: &mut EspNvs<NvsDefault>, key: &str, max_len: usize) -> Result<Option<Vec<u8>>> {
    let mut buf = vec![0u8; max_len];
    match nvs.get_raw(key, &mut buf)? {
        Some(slice) => Ok(Some(slice.to_vec())),
        None => Ok(None),
    }
}

fn get_fixed_blob<const N: usize>(
    nvs: &mut EspNvs<NvsDefault>,
    key: &str,
) -> Result<Option<[u8; N]>> {
    let mut buf = [0u8; N];
    match nvs.get_raw(key, &mut buf)? {
        Some(slice) if slice.len() == N => Ok(Some(buf)),
        Some(_) => Ok(None),
        None => Ok(None),
    }
}

fn get_u8_key(nvs: &mut EspNvs<NvsDefault>, key: &str) -> Result<Option<u8>> {
    let mut buf = [0u8; 1];
    match nvs.get_raw(key, &mut buf)? {
        Some(slice) if slice.len() == 1 => Ok(Some(buf[0])),
        _ => Ok(None),
    }
}

fn set_u8_key(nvs: &mut EspNvs<NvsDefault>, key: &str, value: u8) -> Result<()> {
    nvs.set_raw(key, &[value])?;
    Ok(())
}

fn get_bool_flag(nvs: &mut EspNvs<NvsDefault>, key: &str) -> Result<Option<bool>> {
    Ok(get_u8_key(nvs, key)?.map(|value| value != 0))
}

fn set_bool_flag(nvs: &mut EspNvs<NvsDefault>, key: &str, value: bool) -> Result<()> {
    set_u8_key(nvs, key, bool_to_wire(value))
}

fn set_auth_mode(nvs: &mut EspNvs<NvsDefault>, mode: AuthMode) -> Result<()> {
    set_u8_key(nvs, NVS_AUTH_MODE, mode.as_raw())
}

fn set_mode_finalized(nvs: &mut EspNvs<NvsDefault>, mode: AuthMode) -> Result<()> {
    set_auth_mode(nvs, mode)?;
    set_bool_flag(nvs, NVS_AUTH_FINALIZED, true)?;
    set_u8_key(nvs, NVS_AUTH_FAIL_COUNT, 0)?;
    set_bool_flag(nvs, NVS_AUTH_LOCKED, false)?;
    Ok(())
}

fn clear_pin_hash(nvs: &mut EspNvs<NvsDefault>) -> Result<()> {
    let _ = nvs.remove(NVS_PIN_SALT)?;
    let _ = nvs.remove(NVS_PIN_HASH)?;
    Ok(())
}

fn clear_otp_state(nvs: &mut EspNvs<NvsDefault>) -> Result<()> {
    let _ = nvs.remove(NVS_OTP_SECRET)?;
    let _ = nvs.remove(NVS_OTP_LAST)?;
    let _ = nvs.remove(NVS_OTP_ENROLLED)?;
    Ok(())
}

fn wipe_auth_material(nvs: &mut EspNvs<NvsDefault>) -> Result<()> {
    clear_pin_hash(nvs)?;
    clear_otp_state(nvs)?;
    let _ = nvs.remove(NVS_AUTH_MODE)?;
    let _ = nvs.remove(NVS_AUTH_FINALIZED)?;
    let _ = nvs.remove(NVS_AUTH_FAIL_COUNT)?;
    let _ = nvs.remove(NVS_AUTH_LOCKED)?;
    Ok(())
}

fn clear_auth_failures(nvs: &mut EspNvs<NvsDefault>, auth_state: &mut AuthState) -> Result<()> {
    auth_state.fail_count = 0;
    auth_state.locked = false;
    set_u8_key(nvs, NVS_AUTH_FAIL_COUNT, 0)?;
    set_bool_flag(nvs, NVS_AUTH_LOCKED, false)?;
    Ok(())
}

fn register_auth_failure(nvs: &mut EspNvs<NvsDefault>, auth_state: &mut AuthState) -> Result<()> {
    if auth_state.fail_count < MAX_AUTH_ATTEMPTS {
        auth_state.fail_count += 1;
    }
    if auth_state.fail_count >= MAX_AUTH_ATTEMPTS {
        auth_state.fail_count = MAX_AUTH_ATTEMPTS;
        auth_state.locked = true;
    }

    set_u8_key(nvs, NVS_AUTH_FAIL_COUNT, auth_state.fail_count)?;
    set_bool_flag(nvs, NVS_AUTH_LOCKED, auth_state.locked)?;
    Ok(())
}

fn derive_pin_hash(pin: &str, salt: &[u8; PIN_SALT_BYTES]) -> [u8; PIN_HASH_BYTES] {
    let mut hash = [0u8; PIN_HASH_BYTES];
    pbkdf2_hmac::<Sha256>(pin.as_bytes(), salt, PIN_HASH_ITERS, &mut hash);
    hash
}

fn store_pin(nvs: &mut EspNvs<NvsDefault>, pin: &str) -> Result<()> {
    let mut salt = [0u8; PIN_SALT_BYTES];
    OsRng.fill_bytes(&mut salt);
    let hash = derive_pin_hash(pin, &salt);
    nvs.set_raw(NVS_PIN_SALT, &salt)?;
    nvs.set_raw(NVS_PIN_HASH, &hash)?;
    Ok(())
}

fn verify_pin_hash(nvs: &mut EspNvs<NvsDefault>, pin: &str) -> Result<bool> {
    let mut salt = [0u8; PIN_SALT_BYTES];
    let mut stored_hash = [0u8; PIN_HASH_BYTES];

    let salt_ok = matches!(
        nvs.get_raw(NVS_PIN_SALT, &mut salt)?,
        Some(slice) if slice.len() == PIN_SALT_BYTES
    );
    let hash_ok = matches!(
        nvs.get_raw(NVS_PIN_HASH, &mut stored_hash)?,
        Some(slice) if slice.len() == PIN_HASH_BYTES
    );

    if !salt_ok || !hash_ok {
        return Ok(false);
    }

    let computed = derive_pin_hash(pin, &salt);
    Ok(computed.ct_eq(&stored_hash).into())
}
