use anyhow::{anyhow, Result};
use base64::Engine;
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use esp_idf_svc::bt::BtDriver;
use esp_idf_svc::hal::{
    delay::{FreeRtos, TickType},
    gpio::{AnyIOPin, Input, Output, PinDriver, Pull},
    i2c::{I2cConfig, I2cDriver},
    prelude::{FromValueType, Peripherals},
    usb_serial::{UsbSerialConfig, UsbSerialDriver},
};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

mod ble_session;
mod ble_transport;
mod ble_tx;
mod board;
mod build_info;
mod display;
mod io_budget;
mod local_ui;
mod pairing_lifecycle;
mod review_ui;
mod se050;
mod se050_auth;
mod se050_key_lifecycle;
mod se050_policy;
mod signing_review;
mod transport_frame;
mod tropic;
mod tropic_access;
mod tropic_l2;
mod usb_host;
mod usb_session;
mod wallet_permissions;

use ble_transport::{BleStatusSnapshot, WalletBleTransport};
use board::active_board_config;
use transport_frame::{write_all_line, write_usb_response, LineAccumulator, LineDecodeError};
use usb_session::UsbSession;

const FW_VER: &str = env!("CARGO_PKG_VERSION");

/// The factory eFuse MAC, also reported by espflash board-info. Do not use the
/// derived Bluetooth address or a software-overridden interface MAC as identity.
fn hardware_mac() -> String {
    let mut mac = [0u8; 6];
    let result = unsafe { esp_idf_sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    if result != esp_idf_sys::ESP_OK || mac == [0; 6] || mac == [0xff; 6] {
        return "UNAVAILABLE".into();
    }
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

const SE050_I2C_ADDR: u8 = 0x48;
const SE050_KEY_ID: u32 = 0x0010_0001;
const SE050_I2C_KHZ: u32 = 400;

const AUTH_UNSET: u8 = 0;
const AUTH_NONE: u8 = 1;
const AUTH_PIN: u8 = 2;
const AUTH_OTP: u8 = 3;
const AUTH_BOTH: u8 = 4; // legacy current-firmware value, coerced to PIN

const MAX_AUTH_ATTEMPTS: u8 = 10;
const MAX_SIGN_PAYLOAD_BYTES: usize = 4096;
const MAX_CMD_LEN: usize = 5504;
const UART_POLL_MS: u64 = 50;
const UNLOCK_WINDOW_SECS: u64 = 420;
const PIN_SALT_BYTES: usize = 16;
const PIN_HASH_BYTES: usize = 32;
const PIN_HASH_ITERS: u32 = 10_000;
const MIN_VALID_UNIX_TIME: u64 = 1_577_836_800; // 2020-01-01
const SE050_PUBKEY_BYTES: usize = 32;
const SE050_POWER_SETTLE_MS: u32 = 20;
const SE050_POWER_CYCLE_LOW_MS: u32 = 20;
const BUTTON_CONFIRM_TIMEOUT_MS: u32 = 30_000;
const BUTTON_CONFIRM_GENERATE_HOLD_MS: u32 = 2_000;
const RECEIVE_QR_TIMEOUT_MS: u64 = 30_000;

type DynOutputPin = PinDriver<'static, AnyIOPin, Output>;
type DynInputPin = PinDriver<'static, AnyIOPin, Input>;

static TRANSPORT_NOTE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RECEIVE_QR_OVERRIDE: OnceLock<Mutex<Option<ReceiveQrDisplayState>>> = OnceLock::new();
static TROPIC_HOME: OnceLock<Mutex<TropicHomeSnapshot>> = OnceLock::new();
static ACTIVE_TRANSPORT: OnceLock<Mutex<Option<HostCommandSource>>> = OnceLock::new();

const TRANSPORT_OWNER_IDLE_MS: u64 = 15_000;

const NVS_AUTH_MODE: &str = "auth_mode";
const NVS_AUTH_FINALIZED: &str = "auth_finalized";
const NVS_AUTH_FAIL_COUNT: &str = "auth_fail_count";
const NVS_AUTH_LOCKED: &str = "auth_locked";
const NVS_WIPE_PENDING: &str = "wipe_pending";
const NVS_PIN_SALT: &str = "pin_salt";
const NVS_PIN_HASH: &str = "pin_hash";
const NVS_OTP_SECRET: &str = "otp_secret";
const NVS_OTP_LAST: &str = "otp_last";
const NVS_OTP_ENROLLED: &str = "otp_enrolled";

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

struct TropicState {
    runtime: Option<tropic::Runtime>,
    pubkey_raw: Option<[u8; SE050_PUBKEY_BYTES]>,
    pubkey_base58: Option<String>,
    key_state: KeyState,
    access_state: String,
    active_pairing_key_index: Option<u8>,
    last_err: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct ReceiveQrDisplayState {
    expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostCommandSource {
    Usb(UsbSession),
    Ble(u64),
}

impl HostCommandSource {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Usb(_) => "USB",
            Self::Ble(_) => "BLE",
        }
    }
}

fn source_is_active(source: HostCommandSource, ble: Option<&WalletBleTransport>) -> bool {
    match source {
        HostCommandSource::Usb(session) => usb_host::is_active(session),
        HostCommandSource::Ble(generation) => {
            ble.is_some_and(|transport| transport.is_session_active(generation))
        }
    }
}

#[derive(Clone, Debug)]
struct TropicHomeSnapshot {
    key_state: KeyState,
    last_err: Option<String>,
}

impl Default for TropicHomeSnapshot {
    fn default() -> Self {
        Self {
            key_state: KeyState::Fault,
            last_err: Some("TROPIC_UNINITIALIZED".to_string()),
        }
    }
}

#[derive(Default)]
struct TransportOwner {
    source: Option<HostCommandSource>,
    last_activity_ms: u64,
}

fn transport_note_cell() -> &'static Mutex<Option<String>> {
    TRANSPORT_NOTE.get_or_init(|| Mutex::new(None))
}

fn active_transport_cell() -> &'static Mutex<Option<HostCommandSource>> {
    ACTIVE_TRANSPORT.get_or_init(|| Mutex::new(None))
}

fn tropic_home_cell() -> &'static Mutex<TropicHomeSnapshot> {
    TROPIC_HOME.get_or_init(|| Mutex::new(TropicHomeSnapshot::default()))
}

fn receive_qr_override_cell() -> &'static Mutex<Option<ReceiveQrDisplayState>> {
    RECEIVE_QR_OVERRIDE.get_or_init(|| Mutex::new(None))
}

fn set_transport_note(note: Option<String>) {
    *transport_note_cell().lock().unwrap() = note;
}

fn current_transport_note() -> Option<String> {
    transport_note_cell().lock().unwrap().clone()
}

fn set_active_transport(source: Option<HostCommandSource>) {
    *active_transport_cell().lock().unwrap() = source;
}

fn current_active_transport() -> Option<HostCommandSource> {
    *active_transport_cell().lock().unwrap()
}

fn sync_tropic_home_snapshot(tropic_state: &TropicState) {
    *tropic_home_cell().lock().unwrap() = TropicHomeSnapshot {
        key_state: tropic_state.key_state,
        last_err: tropic_state.last_err.clone(),
    };
}

fn current_tropic_home_snapshot() -> TropicHomeSnapshot {
    tropic_home_cell().lock().unwrap().clone()
}

fn refresh_receive_qr_override() {
    let expires_at_ms = monotonic_ms().saturating_add(RECEIVE_QR_TIMEOUT_MS);
    *receive_qr_override_cell().lock().unwrap() = Some(ReceiveQrDisplayState { expires_at_ms });
}

fn clear_receive_qr_override() -> bool {
    receive_qr_override_cell().lock().unwrap().take().is_some()
}

fn receive_qr_override_active() -> bool {
    let now_ms = monotonic_ms();
    let mut guard = receive_qr_override_cell().lock().unwrap();
    match *guard {
        Some(state) if now_ms <= state.expires_at_ms => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

fn clear_expired_receive_qr_override() -> bool {
    let now_ms = monotonic_ms();
    let mut guard = receive_qr_override_cell().lock().unwrap();
    match *guard {
        Some(state) if now_ms > state.expires_at_ms => {
            *guard = None;
            true
        }
        _ => false,
    }
}

fn ble_transport_note(snapshot: &BleStatusSnapshot) -> Option<String> {
    if snapshot.subscribed {
        Some("iPhone linked".to_string())
    } else if snapshot.connected {
        Some("Phone connected".to_string())
    } else if snapshot.advertising {
        Some("BLE discoverable".to_string())
    } else {
        snapshot.last_event.clone()
    }
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
    let mut nvs = EspNvs::new(nvs_partition.clone(), "signer", true)?;

    let mut uart = UsbSerialDriver::new(
        peripherals.usb_serial,
        pins.gpio19,
        pins.gpio20,
        &UsbSerialConfig::new()
            .rx_buffer_size((MAX_CMD_LEN + 256) as _)
            .tx_buffer_size(2048),
    )?;
    usb_host::start()?;
    // Establish the receive boundary before slow secure-element startup. Older
    // clients can queue PING while that startup runs; preserve it if the USB
    // session has not changed. Initial bytes predate any authorized session.
    FreeRtos::delay_ms(30);
    let startup_usb_session = usb_host::session();
    let mut usb_receive_session =
        if discard_usb_input(&mut uart)? && usb_host::session() == startup_usb_session {
            Some(startup_usb_session)
        } else {
            None
        };

    let ble = match BtDriver::new(peripherals.modem, Some(nvs_partition.clone())) {
        Ok(driver) => match WalletBleTransport::start(Arc::new(driver)) {
            Ok(transport) => {
                set_transport_note(ble_transport_note(&transport.status_snapshot()));
                Some(transport)
            }
            Err(err) => {
                println!("⚠️  BLE transport init failed: {err:?}");
                set_transport_note(Some("BLE init failed".to_string()));
                None
            }
        },
        Err(err) => {
            println!("⚠️  Bluetooth controller init failed: {err:?}");
            set_transport_note(Some("BLE unavailable".to_string()));
            None
        }
    };
    let mut last_ble_status = ble.as_ref().map(|transport| transport.status_snapshot());

    let mut se_enable = setup_se050_power(board.se050.enable)?;

    let mut i2c = I2cDriver::new(
        peripherals.i2c0,
        unsafe { AnyIOPin::new(board.se050.sda) },
        unsafe { AnyIOPin::new(board.se050.scl) },
        &I2cConfig::new().baudrate(SE050_I2C_KHZ.kHz().into()),
    )?;
    println!("ℹ️  SE050 I2C bus: {} kHz", SE050_I2C_KHZ);

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
    if user_button.is_some() {
        println!("ℹ️  User button enabled");
    } else {
        println!("ℹ️  User button disabled for this build");
    }

    let mut se_state = init_se050(&mut i2c, &mut screen);
    let mut tropic_state = init_tropic(
        peripherals.spi3,
        board.tropic,
        &mut se_state,
        &mut i2c,
        &mut screen,
    );
    // Leave experimental keys untouched on an update, but never report such a
    // unit as factory-blank. Falcon is archived on the checkpoint branch.
    let mut legacy_pqc_present = false;
    for key in [
        "pqc_vk",
        "pqc_bind",
        "pqc_sk_ct",
        "pqc_salt",
        "pqc_nonce",
        "pqc_wv",
    ] {
        legacy_pqc_present |= nvs.contains(key)?;
    }
    let legacy_pqc_state = if legacy_pqc_present {
        "LEGACY_STORED"
    } else {
        "UNINITIALIZED"
    };
    let mut auth_state = load_auth_state(&mut nvs)?;
    let mut unlocked_until: u64 = 0;
    let mut session_authorized = false;
    let mut time_override: Option<u64> = None;
    let mut local_ui = local_ui::LocalUi::new();
    let mut local_usb_session = usb_host::session();
    let mut transport_owner = TransportOwner::default();
    set_active_transport(None);

    render_home_screen(
        &mut screen,
        &se_state,
        auth_state,
        unlocked_until,
        time_override,
    );

    let mut command_lines = LineAccumulator::new(MAX_CMD_LEN);
    let mut shown_pairing_generation = None;

    loop {
        let disconnected_owner = transport_owner
            .source
            .is_some_and(|source| !source_is_active(source, ble.as_ref()));
        let expired = session_authorized
            && auth_state.requires_unlock_window()
            && current_unix(time_override) > unlocked_until;
        if disconnected_owner || expired {
            local_ui.close(monotonic_ms());
            revoke_unlock(
                &mut tropic_state,
                &mut unlocked_until,
                &mut session_authorized,
            );
            clear_receive_qr_override();
            if disconnected_owner {
                transport_owner = TransportOwner::default();
                set_active_transport(None);
                local_ui.close(monotonic_ms());
            }
            render_home_screen(
                &mut screen,
                &se_state,
                auth_state,
                unlocked_until,
                time_override,
            );
        }

        let usb_session = usb_host::session();
        if (local_ui.active() || session_authorized)
            && transport_owner.source.is_none()
            && usb_session != local_usb_session
        {
            revoke_unlock(
                &mut tropic_state,
                &mut unlocked_until,
                &mut session_authorized,
            );
            local_ui.close(monotonic_ms());
        }
        if usb_receive_session != Some(usb_session) {
            // The IDF RX ring can retain bytes across a bus reset. Discard it
            // and any partial PIN/command before accepting this new session.
            command_lines.reset();
            if discard_usb_input(&mut uart)? && usb_host::session() == usb_session {
                usb_receive_session = Some(usb_session);
            }
        }

        if let Some(transport) = ble.as_ref() {
            transport.set_pairing_available(
                screen.is_some()
                    && !local_ui.active()
                    && !matches!(transport_owner.source, Some(HostCommandSource::Usb(_))),
            );
            transport.poll_security_timeout();
            if let Some(prompt) = transport.pairing_prompt() {
                if shown_pairing_generation != Some(prompt.generation) {
                    local_ui.close(monotonic_ms());
                    clear_receive_qr_override();
                    if let Some(display) = screen.as_mut() {
                        display.update_activity();
                        if display.show_pairing_code(prompt.passkey).is_err() {
                            transport.cancel_pairing();
                        }
                    }
                    shown_pairing_generation = Some(prompt.generation);
                }
                if user_button.as_ref().is_some_and(|button| {
                    local_ui
                        .button
                        .poll(button_is_pressed(button), monotonic_ms())
                        == Some(local_ui::Input::Cancel)
                }) {
                    transport.cancel_pairing();
                    local_ui.close(monotonic_ms());
                }
                FreeRtos::delay_ms(20);
                continue;
            }
            if shown_pairing_generation.take().is_some() {
                render_home_screen(
                    &mut screen,
                    &se_state,
                    auth_state,
                    unlocked_until,
                    time_override,
                );
            }
        }

        if let Some(owner) = transport_owner.source {
            let session_open = session_authorized
                && auth_state.requires_unlock_window()
                && current_unix(time_override) <= unlocked_until;
            if !session_open
                && !local_ui.active()
                && monotonic_ms().saturating_sub(transport_owner.last_activity_ms)
                    > TRANSPORT_OWNER_IDLE_MS
            {
                println!("🔌 transport owner released: {:?}", owner);
                transport_owner = TransportOwner::default();
                set_active_transport(None);
            }
        }

        if clear_expired_receive_qr_override() && !local_ui.active() {
            render_home_screen(
                &mut screen,
                &se_state,
                auth_state,
                unlocked_until,
                time_override,
            );
        }

        let mut host_command: Option<(HostCommandSource, Zeroizing<String>)> = None;
        let mut handled_host_command = false;

        let mut byte = [0u8; 1];
        let read_session = usb_host::session();
        match uart.read(&mut byte, TickType::new_millis(UART_POLL_MS).ticks()) {
            Ok(1) => {
                if usb_receive_session != Some(read_session) || !usb_host::is_active(read_session) {
                    continue;
                }
                if let Some(display) = screen.as_mut() {
                    display.update_activity();
                }

                match command_lines.push_byte(byte[0]) {
                    Ok(Some(line)) => {
                        host_command =
                            Some((HostCommandSource::Usb(read_session), Zeroizing::new(line)))
                    }
                    Ok(None) => {}
                    Err(LineDecodeError::CommandTooLong) => {
                        let _ = UsbResponseSink {
                            uart: &mut uart,
                            session: read_session,
                        }
                        .send_line("ERROR:Command too long");
                    }
                    Err(LineDecodeError::InvalidUtf8) => {
                        let _ = UsbResponseSink {
                            uart: &mut uart,
                            session: read_session,
                        }
                        .send_line("ERROR:Invalid UTF-8");
                    }
                }
            }
            Err(err) if err.code() == esp_idf_sys::ESP_ERR_TIMEOUT => {}
            Err(err) => return Err(anyhow!("UART read error: {err:?}")),
            _ => {}
        }

        // A loss while read() waited must revoke before dispatching either
        // transport. In particular, a reconnected USB host cannot inherit PIN.
        if usb_host::session() != read_session {
            continue;
        }

        if host_command.is_none() {
            if let Some(ble_transport) = ble.as_ref() {
                if let Some(line) = ble_transport.pop_command() {
                    if let Some(display) = screen.as_mut() {
                        display.update_activity();
                    }
                    host_command = Some((HostCommandSource::Ble(line.generation), line.line));
                }
            }
        }

        if let Some((source, input)) = host_command {
            if !source_is_active(source, ble.as_ref()) {
                continue;
            }
            if !input.is_empty() {
                // Local entry owns the screen/button. Hosts cannot interrupt it,
                // submit PIN guesses, or queue a signing approval behind it.
                if local_ui.active() && input.as_str() != "LOCK" {
                    let response = if input.as_str() == "PING" {
                        "PONG"
                    } else {
                        "ERROR:BUSY"
                    };
                    match source {
                        HostCommandSource::Usb(session) => {
                            let _ = UsbResponseSink {
                                uart: &mut uart,
                                session,
                            }
                            .send_line(response);
                        }
                        HostCommandSource::Ble(generation) => {
                            if let Some(transport) = ble.as_ref() {
                                transport.finish_command(generation);
                                let _ = transport.send_response(generation, response);
                            }
                        }
                    }
                } else {
                    let session_open = session_authorized
                        && auth_state.requires_unlock_window()
                        && current_unix(time_override) <= unlocked_until;
                    if let Some(owner) = transport_owner.source {
                        let owner_recent = monotonic_ms()
                            .saturating_sub(transport_owner.last_activity_ms)
                            <= TRANSPORT_OWNER_IDLE_MS;
                        if owner != source && (session_open || owner_recent) {
                            match source {
                                HostCommandSource::Usb(session) => {
                                    let _ = UsbResponseSink {
                                        uart: &mut uart,
                                        session,
                                    }
                                    .send_line("ERROR:BUSY");
                                }
                                HostCommandSource::Ble(generation) => {
                                    if let Some(ble_transport) = ble.as_ref() {
                                        ble_transport.finish_command(generation);
                                        let _ =
                                            ble_transport.send_response(generation, "ERROR:BUSY");
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    if local_ui::host_must_relock(
                        session_authorized,
                        transport_owner.source,
                        source,
                    ) {
                        revoke_unlock(
                            &mut tropic_state,
                            &mut unlocked_until,
                            &mut session_authorized,
                        );
                    }
                    local_ui.close(monotonic_ms());
                    handled_host_command = true;
                    transport_owner.source = Some(source);
                    transport_owner.last_activity_ms = monotonic_ms();
                    set_active_transport(Some(source));
                    if let Some(transport) = ble.as_ref() {
                        transport.set_pairing_available(false);
                    }

                    if !command_preserves_receive_qr(&input) {
                        clear_receive_qr_override();
                    }

                    let command_result = match source {
                        HostCommandSource::Usb(session) => {
                            let mut out = UsbResponseSink {
                                uart: &mut uart,
                                session,
                            };
                            handle_host_command(
                                &input,
                                &mut out,
                                board,
                                &mut screen,
                                &mut user_button,
                                &mut i2c,
                                &mut nvs,
                                &mut se_enable,
                                &mut se_state,
                                &mut tropic_state,
                                legacy_pqc_state,
                                &mut auth_state,
                                &mut unlocked_until,
                                &mut session_authorized,
                                &mut time_override,
                                ble.as_ref(),
                            )
                        }
                        HostCommandSource::Ble(generation) => {
                            println!("📶 BLE host command: {}", summarize_host_command(&input));
                            let ble_transport = ble.as_ref().ok_or_else(|| {
                                anyhow!("BLE command source without BLE transport")
                            })?;
                            let mut out = BleResponseSink {
                                ble: ble_transport,
                                generation,
                                responses: Vec::new(),
                            };
                            let result = handle_host_command(
                                &input,
                                &mut out,
                                board,
                                &mut screen,
                                &mut user_button,
                                &mut i2c,
                                &mut nvs,
                                &mut se_enable,
                                &mut se_state,
                                &mut tropic_state,
                                legacy_pqc_state,
                                &mut auth_state,
                                &mut unlocked_until,
                                &mut session_authorized,
                                &mut time_override,
                                Some(ble_transport),
                            );
                            // Finish handler work (including display updates) before
                            // publishing replies. A client may send its next request
                            // immediately after receiving the final response line.
                            ble_transport.finish_command(generation);
                            let response_result = out.flush();
                            result.and(response_result)
                        }
                    };

                    // This also covers loss during PIN verification, chip I/O,
                    // physical approval, or response transmission.
                    if !source_is_active(source, ble.as_ref()) {
                        revoke_unlock(
                            &mut tropic_state,
                            &mut unlocked_until,
                            &mut session_authorized,
                        );
                        clear_receive_qr_override();
                        transport_owner = TransportOwner::default();
                        set_active_transport(None);
                        local_ui.close(monotonic_ms());
                    }

                    if let Err(err) = command_result {
                        // Expected host loss is already handled above. Logging
                        // into a stopped USB console can leave a truncated line.
                        if source_is_active(source, ble.as_ref()) {
                            println!("⚠️  Host command {:?} failed: {err:?}", source);
                        }
                        render_home_screen(
                            &mut screen,
                            &se_state,
                            auth_state,
                            unlocked_until,
                            time_override,
                        );
                    }
                }
            }
        }
        if !handled_host_command {
            let was_active = local_ui.active();
            if let Err(error) = process_local_button_ui(
                &mut local_ui,
                &mut screen,
                &user_button,
                ble.as_ref(),
                &mut se_state,
                &mut tropic_state,
                &mut i2c,
                &mut nvs,
                &mut auth_state,
                &mut unlocked_until,
                &mut session_authorized,
                time_override,
                &mut transport_owner,
            ) {
                revoke_unlock(
                    &mut tropic_state,
                    &mut unlocked_until,
                    &mut session_authorized,
                );
                local_ui.close(monotonic_ms());
                println!("Local controls failed: {error}");
            }
            if !was_active && local_ui.active() {
                local_usb_session = usb_host::session();
            }
        }

        if let Some(ble_transport) = ble.as_ref() {
            let snapshot = ble_transport.status_snapshot();
            if last_ble_status.as_ref() != Some(&snapshot) {
                set_transport_note(ble_transport_note(&snapshot));
                if !local_ui.active() {
                    render_home_screen(
                        &mut screen,
                        &se_state,
                        auth_state,
                        unlocked_until,
                        time_override,
                    );
                }
                last_ble_status = Some(snapshot);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_host_command(
    input: &str,
    out: &mut dyn ResponseSink,
    board: board::BoardConfig,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_enable: &mut Option<DynOutputPin>,
    se_state: &mut SeState,
    tropic_state: &mut TropicState,
    legacy_pqc_state: &str,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_authorized: &mut bool,
    time_override: &mut Option<u64>,
    ble: Option<&WalletBleTransport>,
) -> Result<()> {
    anyhow::ensure!(out.is_active(), "HOST_SESSION_CLOSED");
    if input == "LOCK" {
        revoke_unlock(tropic_state, unlocked_until, session_authorized);
        send_host_response(out, "LOCKED")?;
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            *time_override,
        );
        return Ok(());
    }
    if input == "PING" {
        send_host_response(out, "PONG")?;
        return Ok(());
    }

    #[cfg(feature = "qualification")]
    if input == "QUALIFY_BOOTSTRAP"
        || input == "QUALIFY_STORAGE"
        || input.starts_with("QUALIFY_PIN:")
    {
        revoke_unlock(tropic_state, unlocked_until, session_authorized);
        let result = if input == "QUALIFY_BOOTSTRAP" {
            ensure_tropic_runtime(tropic_state)?.verify_bootstrap_cannot_sign()
        } else if input == "QUALIFY_STORAGE" {
            ensure_se050_session(se_state, i2c, screen)?;
            se_state
                .session
                .as_mut()
                .ok_or_else(|| anyhow!("SE050_UNAVAILABLE"))?
                .qualification_check_pin_storage(
                    i2c,
                    tropic::SE050_PIN_AUTH_ID,
                    tropic::TROPIC_ACCESS_OBJECT_ID,
                )
        } else {
            // Deliberately bypass ESP32/NVS lockout to test the SE050 itself.
            let pin = input.strip_prefix("QUALIFY_PIN:").unwrap_or("");
            if !valid_pin(pin) {
                Err(anyhow!("BAD_PIN_FORMAT"))
            } else {
                match open_native_pin_session(se_state, i2c, screen, pin) {
                    Ok(session) => {
                        close_or_recover_pin_session(se_state, i2c, &session, "qualification")
                    }
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Ok(()) => send_host_response(out, "QUALIFY:PASS")?,
            Err(error) => send_host_response(out, &format!("QUALIFY:ERROR:{error}"))?,
        }
        return Ok(());
    }

    if auth_state.locked && !command_allowed_when_locked(input) {
        send_host_response(out, "ERROR:AUTH_LOCKED")?;
        return Ok(());
    }

    if (se_state.key_state == KeyState::Fault || tropic_state.key_state == KeyState::Fault)
        && !command_allowed_when_faulted(input)
    {
        send_host_response(out, "ERROR:KEYSTORE_CORRUPT")?;
        return Ok(());
    }

    if input == "GET_INFO" {
        let auth_store = if pin_auth_object_exists(se_state, i2c)? {
            "PIN_HW_V1"
        } else {
            let session = se_state
                .session
                .as_mut()
                .ok_or_else(|| anyhow!("SE050_UNAVAILABLE"))?;
            let mut legacy = false;
            for id in tropic::LEGACY_PIN_IDS {
                legacy |= session.check_object_exists(i2c, id)?;
            }
            if legacy {
                "LEGACY"
            } else {
                "EMPTY"
            }
        };
        let active_transport = current_active_transport()
            .map(HostCommandSource::as_wire)
            .unwrap_or("IDLE");
        let info = format!(
            "INFO;PROTOCOL=2;BLE_SECURITY=SC_MITM_BOND;SESSION_BOUND=1;VER={};BOARD={};AUTH_MODE={};FINALIZED={};LOCKED={};RETRIES_LEFT={};KEY_STATE={};PQC_STATE={};TX_V1=1;MAX_SIGN_BYTES={};PIN_BACKEND=SE050;SOLANA_BACKEND=TROPIC;PQC_BACKEND=DISABLED;PQC_SUPPORTED=0;TRANSPORTS=USB,BLE;IPHONE_TRANSPORT=BLE;ACTIVE_TRANSPORT={};TROPIC_ACCESS={};ACTIVE_PAIR={};BUILD={};WIPE_PENDING={};RESET_SUPPORTED=0;RESET_POLICY=PERMANENT_LOCKOUT;PIN_ATTEMPT_LIMIT=10;QUALIFICATION={};AUTH_STORE={};USB_HOST_LOCK=1;USB_APP_LOCK=PORT_POLL;USB_APP_TIMEOUT_MS=500;USB_SESSION={};UPTIME_MS={};LOCAL_PIN=1;FORGET_BONDS=LOCAL;SESSION_UNLOCKED={};UNLOCK_REMAINING_SECS={};SIGN_REVIEW=PAGED;BLIND_SIGN=EXPLICIT;REVIEW_TIMEOUT_SECS=60;UI=MONO",
            FW_VER,
            board.profile.as_wire(),
            auth_state.mode.as_wire(),
            bool_to_wire(auth_state.finalized),
            bool_to_wire(auth_state.locked),
            auth_state.retries_left(),
            tropic_state.key_state.as_wire(),
            legacy_pqc_state,
            MAX_SIGN_PAYLOAD_BYTES,
            active_transport,
            tropic_state.access_state.as_str(),
            tropic_state
                .active_pairing_key_index
                .map(|slot| slot.to_string())
                .unwrap_or_else(|| "NONE".to_string()),
            build_info::BUILD_ID,
            bool_to_wire(get_bool_flag(nvs, NVS_WIPE_PENDING)?.unwrap_or(false)),
            bool_to_wire(cfg!(feature = "qualification")),
            auth_store,
            usb_host::session().0,
            monotonic_ms(),
            bool_to_wire(*session_authorized && current_unix(*time_override) <= *unlocked_until),
            if *session_authorized { unlocked_until.saturating_sub(current_unix(*time_override)) } else { 0 },
        );
        let bonds = ble
            .and_then(|transport| transport.bonded_device_count().ok())
            .map(|count| count.to_string())
            .unwrap_or_else(|| "UNKNOWN".into());
        send_host_response(out, &format!("{info};DEVICE_MAC={};BLE_BONDS={bonds}", hardware_mac()))?;
    } else if input == "BUTTON_STATUS" {
        send_host_response(out, &format!("BUTTON:{}", button_state_wire(user_button)))?;
    } else if input == "GET_TIME" {
        let now = current_unix(*time_override);
        send_host_response(out, &format!("TIME:{now}"))?;
    } else if let Some(raw_unix) = input.strip_prefix("SET_TIME:") {
        let unix = match raw_unix.trim().parse::<u64>() {
            Ok(value) => value,
            Err(_) => {
                send_host_response(out, "ERROR:BAD_TIME_FORMAT")?;
                return Ok(());
            }
        };

        if unix < MIN_VALID_UNIX_TIME {
            send_host_response(out, "ERROR:BAD_TIME_VALUE")?;
            return Ok(());
        }

        if !matches!(auth_state.mode, AuthMode::Unset | AuthMode::Otp) {
            send_host_response(out, "ERROR:TIME_NOT_ALLOWED")?;
            return Ok(());
        }

        *time_override = Some(unix);
        send_host_response(out, &format!("TIME_SET:{unix}"))?;
    } else if input == "GET_PUBKEY" {
        match tropic_state.key_state {
            KeyState::Ready => {
                let pubkey = tropic_state
                    .pubkey_base58
                    .as_deref()
                    .ok_or_else(|| anyhow!("missing runtime pubkey"))?;
                send_host_response(out, &format!("PUBKEY:{pubkey}"))?;
            }
            KeyState::Uninitialized => {
                send_host_response(out, "ERROR:WALLET_NOT_INITIALIZED")?;
            }
            KeyState::Fault => {
                send_host_response(out, "ERROR:KEYSTORE_CORRUPT")?;
            }
        }
    } else if input == "SHOW_RECEIVE_QR" {
        match tropic_state.key_state {
            KeyState::Ready => {
                let Some(display) = screen.as_mut() else {
                    send_host_response(out, "ERROR:UNSUPPORTED")?;
                    return Ok(());
                };
                let Some(pubkey) = tropic_state.pubkey_base58.as_deref() else {
                    send_host_response(out, "ERROR:KEYSTORE_CORRUPT")?;
                    return Ok(());
                };

                refresh_receive_qr_override();
                display.update_activity();
                match display.show_qr("RECEIVE", pubkey, "Scan to fund") {
                    Ok(()) => send_host_response(out, "RECEIVE_QR_SHOWN")?,
                    Err(err) => {
                        println!("⚠️  Failed to render receive QR: {err:?}");
                        send_host_response(out, "ERROR:DISPLAY_FAILED")?;
                    }
                }
            }
            KeyState::Uninitialized => {
                send_host_response(out, "ERROR:WALLET_NOT_INITIALIZED")?;
            }
            KeyState::Fault => {
                send_host_response(out, "ERROR:KEYSTORE_CORRUPT")?;
            }
        }
    } else if matches!(input, "HIDE_RECEIVE_QR" | "SHOW_HOME") {
        clear_receive_qr_override();
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            *time_override,
        );
        send_host_response(out, "HOME_SHOWN")?;
    } else if input == "SET_MODE:NONE"
        || input == "SET_MODE:OTP_BEGIN"
        || input == "OTP_BEGIN"
        || input.starts_with("SET_MODE:OTP_CONFIRM:")
        || input.starts_with("OTP_CONFIRM:")
    {
        send_host_response(out, "ERROR:PIN_REQUIRED")?;
    } else if input.starts_with("SET_AUTH:") {
        let Some(pin) = parse_legacy_set_auth_pin(input) else {
            send_host_response(out, "ERROR:PIN_REQUIRED")?;
            return Ok(());
        };
        handle_set_mode_pin(
            out,
            screen,
            user_button,
            i2c,
            nvs,
            se_state,
            tropic_state,
            auth_state,
            unlocked_until,
            session_authorized,
            pin,
        )?;
    } else if let Some(pin_raw) = input.strip_prefix("SET_MODE:PIN:") {
        let pin = pin_raw.split(':').next().unwrap_or("").trim();
        handle_set_mode_pin(
            out,
            screen,
            user_button,
            i2c,
            nvs,
            se_state,
            tropic_state,
            auth_state,
            unlocked_until,
            session_authorized,
            pin,
        )?;
    } else if input == "UNLOCK:DEVICE" {
        revoke_unlock(tropic_state, unlocked_until, session_authorized);
        if auth_state.mode != AuthMode::Pin || !auth_state.finalized {
            send_host_response(out, "ERROR:MODE_UNSET")?;
        } else if let Some(secret) = read_device_pin(out, screen, user_button)? {
            handle_unlock_pin(
                out,
                screen,
                se_state,
                tropic_state,
                i2c,
                nvs,
                auth_state,
                unlocked_until,
                session_authorized,
                *time_override,
                core::str::from_utf8(secret.as_ref())?,
            )?;
        } else {
            render_home_screen(
                screen,
                se_state,
                *auth_state,
                *unlocked_until,
                *time_override,
            );
            send_host_response(out, "ERROR:CONFIRM_TIMEOUT")?;
        }
    } else if let Some(pin_raw) = input
        .strip_prefix("UNLOCK:PIN:")
        .or_else(|| input.strip_prefix("PIN_UNLOCK:"))
        .or_else(|| input.strip_prefix("UNLOCK_PIN:"))
    {
        let pin = pin_raw.split(':').next().unwrap_or("").trim();
        handle_unlock_pin(
            out,
            screen,
            se_state,
            tropic_state,
            i2c,
            nvs,
            auth_state,
            unlocked_until,
            session_authorized,
            *time_override,
            pin,
        )?;
    } else if input.starts_with("UNLOCK:OTP:") || input.starts_with("OTP_UNLOCK:") {
        if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
            send_host_response(out, "ERROR:MODE_UNSET")?;
        } else {
            send_host_response(out, "ERROR:AUTH_MODE_MISMATCH")?;
        }
    } else if input == "GENERATE" {
        handle_generate(
            out,
            screen,
            user_button,
            i2c,
            nvs,
            se_state,
            tropic_state,
            *auth_state,
            unlocked_until,
            *time_override,
        )?;
    } else if matches!(input, "WIPE_KEYS" | "FACTORY_WIPE" | "RESET_DEVICE") {
        // Pairing slots are OTP. The previous reset invalidated all customer
        // slots and could permanently prevent re-provisioning. Refuse before
        // button approval, NVS mutation, key deletion or pairing invalidation.
        send_host_response(out, "ERROR:RESET_UNAVAILABLE")?;
    } else if input == "TROPIC_AUDIT" {
        revoke_unlock(tropic_state, unlocked_until, session_authorized);
        match ensure_tropic_runtime(tropic_state)?.audit_access() {
            Ok(report) => send_host_response(out, &report)?,
            Err(error) => send_host_response(out, &format!("ERROR:AUDIT:{error}"))?,
        }
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            *time_override,
        );
    } else if input == "SE050_STATUS" {
        let status = match se_state.key_state {
            KeyState::Ready => "SE050_OK".to_string(),
            KeyState::Uninitialized => "SE050_BLANK".to_string(),
            KeyState::Fault => match &se_state.last_err {
                Some(err) => format!("SE050_ERR:{err}"),
                None => "SE050_ERR:UNKNOWN".to_string(),
            },
        };
        send_host_response(out, &status)?;
    } else if input == "SE050_REINIT" {
        power_cycle_se050(se_enable)?;
        *se_state = init_se050(i2c, screen);
        let status = match se_state.key_state {
            KeyState::Ready => "SE050_REINIT_OK".to_string(),
            KeyState::Uninitialized => "SE050_REINIT_BLANK".to_string(),
            KeyState::Fault => match &se_state.last_err {
                Some(err) => format!("SE050_REINIT_ERR:{err}"),
                None => "SE050_REINIT_ERR:UNKNOWN".to_string(),
            },
        };
        send_host_response(out, &status)?;
        render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            *time_override,
        );
    } else if let Some(payload_b64) = input.strip_prefix("SIGN:") {
        handle_sign(
            out,
            screen,
            user_button,
            i2c,
            se_state,
            tropic_state,
            *auth_state,
            unlocked_until,
            *time_override,
            payload_b64,
        )?;
    } else if input.starts_with("PQC_") || input.starts_with("SIGN_HYBRID:") {
        send_host_response(out, "ERROR:UNSUPPORTED")?;
    } else if matches!(input, "EXPORT_PRIVKEY" | "SHOW_PRIVKEY") {
        send_host_response(out, "ERROR:UNSUPPORTED")?;
    } else {
        send_host_response(out, "ERROR:Unknown command")?;
    }

    Ok(())
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

pub(crate) fn monotonic_ms() -> u64 {
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

fn confirm_with_button<F>(
    out: &dyn ResponseSink,
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
    if !out.is_active() || user_button.is_none() || screen.is_none() {
        return Ok(false);
    }

    println!(
        "🔘 confirm start: title={title} button={} hold_ms={} timeout_ms={}",
        button_state_wire(user_button),
        hold_ms,
        timeout_ms
    );
    redraw_base(screen)?;

    let deadline = monotonic_ms().saturating_add(timeout_ms as u64);
    let mut hold_started_at: Option<u64> = None;
    let mut showing_progress = false;
    let mut waiting_for_release = button_state_wire(user_button) == "PRESSED";
    let mut logged_waiting_for_release = false;
    let mut logged_hold_started = false;

    loop {
        if !out.is_active() {
            return Ok(false);
        }
        let now = monotonic_ms();
        let pressed = button_is_pressed(
            user_button
                .as_ref()
                .ok_or_else(|| anyhow!("missing user button"))?,
        );

        if waiting_for_release {
            if pressed {
                if !logged_waiting_for_release {
                    println!("🔘 confirm waiting_for_release: title={title}");
                    logged_waiting_for_release = true;
                }
                if let Some(display) = screen.as_mut() {
                    let _ = display.show_lines(title, &["Release button", "then hold to confirm"]);
                    display.update_activity();
                }
            } else {
                waiting_for_release = false;
                logged_waiting_for_release = false;
                redraw_base(screen)?;
            }
        } else if pressed {
            let started_at = *hold_started_at.get_or_insert(now);
            if !logged_hold_started {
                println!("🔘 confirm hold_started: title={title}");
                logged_hold_started = true;
            }
            let held_ms = now.saturating_sub(started_at).min(hold_ms as u64) as u32;
            if let Some(display) = screen.as_mut() {
                let _ = display.show_hold_action_progress(
                    title,
                    subtitle,
                    held_ms,
                    hold_ms,
                    Rgb565::WHITE,
                );
                display.update_activity();
            }
            showing_progress = true;

            if held_ms >= hold_ms {
                println!("🔘 confirm success: title={title}");
                return Ok(true);
            }
        } else {
            hold_started_at = None;
            logged_hold_started = false;
            if showing_progress {
                redraw_base(screen)?;
                showing_progress = false;
            }
        }

        if now >= deadline {
            println!("🔘 confirm timeout: title={title}");
            if let Some(display) = screen.as_mut() {
                let _ = display.show_lines("CANCELLED", &["Confirmation timed out"]);
            }
            FreeRtos::delay_ms(500);
            return Ok(false);
        }

        FreeRtos::delay_ms(20);
    }
}

struct LocalResponseSink<'a> {
    source: Option<HostCommandSource>,
    ble: Option<&'a WalletBleTransport>,
    usb_session: UsbSession,
    response: Option<String>,
}

fn read_device_pin(
    out: &dyn ResponseSink,
    screen: &mut Option<display::Display>,
    user_button: &Option<DynInputPin>,
) -> Result<Option<Zeroizing<[u8; 6]>>> {
    use local_ui::{Button, PinAction, PinEntry};
    let display = screen
        .as_mut()
        .ok_or_else(|| anyhow!("DISPLAY_UNAVAILABLE"))?;
    let button = user_button
        .as_ref()
        .ok_or_else(|| anyhow!("BUTTON_UNAVAILABLE"))?;
    let mut entry = PinEntry::new(monotonic_ms());
    let mut input = Button::new();
    display.show_pin_entry(entry.cursor(), entry.active_digit())?;
    loop {
        if !out.is_active() || entry.expired(monotonic_ms()) {
            return Ok(None);
        }
        if let Some(event) = input.poll(button_is_pressed(button), monotonic_ms()) {
            match entry.input(event, monotonic_ms()) {
                PinAction::Submit(secret) => return Ok(Some(secret)),
                PinAction::Cancel => return Ok(None),
                PinAction::Editing => {
                    display.show_pin_entry(entry.cursor(), entry.active_digit())?
                }
            }
        }
        FreeRtos::delay_ms(20);
    }
}

impl ResponseSink for LocalResponseSink<'_> {
    fn is_active(&self) -> bool {
        self.source
            .map(|source| source_is_active(source, self.ble))
            .unwrap_or_else(|| usb_host::session() == self.usb_session)
    }
    fn send_line(&mut self, line: &str) -> Result<()> {
        anyhow::ensure!(self.is_active(), "HOST_SESSION_CLOSED");
        self.response = Some(line.to_owned());
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn process_local_button_ui(
    ui: &mut local_ui::LocalUi,
    screen: &mut Option<display::Display>,
    user_button: &Option<DynInputPin>,
    ble: Option<&WalletBleTransport>,
    se_state: &mut SeState,
    tropic_state: &mut TropicState,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_authorized: &mut bool,
    time_override: Option<u64>,
    owner: &mut TransportOwner,
) -> Result<()> {
    use local_ui::{Input, Page, PinAction, PinEntry};
    let Some(button) = user_button.as_ref() else {
        return Ok(());
    };
    if screen.is_none() {
        return Ok(());
    }
    let now = monotonic_ms();
    let input = ui.poll(button_is_pressed(button), now);
    let mut submit = None;
    let mut forget = false;
    if let Some(input) = input {
        if input == Input::Cancel {
            ui.close(now);
        } else {
            match &mut ui.page {
                Page::Home => {
                    clear_receive_qr_override();
                    if input == Input::Select {
                        ui.show(Page::Menu(0), now);
                    } else {
                        ui.changed = true;
                    }
                }
                Page::Menu(selected) if input == Input::Tap => {
                    *selected = (*selected + 1) % local_ui::MENU_LEN;
                    ui.changed = true;
                }
                Page::Menu(selected) => match *selected {
                    0 if *session_authorized => {
                        revoke_unlock(tropic_state, unlocked_until, session_authorized);
                        ui.close(now);
                    }
                    0 if auth_state.locked => {
                        ui.show(Page::Notice("LOCKED OUT", "PIN unavailable"), now)
                    }
                    0 if se_state.key_state == KeyState::Fault
                        || tropic_state.key_state == KeyState::Fault =>
                    {
                        ui.show(Page::Notice("WALLET ERROR", "Cannot unlock"), now);
                    }
                    0 if !auth_state.finalized || auth_state.mode != AuthMode::Pin => {
                        ui.show(Page::Notice("SETUP", "Set PIN in app first"), now);
                    }
                    0 => {
                        revoke_unlock(tropic_state, unlocked_until, session_authorized);
                        ui.show(Page::Pin(PinEntry::new(now)), now);
                    }
                    1 => ui.show(Page::Receive, now),
                    2 => ui.show(Page::Forget, now),
                    3 => ui.show(Page::DeviceInfo, now),
                    _ => ui.close(now),
                },
                Page::Pin(pin) => match pin.input(input, now) {
                    PinAction::Editing => ui.changed = true,
                    PinAction::Cancel => ui.close(now),
                    PinAction::Submit(secret) => submit = Some(secret),
                },
                Page::Forget if input == Input::Select => forget = true,
                Page::Forget => ui.show(Page::Menu(2), now),
                Page::Notice(..) | Page::Receive => ui.show(Page::Menu(0), now),
                Page::DeviceInfo => ui.show(Page::Menu(3), now),
            }
        }
    }
    if ui.active() {
        if let Some(transport) = ble {
            transport.set_pairing_available(false);
        }
    }
    if let Some(secret) = submit {
        // Preserve the exact existing host generation, or remain local-only.
        // The dispatcher revokes local-only authorization before assigning a host.
        let mut out = LocalResponseSink {
            source: owner.source,
            ble,
            usb_session: usb_host::session(),
            response: None,
        };
        ui.close(now);
        handle_unlock_pin(
            &mut out,
            screen,
            se_state,
            tropic_state,
            i2c,
            nvs,
            auth_state,
            unlocked_until,
            session_authorized,
            time_override,
            core::str::from_utf8(secret.as_ref())?,
        )?;
        drop(secret);
        if !*session_authorized {
            let notice = match out.response.as_deref() {
                Some("ERROR:AUTH_FAILED") => ("INCORRECT PIN", "An attempt was used"),
                Some("ERROR:AUTH_LOCKED") => ("LOCKED OUT", "PIN unavailable"),
                _ => ("UNLOCK FAILED", "Check wallet status"),
            };
            ui.show(Page::Notice(notice.0, notice.1), monotonic_ms());
        }
    }
    if forget {
        revoke_unlock(tropic_state, unlocked_until, session_authorized);
        *owner = TransportOwner::default();
        set_active_transport(None);
        screen
            .as_mut()
            .unwrap()
            .show_lines("FORGET PHONES", &["Removing pairings..."])?;
        let result = ble
            .ok_or_else(|| anyhow!("BLE unavailable"))?
            .forget_bonded_devices();
        let notice = if result.is_ok() {
            ("PHONES REMOVED", "Pair again to connect")
        } else {
            println!("Bluetooth bond removal failed: {:?}", result.err());
            ("REMOVE FAILED", "Restart and retry")
        };
        ui.show(Page::Notice(notice.0, notice.1), monotonic_ms());
    }
    if !ui.changed {
        return Ok(());
    }
    ui.changed = false;
    let display = screen.as_mut().unwrap();
    display.update_activity();
    match &ui.page {
        Page::Home => render_home_screen(
            screen,
            se_state,
            *auth_state,
            *unlocked_until,
            time_override,
        ),
        Page::Menu(selected) => display.show_local_menu(
            *selected,
            *session_authorized,
            ble.and_then(|transport| transport.bonded_device_count().ok()),
        )?,
        Page::Pin(pin) => display.show_pin_entry(pin.cursor(), pin.active_digit())?,
        Page::DeviceInfo => display.show_device_info(&hardware_mac(), FW_VER)?,
        Page::Forget => display.show_lines(
            "FORGET PHONES",
            &[
                "Remove all",
                "paired phones?",
                "Wallet stays.",
                "Tap: cancel",
                "Hold 1s: forget",
            ],
        )?,
        Page::Notice(title, message) => {
            display.show_lines(title, &[message, "Tap: menu", "Hold 3s: home"])?
        }
        Page::Receive => {
            if let Some(address) = tropic_state.pubkey_base58.as_deref() {
                display.show_qr("RECEIVE", address, "Tap: menu")?;
            } else {
                display.show_lines("RECEIVE", &["No wallet yet", "Set up in app", "Tap: menu"])?;
            }
        }
    }
    Ok(())
}

fn handle_set_mode_pin(
    out: &mut dyn ResponseSink,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &mut SeState,
    tropic_state: &mut TropicState,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_authorized: &mut bool,
    pin: &str,
) -> Result<()> {
    if auth_state.finalized {
        send_host_response(out, "ERROR:MODE_FINAL")?;
        return Ok(());
    }
    if !valid_pin(pin) {
        send_host_response(out, "ERROR:BAD_PIN_FORMAT")?;
        return Ok(());
    }

    if !confirm_with_button(
        out,
        screen,
        user_button,
        |screen| {
            show_action_prompt(
                screen,
                "SET DEVICE PIN",
                &["10 tries. No reset.", "Hold to set PIN"],
            )
        },
        "SET DEVICE PIN",
        "Hold to confirm",
        BUTTON_CONFIRM_GENERATE_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_host_response(out, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, *auth_state, *unlocked_until, None);
        return Ok(());
    }
    anyhow::ensure!(out.is_active(), "HOST_SESSION_CLOSED");

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("PIN SETUP", &["Saving PIN...", "Unlock to create wallet"]);
    }

    // An incomplete enrollment is resumed with the same PIN. Never delete a
    // credential on failure: an OTP write or SE050 write may already have landed.
    revoke_unlock(tropic_state, unlocked_until, session_authorized);
    let mut pin_session: Option<se050::UserIdSession> = None;
    let mut setup_result = (|| -> Result<()> {
        if !pin_auth_object_exists(se_state, i2c)? {
            create_native_pin_auth_object(se_state, i2c, screen, pin)?;
        }
        pin_session = Some(open_native_pin_session(se_state, i2c, screen, pin)?);
        let runtime = ensure_tropic_runtime(tropic_state)?;
        let se_session = se_state
            .session
            .as_mut()
            .ok_or_else(|| anyhow!("missing SE050 session"))?;
        runtime.ensure_access_bundle_after_pin_auth(
            se_session,
            pin_session.as_ref().expect("PIN authenticated"),
            i2c,
            pin,
            false,
        )?;
        Ok(())
    })();
    if let Some(session) = pin_session.take() {
        if let Err(error) = close_or_recover_pin_session(se_state, i2c, &session, "pin setup") {
            setup_result = Err(error);
        }
    }
    if let Err(error) = &setup_result {
        if error.is::<se050_auth::PinRejected>() {
            register_auth_failure(nvs, auth_state)?;
        }
    }
    revoke_unlock(tropic_state, unlocked_until, session_authorized);
    sync_tropic_runtime_state(tropic_state);

    match setup_result {
        Ok(()) => {
            clear_pin_hash(nvs)?;
            clear_otp_state(nvs)?;
            set_mode_finalized(nvs, AuthMode::Pin)?;

            *auth_state = load_auth_state(nvs)?;
            *unlocked_until = 0;
            clear_session_authorization(session_authorized);
            refresh_tropic_state(tropic_state, se_state, i2c)?;

            send_host_response(out, "MODE_SET:PIN")?;
            render_home_screen(screen, se_state, *auth_state, *unlocked_until, None);
        }
        Err(err) => {
            send_host_response(out, &format!("ERROR:{err}"))?;
            render_home_screen(screen, se_state, *auth_state, *unlocked_until, None);
        }
    }

    Ok(())
}

fn handle_unlock_pin(
    out: &mut dyn ResponseSink,
    screen: &mut Option<display::Display>,
    se_state: &mut SeState,
    tropic_state: &mut TropicState,
    i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    auth_state: &mut AuthState,
    unlocked_until: &mut u64,
    session_authorized: &mut bool,
    time_override: Option<u64>,
    pin: &str,
) -> Result<()> {
    revoke_unlock(tropic_state, unlocked_until, session_authorized);
    anyhow::ensure!(out.is_active(), "HOST_SESSION_CLOSED");
    if auth_state.locked {
        return send_host_response(out, "ERROR:AUTH_LOCKED");
    }
    if se_state.key_state == KeyState::Fault || tropic_state.key_state == KeyState::Fault {
        return send_host_response(out, "ERROR:KEYSTORE_CORRUPT");
    }
    let unlock_total_start_ms = monotonic_ms();
    let mut legacy_verify_ms = 0u64;
    let mut native_auth_create_ms = 0u64;
    let mut pin_session_open_ms = 0u64;
    let mut access_bundle_ms = 0u64;
    let mut access_bundle_format = "unresolved";
    let mut locked_session_open_ms = 0u64;
    let mut locked_session_bundle_format = "unresolved";
    let mut se050_wrap_signature_ms = 0u64;
    let mut derive_wrap_key_ms = 0u64;
    let mut decrypt_pairing_priv_ms = 0u64;
    let mut open_bound_session_ms = 0u64;
    let mut pin_session_close_ms = 0u64;
    let mut reused_active_session = false;

    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_host_response(out, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode != AuthMode::Pin {
        send_host_response(out, "ERROR:AUTH_MODE_MISMATCH")?;
        return Ok(());
    }
    if !valid_pin(pin) {
        send_host_response(out, "ERROR:BAD_PIN_FORMAT")?;
        return Ok(());
    }

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("UNLOCK", &["Checking PIN...", "Opening sign window"]);
    }

    let auth_lookup_start_ms = monotonic_ms();
    let native_auth_exists = pin_auth_object_exists(se_state, i2c)?;
    let legacy_pin_present = if native_auth_exists {
        false
    } else {
        legacy_pin_material_present(nvs)?
    };
    let auth_lookup_ms = monotonic_ms().saturating_sub(auth_lookup_start_ms);
    let mut created_native_auth = false;
    let mut migrated_legacy_pin = false;
    let mut pin_session: Option<se050::UserIdSession> = None;

    let mut unlock_result = (|| -> Result<()> {
        if native_auth_exists {
            let pin_session_start_ms = monotonic_ms();
            pin_session = Some(open_native_pin_session(se_state, i2c, screen, pin)?);
            pin_session_open_ms = monotonic_ms().saturating_sub(pin_session_start_ms);
        } else {
            if !legacy_pin_present {
                return Err(anyhow!("AUTH_STORE_MISSING"));
            }
            let legacy_verify_start_ms = monotonic_ms();
            if !verify_pin_hash(nvs, pin)? {
                legacy_verify_ms = monotonic_ms().saturating_sub(legacy_verify_start_ms);
                return Err(se050_auth::PinRejected.into());
            }
            legacy_verify_ms = monotonic_ms().saturating_sub(legacy_verify_start_ms);

            let native_auth_create_start_ms = monotonic_ms();
            create_native_pin_auth_object(se_state, i2c, screen, pin)?;
            native_auth_create_ms = monotonic_ms().saturating_sub(native_auth_create_start_ms);
            created_native_auth = true;
            let pin_session_start_ms = monotonic_ms();
            pin_session = Some(open_native_pin_session(se_state, i2c, screen, pin)?);
            pin_session_open_ms = monotonic_ms().saturating_sub(pin_session_start_ms);
            migrated_legacy_pin = true;
        }

        {
            let runtime = ensure_tropic_runtime(tropic_state)?;
            reused_active_session = runtime.has_active_session();
        }

        if !reused_active_session {
            let access_bundle_start_ms = monotonic_ms();
            let bundle = {
                let runtime = ensure_tropic_runtime(tropic_state)?;
                let se_session = se_state
                    .session
                    .as_mut()
                    .ok_or_else(|| anyhow!("missing SE050 session"))?;
                runtime.ensure_access_bundle_after_pin_auth(
                    se_session,
                    pin_session.as_ref().expect("pin session established"),
                    i2c,
                    pin,
                    created_native_auth,
                )?
            };
            access_bundle_ms = monotonic_ms().saturating_sub(access_bundle_start_ms);
            access_bundle_format = bundle.format_label();

            let locked_session_start_ms = monotonic_ms();
            {
                let runtime = ensure_tropic_runtime(tropic_state)?;
                let se_session = se_state
                    .session
                    .as_mut()
                    .ok_or_else(|| anyhow!("missing SE050 session"))?;
                let metrics = runtime
                    .open_locked_session(se_session, i2c, &bundle, pin)
                    .map_err(|err| anyhow!("UNLOCK_SESSION:{err}"))?;
                locked_session_bundle_format = metrics.bundle_format;
                se050_wrap_signature_ms = metrics.se050_wrap_signature_ms;
                derive_wrap_key_ms = metrics.derive_wrap_key_ms;
                decrypt_pairing_priv_ms = metrics.decrypt_pairing_priv_ms;
                open_bound_session_ms = metrics.open_bound_session_ms;
            }
            locked_session_open_ms = monotonic_ms().saturating_sub(locked_session_start_ms);
        } else {
            access_bundle_format = "reused-active-session";
            locked_session_bundle_format = "reused-active-session";
            println!("🔐 UNLOCK fast-path: reusing active Tropic session after PIN verify");
        }

        sync_tropic_runtime_state(tropic_state);
        Ok(())
    })();

    if let Some(session) = pin_session.take() {
        let pin_session_close_start_ms = monotonic_ms();
        if let Err(close_err) = close_or_recover_pin_session(se_state, i2c, &session, "pin unlock")
        {
            if unlock_result.is_ok() {
                unlock_result = Err(close_err);
            }
        }
        pin_session_close_ms = monotonic_ms().saturating_sub(pin_session_close_start_ms);
    }

    match unlock_result {
        Ok(()) => {
            if !out.is_active() {
                revoke_unlock(tropic_state, unlocked_until, session_authorized);
                return Err(anyhow!("HOST_SESSION_CLOSED"));
            }
            clear_auth_failures(nvs, auth_state)?;
            if migrated_legacy_pin {
                clear_pin_hash(nvs)?;
            }
            *unlocked_until = current_unix(time_override) + UNLOCK_WINDOW_SECS;
            *session_authorized = true;
            println!(
                "🔐 UNLOCK timing: result=ok native_auth={} legacy_pin_present={} created_native_auth={} reused_active_session={} access_bundle_format={} locked_session_bundle_format={} auth_lookup_ms={} legacy_verify_ms={} native_auth_create_ms={} pin_session_open_ms={} access_bundle_ms={} se050_wrap_signature_ms={} derive_wrap_key_ms={} decrypt_pairing_priv_ms={} open_bound_session_ms={} locked_session_open_ms={} pin_session_close_ms={} total_ms={}",
                native_auth_exists,
                legacy_pin_present,
                created_native_auth,
                reused_active_session,
                access_bundle_format,
                locked_session_bundle_format,
                auth_lookup_ms,
                legacy_verify_ms,
                native_auth_create_ms,
                pin_session_open_ms,
                access_bundle_ms,
                se050_wrap_signature_ms,
                derive_wrap_key_ms,
                decrypt_pairing_priv_ms,
                open_bound_session_ms,
                locked_session_open_ms,
                pin_session_close_ms,
                monotonic_ms().saturating_sub(unlock_total_start_ms),
            );
            send_host_response(out, &format!("UNLOCKED_UNTIL:{}", *unlocked_until))?;
        }
        Err(err) => {
            let err_msg = err.to_string();
            println!(
                "🔐 UNLOCK timing: result=err err={} native_auth={} legacy_pin_present={} created_native_auth={} reused_active_session={} access_bundle_format={} locked_session_bundle_format={} auth_lookup_ms={} legacy_verify_ms={} native_auth_create_ms={} pin_session_open_ms={} access_bundle_ms={} se050_wrap_signature_ms={} derive_wrap_key_ms={} decrypt_pairing_priv_ms={} open_bound_session_ms={} locked_session_open_ms={} pin_session_close_ms={} total_ms={}",
                err_msg,
                native_auth_exists,
                legacy_pin_present,
                created_native_auth,
                reused_active_session,
                access_bundle_format,
                locked_session_bundle_format,
                auth_lookup_ms,
                legacy_verify_ms,
                native_auth_create_ms,
                pin_session_open_ms,
                access_bundle_ms,
                se050_wrap_signature_ms,
                derive_wrap_key_ms,
                decrypt_pairing_priv_ms,
                open_bound_session_ms,
                locked_session_open_ms,
                pin_session_close_ms,
                monotonic_ms().saturating_sub(unlock_total_start_ms),
            );
            let auth_failed = err.is::<se050_auth::PinRejected>();
            if auth_failed {
                register_auth_failure(nvs, auth_state)?;
                if auth_state.locked {
                    *unlocked_until = 0;
                    clear_session_authorization(session_authorized);
                    if let Some(runtime) = tropic_state.runtime.as_mut() {
                        runtime.close_active_session();
                    }
                    sync_tropic_runtime_state(tropic_state);
                    send_host_response(out, "ERROR:AUTH_LOCKED")?;
                } else {
                    send_host_response(out, "ERROR:AUTH_FAILED")?;
                }
            } else {
                if let Some(runtime) = tropic_state.runtime.as_mut() {
                    runtime.close_active_session();
                }
                sync_tropic_runtime_state(tropic_state);
                send_host_response(out, &format!("ERROR:{err_msg}"))?;
            }
        }
    }

    render_home_screen(
        screen,
        se_state,
        *auth_state,
        *unlocked_until,
        time_override,
    );

    Ok(())
}

fn handle_generate(
    out: &mut dyn ResponseSink,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    _i2c: &mut I2cDriver,
    nvs: &mut EspNvs<NvsDefault>,
    se_state: &SeState,
    tropic_state: &mut TropicState,
    auth_state: AuthState,
    unlocked_until: &mut u64,
    time_override: Option<u64>,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode != AuthMode::Pin {
        send_host_response(out, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if tropic_state.key_state == KeyState::Ready {
        send_host_response(out, "ERROR:WALLET_ALREADY_INITIALIZED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > *unlocked_until {
        send_host_response(out, "ERROR:LOCKED")?;
        return Ok(());
    }
    if !ensure_tropic_runtime(tropic_state)?.has_active_session() {
        send_host_response(out, "ERROR:LOCKED")?;
        return Ok(());
    }

    if !confirm_with_button(
        out,
        screen,
        user_button,
        |screen| {
            show_action_prompt(
                screen,
                "CREATE WALLET",
                &["Hold button", "Create new wallet"],
            )
        },
        "CREATE WALLET",
        "Hold to confirm",
        BUTTON_CONFIRM_GENERATE_HOLD_MS,
        BUTTON_CONFIRM_TIMEOUT_MS,
    )? {
        send_host_response(out, "ERROR:CONFIRM_TIMEOUT")?;
        render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        return Ok(());
    }
    anyhow::ensure!(out.is_active(), "HOST_SESSION_CLOSED");
    if auth_state.requires_unlock_window() && current_unix(time_override) > *unlocked_until {
        send_host_response(out, "ERROR:LOCKED")?;
        return Ok(());
    }

    let generate_result = (|| -> Result<String> {
        let runtime = ensure_tropic_runtime(tropic_state)?;
        let pubkey = runtime.ensure_wallet_key_initialized()?;
        sync_tropic_runtime_state(tropic_state);
        Ok(bs58::encode(pubkey).into_string())
    })();

    match generate_result {
        Ok(pubkey) => {
            *unlocked_until = 0;
            send_host_response(out, "KEY_GENERATED")?;
            send_host_response(out, &format!("PUBKEY:{pubkey}"))?;
            let auth_state = load_auth_state(nvs)?;
            render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        }
        Err(err) => {
            mark_tropic_fault(tropic_state, format!("KEYGEN:{err}"));
            send_host_response(out, &format!("ERROR:{err}"))?;
            render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        }
    }

    Ok(())
}

fn handle_sign(
    out: &mut dyn ResponseSink,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    _i2c: &mut I2cDriver,
    se_state: &mut SeState,
    tropic_state: &mut TropicState,
    auth_state: AuthState,
    unlocked_until: &mut u64,
    time_override: Option<u64>,
    payload_b64: &str,
) -> Result<()> {
    if !auth_state.finalized || auth_state.mode == AuthMode::Unset {
        send_host_response(out, "ERROR:MODE_UNSET")?;
        return Ok(());
    }
    if auth_state.mode == AuthMode::Otp {
        send_host_response(out, "ERROR:PIN_REQUIRED")?;
        return Ok(());
    }
    if auth_state.requires_unlock_window() && current_unix(time_override) > *unlocked_until {
        send_host_response(out, "ERROR:LOCKED")?;
        return Ok(());
    }
    if tropic_state.key_state == KeyState::Uninitialized {
        send_host_response(out, "ERROR:WALLET_NOT_INITIALIZED")?;
        return Ok(());
    }
    if !ensure_tropic_runtime(tropic_state)?.has_active_session() {
        send_host_response(out, "ERROR:LOCKED")?;
        return Ok(());
    }

    let message_bytes = match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
        Ok(bytes) => bytes,
        Err(_) => {
            send_host_response(out, "ERROR:Invalid base64 encoding")?;
            return Ok(());
        }
    };
    let now = current_unix(time_override);
    println!(
        "🔐 SIGN gate: finalized={} mode={} now={} unlocked_until={} window_open={} button={} message_bytes={}",
        auth_state.finalized,
        auth_state.mode.as_wire(),
        now,
        *unlocked_until,
        now <= *unlocked_until,
        button_state_wire(user_button),
        message_bytes.len()
    );
    println!(
        "🔐 SIGN request: message_bytes={} b64_chars={} host_limit={} tropic_session_open={}",
        message_bytes.len(),
        payload_b64.len(),
        MAX_SIGN_PAYLOAD_BYTES,
        ensure_tropic_runtime(tropic_state)?.has_active_session(),
    );
    if message_bytes.len() > MAX_SIGN_PAYLOAD_BYTES {
        println!(
            "⚠️  SIGN payload exceeds host limit: message_bytes={} host_limit={}",
            message_bytes.len(),
            MAX_SIGN_PAYLOAD_BYTES
        );
        send_host_response(out, "ERROR:PAYLOAD_TOO_LARGE")?;
        return Ok(());
    }

    let review = signing_review::Review::from_message(
        &message_bytes,
        tropic_state
            .pubkey_base58
            .as_deref()
            .ok_or_else(|| anyhow!("WALLET_NOT_INITIALIZED"))?,
    );
    println!(
        "SIGN review: blind={} pages={}",
        review.blind,
        review.pages.len()
    );
    let decision = review_signing_with_button(
        out,
        screen,
        user_button,
        &review,
        *unlocked_until,
        time_override,
    )?;
    anyhow::ensure!(out.is_active(), "HOST_SESSION_CLOSED");
    if auth_state.requires_unlock_window() && current_unix(time_override) > *unlocked_until {
        send_host_response(out, "ERROR:LOCKED")?;
        render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        return Ok(());
    }
    if let Some(error) = decision.error() {
        send_host_response(out, error)?;
        render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
        return Ok(());
    }

    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("SIGNING", &["Creating signature..."]);
    }

    let sign_result = {
        let runtime = ensure_tropic_runtime(tropic_state)?;
        runtime.sign_active(&message_bytes)
    };
    let signature = match sign_result {
        Ok(signature) => {
            println!(
                "✅ TROPIC sign success: message_bytes={} signature_bytes={}",
                message_bytes.len(),
                signature.len()
            );
            signature
        }
        Err(err) => {
            println!(
                "⚠️  TROPIC sign failed: message_bytes={} error={err}",
                message_bytes.len()
            );
            if let Some(runtime) = tropic_state.runtime.as_mut() {
                runtime.close_active_session();
            }
            sync_tropic_runtime_state(tropic_state);
            if let Some(display) = screen.as_mut() {
                let _ = display.show_lines("SIGN FAILED", &["Resetting transport", "Try again"]);
            }
            send_host_response(out, &format!("ERROR:Sign failed: {err}"))?;
            render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
            return Ok(());
        }
    };

    refresh_unlock_window_on_activity(auth_state, unlocked_until, true, time_override);
    sync_tropic_runtime_state(tropic_state);
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature);
    send_host_response(out, &format!("SIGNATURE:{encoded}"))?;
    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines(
            "SIGNED",
            &[
                "Signature returned",
                "to your app.",
                "Check app for",
                "transaction status.",
            ],
        );
    }
    FreeRtos::delay_ms(750);

    render_home_screen(screen, se_state, auth_state, *unlocked_until, time_override);
    Ok(())
}

fn revoke_unlock(
    tropic_state: &mut TropicState,
    unlocked_until: &mut u64,
    session_authorized: &mut bool,
) {
    *unlocked_until = 0;
    clear_session_authorization(session_authorized);
    if let Some(runtime) = tropic_state.runtime.as_mut() {
        runtime.close_active_session();
    }
    sync_tropic_runtime_state(tropic_state);
}

fn clear_session_authorization(session_authorized: &mut bool) {
    *session_authorized = false;
}

fn refresh_unlock_window_on_activity(
    auth_state: AuthState,
    unlocked_until: &mut u64,
    session_ready: bool,
    time_override: Option<u64>,
) {
    if session_ready && auth_state.requires_unlock_window() {
        *unlocked_until = current_unix(time_override) + UNLOCK_WINDOW_SECS;
    }
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

fn mark_tropic_fault(tropic_state: &mut TropicState, err: impl Into<String>) {
    tropic_state.key_state = KeyState::Fault;
    tropic_state.last_err = Some(err.into());
    sync_tropic_home_snapshot(tropic_state);
}

fn sync_tropic_runtime_state(tropic_state: &mut TropicState) {
    if let Some(runtime) = tropic_state.runtime.as_ref() {
        tropic_state.pubkey_raw = runtime.wallet_pubkey();
        tropic_state.pubkey_base58 = runtime.wallet_pubkey_base58();
        tropic_state.key_state = if tropic_state.pubkey_raw.is_some() {
            KeyState::Ready
        } else {
            KeyState::Uninitialized
        };
        tropic_state.access_state = runtime.access_state().as_wire().to_string();
        tropic_state.active_pairing_key_index = runtime.active_pairing_key_index();
        if tropic_state.key_state != KeyState::Fault {
            tropic_state.last_err = None;
        }
    }

    sync_tropic_home_snapshot(tropic_state);
}

fn init_tropic(
    spi3: esp_idf_svc::hal::spi::SPI3,
    pins: Option<board::TropicPins>,
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
) -> TropicState {
    if let Some(display) = screen.as_mut() {
        let _ = display.show_lines("TROPIC", &["Initializing..."]);
    }

    let Some(pins) = pins else {
        let state = TropicState {
            runtime: None,
            pubkey_raw: None,
            pubkey_base58: None,
            key_state: KeyState::Fault,
            access_state: "UNAVAILABLE".to_string(),
            active_pairing_key_index: None,
            last_err: Some("UNAVAILABLE".to_string()),
        };
        sync_tropic_home_snapshot(&state);
        return state;
    };

    match tropic::Runtime::new(spi3, pins) {
        Ok(mut runtime) => {
            if let Some(session) = se_state.session.as_mut() {
                if let Ok(access_state) = runtime.probe_access_state(session, i2c) {
                    let _ = access_state;
                }
            }
            let mut state = TropicState {
                runtime: Some(runtime),
                pubkey_raw: None,
                pubkey_base58: None,
                key_state: KeyState::Uninitialized,
                access_state: "UNKNOWN".to_string(),
                active_pairing_key_index: None,
                last_err: None,
            };
            sync_tropic_runtime_state(&mut state);
            state
        }
        Err(err) => {
            let state = TropicState {
                runtime: None,
                pubkey_raw: None,
                pubkey_base58: None,
                key_state: KeyState::Fault,
                access_state: "FAULT".to_string(),
                active_pairing_key_index: None,
                last_err: Some(format!("INIT:{err}")),
            };
            sync_tropic_home_snapshot(&state);
            state
        }
    }
}

fn refresh_tropic_state(
    tropic_state: &mut TropicState,
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
) -> Result<()> {
    let runtime = tropic_state
        .runtime
        .as_mut()
        .ok_or_else(|| anyhow!("TROPIC_NOT_READY"))?;
    runtime.refresh_wallet_pubkey()?;
    if let Some(session) = se_state.session.as_mut() {
        let _ = runtime.probe_access_state(session, i2c);
    }
    sync_tropic_runtime_state(tropic_state);
    Ok(())
}

fn ensure_tropic_runtime(tropic_state: &mut TropicState) -> Result<&mut tropic::Runtime> {
    tropic_state
        .runtime
        .as_mut()
        .ok_or_else(|| anyhow!("TROPIC_NOT_READY"))
}

fn pin_auth_object_exists(se_state: &mut SeState, i2c: &mut I2cDriver) -> Result<bool> {
    let mut screen = None;
    ensure_se050_session(se_state, i2c, &mut screen)?;
    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    session.check_object_exists(i2c, tropic::SE050_PIN_AUTH_ID)
}

fn create_native_pin_auth_object(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
    pin: &str,
) -> Result<()> {
    ensure_se050_session(se_state, i2c, screen)?;
    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    let policy = se050_policy::pin_object_policy(tropic::SE050_PIN_AUTH_ID);
    for legacy in tropic::LEGACY_PIN_IDS {
        anyhow::ensure!(
            !session.check_object_exists(i2c, legacy)?,
            "LEGACY_PIN_MIGRATION_REQUIRED"
        );
    }
    session.write_userid_auth_object(
        i2c,
        tropic::SE050_PIN_AUTH_ID,
        pin.as_bytes(),
        tropic::PIN_ATTEMPT_LIMIT,
        &policy,
    )
}

fn open_native_pin_session(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    screen: &mut Option<display::Display>,
    pin: &str,
) -> Result<se050::UserIdSession> {
    ensure_se050_session(se_state, i2c, screen)?;
    let session = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    session.create_userid_session(i2c, tropic::SE050_PIN_AUTH_ID, pin.as_bytes())
}

fn close_or_recover_pin_session(
    se_state: &mut SeState,
    i2c: &mut I2cDriver,
    session: &se050::UserIdSession,
    context: &str,
) -> Result<()> {
    let base = se_state
        .session
        .as_mut()
        .ok_or_else(|| anyhow!("missing SE050 session"))?;
    match session.close(base, i2c) {
        Ok(()) => Ok(()),
        Err(close_err) => base.recover_link(i2c).map_err(|recover_err| {
            anyhow!("PIN_SESSION_CLOSE:{context}:{close_err};RECOVER:{recover_err}")
        }),
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

    println!(
        "🔐 SE050 session init: key_state={} last_err={}",
        se_state.key_state.as_wire(),
        se_state.last_err.as_deref().unwrap_or("none")
    );
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

fn render_home_screen(
    screen: &mut Option<display::Display>,
    se_state: &SeState,
    auth_state: AuthState,
    _unlocked_until: u64,
    _time_override: Option<u64>,
) {
    if receive_qr_override_active() {
        return;
    }

    let Some(display) = screen.as_mut() else {
        return;
    };
    let transport_note = current_transport_note();
    let tropic_state = current_tropic_home_snapshot();

    if se_state.key_state == KeyState::Fault {
        let line = se_state.last_err.as_deref().unwrap_or("SE050 auth fault");
        let mut lines = vec![line.to_string()];
        if let Some(note) = transport_note.as_deref() {
            lines.push(note.to_string());
        }
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let _ = display.show_lines("SE050", &line_refs);
        return;
    }

    match tropic_state.key_state {
        KeyState::Fault => {
            let line = tropic_state.last_err.as_deref().unwrap_or("Tropic fault");
            let mut lines = vec![line.to_string()];
            if let Some(note) = transport_note.as_deref() {
                lines.push(note.to_string());
            }
            let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let _ = display.show_lines("TROPIC", &line_refs);
        }
        KeyState::Uninitialized => {
            let mut lines = if auth_state.finalized && auth_state.mode == AuthMode::Pin {
                vec![
                    "PIN stored securely".to_string(),
                    setup_action_hint().to_string(),
                ]
            } else {
                vec![
                    "Set a 6-digit PIN in app".to_string(),
                    "Tropic wallet not initialized".to_string(),
                ]
            };
            if let Some(note) = transport_note.as_deref() {
                lines.push(note.to_string());
            }
            let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let _ = display.show_lines("SETUP", &line_refs);
        }
        KeyState::Ready => {
            if auth_state.locked {
                let _ = display.show_lines(
                    "LOCKED OUT",
                    &["PIN attempts ended.", "This wallet cannot", "be recovered."],
                );
            } else if !auth_state.finalized || auth_state.mode != AuthMode::Pin {
                let _ = display.show_lines("SETUP", &["Finish PIN setup", "from your app."]);
            } else {
                let _ = display.show_screensaver();
            }
        }
    }
}

fn setup_action_hint() -> &'static str {
    "Create wallet from app"
}

fn review_signing_with_button(
    out: &dyn ResponseSink,
    screen: &mut Option<display::Display>,
    user_button: &mut Option<DynInputPin>,
    review: &signing_review::Review,
    unlock_deadline: u64,
    time_override: Option<u64>,
) -> Result<signing_review::Decision> {
    use signing_review::{Button, Decision, Flow, Outcome, REVIEW_TIMEOUT_MS};
    if !out.is_active() || screen.is_none() || user_button.is_none() {
        return Ok(Decision::TimedOut);
    }
    let deadline = monotonic_ms().saturating_add(REVIEW_TIMEOUT_MS);
    let mut button = Button::new();
    let mut flow = Flow::new();
    let mut last_frame = None;
    let mut last_hold_bucket = None;
    loop {
        let now = monotonic_ms();
        if !out.is_active() || current_unix(time_override) > unlock_deadline {
            return Ok(Decision::TimedOut);
        }
        if now >= deadline {
            screen.as_mut().unwrap().show_lines(
                "REVIEW EXPIRED",
                &["No signature made.", "Try again in app."],
            )?;
            FreeRtos::delay_ms(750);
            return Ok(Decision::TimedOut);
        }
        let input = button.poll(
            button_is_pressed(user_button.as_ref().unwrap()),
            now,
            review.pages[flow.index()].kind,
        );
        if let Some(input) = input {
            match flow.input(review, input) {
                Outcome::Approved => return Ok(Decision::Approved),
                Outcome::Rejected => {
                    screen
                        .as_mut()
                        .unwrap()
                        .show_lines("REJECTED", &["No signature", "was created."])?;
                    FreeRtos::delay_ms(750);
                    return Ok(Decision::Rejected);
                }
                Outcome::Reviewing => {}
            }
            last_frame = None;
            last_hold_bucket = None;
        }
        let seconds = deadline.saturating_sub(now).div_ceil(1000);
        let frame = (flow.index(), seconds);
        let held_ms = button.held_ms(now);
        if held_ms == 0 && last_hold_bucket.is_some() {
            last_frame = None;
            last_hold_bucket = None;
        }
        if last_frame.map(|(index, _)| index) != Some(flow.index()) {
            let render_start = monotonic_ms();
            screen.as_mut().unwrap().show_review_page(
                review,
                flow.index(),
                seconds,
                flow.choice(),
            )?;
            println!(
                "SIGN review page={} render_ms={}",
                flow.index(),
                monotonic_ms().saturating_sub(render_start)
            );
            last_frame = Some(frame);
            last_hold_bucket = None;
        } else if last_frame != Some(frame) {
            screen
                .as_mut()
                .unwrap()
                .show_review_timer(&review.pages[flow.index()], seconds)?;
            last_frame = Some(frame);
        }
        if held_ms >= 50 && last_hold_bucket != Some(held_ms / 50) {
            screen.as_mut().unwrap().show_review_hold(
                &review.pages[flow.index()],
                held_ms,
                flow.choice(),
            )?;
            last_hold_bucket = Some(held_ms / 50);
        }
        FreeRtos::delay_ms(20);
    }
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
    let locked = get_bool_flag(nvs, NVS_AUTH_LOCKED)?.unwrap_or(false)
        || fail_count >= MAX_AUTH_ATTEMPTS
        || get_bool_flag(nvs, NVS_WIPE_PENDING)?.unwrap_or(false);

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
            | "SHOW_RECEIVE_QR"
            | "HIDE_RECEIVE_QR"
            | "SHOW_HOME"
            | "GET_TIME"
            | "PQC_STATUS"
            | "PQC_GET_PUBKEY"
            | "PQC_GET_BINDING"
            | "SE050_STATUS"
            | "TROPIC_AUDIT"
            | "SE050_REINIT"
            | "WIPE_KEYS"
            | "FACTORY_WIPE"
            | "RESET_DEVICE"
    ) || cmd.starts_with("SET_TIME:")
}

fn command_preserves_receive_qr(cmd: &str) -> bool {
    matches!(
        cmd,
        "PING"
            | "GET_INFO"
            | "BUTTON_STATUS"
            | "GET_PUBKEY"
            | "SHOW_RECEIVE_QR"
            | "GET_TIME"
            | "PQC_STATUS"
            | "PQC_GET_PUBKEY"
            | "PQC_GET_BINDING"
            | "SE050_STATUS"
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

trait ResponseSink {
    fn is_active(&self) -> bool {
        true
    }
    fn send_line(&mut self, response: &str) -> Result<()>;
}

struct UsbResponseSink<'a, 'd> {
    uart: &'a mut UsbSerialDriver<'d>,
    session: UsbSession,
}

impl ResponseSink for UsbResponseSink<'_, '_> {
    fn is_active(&self) -> bool {
        usb_host::is_active(self.session)
    }

    fn send_line(&mut self, response: &str) -> Result<()> {
        write_usb_response(response, |bytes| {
            anyhow::ensure!(self.is_active(), "HOST_SESSION_CLOSED");
            let written = self.uart.write(bytes, TickType::new_millis(10).ticks())?;
            anyhow::ensure!(self.is_active(), "HOST_SESSION_CLOSED");
            if written == 0 {
                FreeRtos::delay_ms(5);
            }
            Ok(written)
        })
    }
}

struct BleResponseSink<'a> {
    ble: &'a WalletBleTransport,
    generation: u64,
    responses: Vec<String>,
}

impl BleResponseSink<'_> {
    fn flush(&mut self) -> Result<()> {
        for response in self.responses.drain(..) {
            self.ble.send_response(self.generation, &response)?;
        }
        Ok(())
    }
}

impl ResponseSink for BleResponseSink<'_> {
    fn is_active(&self) -> bool {
        self.ble.is_session_active(self.generation)
    }
    fn send_line(&mut self, response: &str) -> Result<()> {
        anyhow::ensure!(self.is_active(), "HOST_SESSION_CLOSED");
        self.responses.push(response.to_owned());
        Ok(())
    }
}

fn summarize_host_command(input: &str) -> String {
    input
        .split(':')
        .next()
        .unwrap_or("CMD")
        .trim()
        .chars()
        .take(24)
        .collect()
}

fn send_host_response(out: &mut dyn ResponseSink, response: &str) -> Result<()> {
    out.send_line(response)
}

// Boot diagnostics only, before there is a host command or wallet session.
fn send_response(uart: &mut UsbSerialDriver, response: &str) -> Result<()> {
    write_all_line(response, |bytes| {
        Ok(uart.write(bytes, TickType::new_millis(250).ticks())?)
    })
}

fn discard_usb_input(uart: &mut UsbSerialDriver) -> Result<bool> {
    let mut scratch = Zeroizing::new([0u8; 256]);
    // Bounded work even if a host continuously sends. Until fully drained we
    // leave the receive session unaccepted and retry on the next loop.
    for _ in 0..(2 * (MAX_CMD_LEN + 256) / 256 + 1) {
        match uart.read(&mut scratch[..], 0) {
            Ok(0) => return Ok(true),
            Ok(_) => {}
            Err(err) if err.code() == esp_idf_sys::ESP_ERR_TIMEOUT => return Ok(true),
            Err(err) => return Err(err.into()),
        }
    }
    Ok(false)
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

fn legacy_pin_material_present(nvs: &mut EspNvs<NvsDefault>) -> Result<bool> {
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

    Ok(salt_ok && hash_ok)
}

fn verify_pin_hash(nvs: &mut EspNvs<NvsDefault>, pin: &str) -> Result<bool> {
    if !legacy_pin_material_present(nvs)? {
        return Ok(false);
    }

    let mut salt = [0u8; PIN_SALT_BYTES];
    let mut stored_hash = [0u8; PIN_HASH_BYTES];
    let _ = nvs.get_raw(NVS_PIN_SALT, &mut salt)?;
    let _ = nvs.get_raw(NVS_PIN_HASH, &mut stored_hash)?;

    let computed = derive_pin_hash(pin, &salt);
    Ok(computed.ct_eq(&stored_hash).into())
}
