#[path = "../board.rs"]
mod board;
#[path = "../build_info.rs"]
mod build_info;
#[allow(dead_code)]
#[path = "../display.rs"]
mod display;
#[allow(dead_code)]
#[path = "../review_ui.rs"]
mod review_ui;
#[allow(dead_code)]
#[path = "../signing_review.rs"]
mod signing_review;

use board::active_board_config;
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{AnyIOPin, Input, Output, PinDriver, Pull},
    i2c::{I2cConfig, I2cDriver},
    peripherals::Peripherals,
    prelude::*,
    spi::{
        config::{Config as SpiDeviceConfig, DriverConfig as SpiDriverConfig, MODE_0},
        SpiDeviceDriver, SpiDriver,
    },
};

const SE050_ADDR: u8 = 0x48;
const SE050_SOFT_RESET: [u8; 5] = [0x5A, 0xCF, 0x00, 0x37, 0x7F];
const SE050_SELECT_ISD: [u8; 22] = [
    0x00, 0xA4, 0x04, 0x00, 0x10, 0xA0, 0x00, 0x00, 0x03, 0x96, 0x54, 0x53, 0x00, 0x00, 0x00, 0x01,
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const SE050_HOST_NAD: u8 = 0x5A;
const SE050_SE_NAD: u8 = 0xA5;

const TROPIC_REQ_ID_GET_RESPONSE: u8 = 0xAA;
const TROPIC_STATUS_NO_RESP: u8 = 0xFF;
const TROPIC_STATUS_UNKNOWN_REQ: u8 = 0x7E;
const TROPIC_UNKNOWN_REQUEST_ID: u8 = 0x00;
const TROPIC_MAX_L2_RSP: usize = 260;

const BUTTON_TIMEOUT_MS: u64 = 30_000;
const BUTTON_RELEASE_TIMEOUT_MS: u64 = 5_000;
const BUTTON_DEBOUNCE_MS: u64 = 50;
const MIN_FACTORY_HEAP_BYTES: u32 = 32 * 1024;
const MIN_FLASH_BYTES: u32 = 4 * 1024 * 1024;

type DynOutputPin = PinDriver<'static, AnyIOPin, Output>;
type DynInputPin = PinDriver<'static, AnyIOPin, Input>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckState {
    Pass,
    Fail,
    NotFitted,
    Unconfirmed,
}

impl CheckState {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotFitted => "NOT_FITTED",
            Self::Unconfirmed => "UNCONFIRMED",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Esp32ProbeSummary {
    heap_ok: bool,
    flash_ok: bool,
    identity_ok: bool,
}

impl Esp32ProbeSummary {
    fn passed(self) -> bool {
        self.heap_ok && self.flash_ok && self.identity_ok
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Se050ProbeSummary {
    power_control_ok: bool,
    reset_frame_ok: bool,
    select_apdu_ok: bool,
}

impl Se050ProbeSummary {
    fn passed(self) -> bool {
        self.power_control_ok && self.reset_frame_ok && self.select_apdu_ok
    }
}

fn main() {
    esp_idf_sys::link_patches();
    unsafe {
        esp_idf_sys::esp_log_level_set(b"*\0".as_ptr(), esp_idf_sys::esp_log_level_t_ESP_LOG_NONE);
    }

    let board = active_board_config();
    println!("\n==== UNRUGGABLE FACTORY PCB TEST ====");
    println!("Firmware: factory-test {}", env!("CARGO_PKG_VERSION"));
    println!("Build: {}", build_info::BUILD_ID);
    println!("Board: {} ({})", board.profile.as_wire(), board.description);
    println!(
        "Pins: SE050 SDA={} SCL={} EN={} | BUTTON={}",
        board.se050.sda,
        board.se050.scl,
        board
            .se050
            .enable
            .map(|pin| pin.to_string())
            .unwrap_or_else(|| "none".to_string()),
        board
            .user_button
            .map(|pin| pin.to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
    println!("This test does not create, delete, or modify secure-element objects.");
    println!("--------------------------------------");

    let peripherals = Peripherals::take().expect("ESP32 peripherals already taken");
    let esp32 = probe_esp32();

    let (se050_enable, se050_power_ok) = setup_se050_power(board.se050.enable);
    let se050 = probe_se050(
        peripherals.i2c0,
        board.se050.sda,
        board.se050.scl,
        se050_power_ok,
    );

    let tropic_state = match board.tropic {
        Some(pins) => probe_tropic(peripherals.spi3, pins.csn, pins.sck, pins.miso, pins.mosi),
        None => {
            println!("[TROPIC] No footprint configured for this board profile");
            CheckState::NotFitted
        }
    };

    let mut screen = match board.display {
        Some(pins) => match display::Display::new(peripherals.spi2, pins) {
            Ok(mut display) => match display.show_factory_test_pattern() {
                Ok(()) => {
                    println!("[DISPLAY] Pattern drawn; operator confirmation required");
                    Some(display)
                }
                Err(error) => {
                    println!("[DISPLAY] Pattern draw failed: {error:?}");
                    None
                }
            },
            Err(error) => {
                println!("[DISPLAY] Initialization failed: {error:?}");
                None
            }
        },
        None => {
            println!("[DISPLAY] No display configured for this board profile");
            None
        }
    };

    let mut button = setup_button(board.user_button);
    let button_state = test_button(button.as_mut());
    let display_state = if screen.is_none() {
        CheckState::Fail
    } else if button_state == CheckState::Pass {
        // The button prompt explicitly asks the operator to press only if the
        // RGBW pattern, text, and one-pixel border are visually correct.
        CheckState::Pass
    } else {
        CheckState::Unconfirmed
    };

    let esp32_state = state_for(esp32.passed());
    let se050_state = state_for(se050.passed());
    let core_passed = esp32_state == CheckState::Pass
        && se050_state == CheckState::Pass
        && display_state == CheckState::Pass
        && button_state == CheckState::Pass;
    let pcb_result = if core_passed && tropic_state == CheckState::Pass {
        "PASS"
    } else if core_passed && tropic_state == CheckState::NotFitted {
        "PASS_PRE_TROPIC"
    } else {
        "FAIL"
    };

    println!("--------------------------------------");
    println!(
        "FACTORY_RESULT PCB={} ESP32={} SE050={} DISPLAY={} BUTTON={} TROPIC={}",
        pcb_result,
        esp32_state.as_wire(),
        se050_state.as_wire(),
        display_state.as_wire(),
        button_state.as_wire(),
        tropic_state.as_wire(),
    );
    if pcb_result == "PASS_PRE_TROPIC" {
        println!("PCB core test passed; Tropic was not detected (expected before fitting it).");
    } else if pcb_result == "PASS" {
        println!("All fitted devices passed, including Tropic.");
    } else {
        println!("Factory test failed; inspect the component-specific log above.");
    }

    if let Some(display) = screen.as_mut() {
        let title = if pcb_result == "FAIL" {
            "PCB FAIL"
        } else {
            "PCB PASS"
        };
        let esp_line = format!("ESP32 {}", esp32_state.as_wire());
        let se_line = format!("SE050 {}", se050_state.as_wire());
        let tropic_line = match tropic_state {
            CheckState::NotFitted => "TROPIC NOT FITTED".to_string(),
            _ => format!("TROPIC {}", tropic_state.as_wire()),
        };
        let display_line = format!("DISPLAY {}", display_state.as_wire());
        let button_line = format!("BUTTON {}", button_state.as_wire());
        let _ = display.show_lines(
            title,
            &[
                &esp_line,
                &se_line,
                &tropic_line,
                &display_line,
                &button_line,
                "RESET TO RETEST",
            ],
        );
    }

    // Keep SE_ENA asserted and the result visible until the operator resets or
    // flashes the production image.
    let _keep_se050_powered = se050_enable;
    println!("Test complete. Reset the board to run it again.");
    loop {
        FreeRtos::delay_ms(1000);
    }
}

fn probe_esp32() -> Esp32ProbeSummary {
    println!("[ESP32] Runtime reached factory test on ESP32-S3");

    let heap_bytes = unsafe { esp_idf_sys::esp_get_free_heap_size() };
    let heap_ok = heap_bytes >= MIN_FACTORY_HEAP_BYTES;
    println!(
        "[ESP32] Heap={} bytes (minimum {}) -> {}",
        heap_bytes,
        MIN_FACTORY_HEAP_BYTES,
        pass_fail(heap_ok),
    );

    let mut flash_bytes = 0u32;
    let flash_err = unsafe {
        esp_idf_sys::esp_flash_get_size(core::ptr::null_mut(), &mut flash_bytes as *mut u32)
    };
    let flash_ok = flash_err == esp_idf_sys::ESP_OK && flash_bytes >= MIN_FLASH_BYTES;
    println!(
        "[ESP32] Flash={} bytes query_err={} -> {}",
        flash_bytes,
        flash_err,
        pass_fail(flash_ok),
    );

    let mut mac = [0u8; 6];
    let mac_err = unsafe { esp_idf_sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    let identity_ok = mac_err == esp_idf_sys::ESP_OK
        && mac.iter().any(|byte| *byte != 0x00)
        && mac.iter().any(|byte| *byte != 0xFF);
    println!(
        "[ESP32] eFuse MAC={:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} -> {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        pass_fail(identity_ok),
    );
    println!("[ESP32] Reset reason={}", unsafe {
        esp_idf_sys::esp_reset_reason()
    });

    Esp32ProbeSummary {
        heap_ok,
        flash_ok,
        identity_ok,
    }
}

fn setup_se050_power(enable_pin: Option<i32>) -> (Option<DynOutputPin>, bool) {
    let Some(enable_pin) = enable_pin else {
        println!("[SE050] No enable pin configured; allowing power to settle");
        FreeRtos::delay_ms(20);
        return (None, true);
    };

    let mut enable = match PinDriver::output(unsafe { AnyIOPin::new(enable_pin) }) {
        Ok(pin) => pin,
        Err(error) => {
            println!("[SE050] EN IO{enable_pin} setup failed: {error:?}");
            return (None, false);
        }
    };
    if let Err(error) = enable.set_high() {
        println!("[SE050] EN IO{enable_pin} HIGH failed: {error:?}");
        return (Some(enable), false);
    }
    println!("[SE050] EN IO{enable_pin}=HIGH -> PASS");
    FreeRtos::delay_ms(20);
    (Some(enable), true)
}

fn probe_se050(
    i2c0: esp_idf_svc::hal::i2c::I2C0,
    sda_pin: i32,
    scl_pin: i32,
    power_control_ok: bool,
) -> Se050ProbeSummary {
    println!("[SE050] I2C0 SDA=IO{sda_pin} SCL=IO{scl_pin} @100kHz");
    let mut i2c = match I2cDriver::new(
        i2c0,
        unsafe { AnyIOPin::new(sda_pin) },
        unsafe { AnyIOPin::new(scl_pin) },
        &I2cConfig::new().baudrate(100.kHz().into()),
    ) {
        Ok(driver) => driver,
        Err(error) => {
            println!("[SE050] I2C initialization failed: {error:?}");
            return Se050ProbeSummary {
                power_control_ok,
                ..Default::default()
            };
        }
    };

    if let Err(error) = i2c.write(SE050_ADDR, &SE050_SOFT_RESET, 200) {
        println!("[SE050] No ACK/reset response at 0x{SE050_ADDR:02X}: {error:?}");
        return Se050ProbeSummary {
            power_control_ok,
            ..Default::default()
        };
    }
    FreeRtos::delay_ms(10);

    let reset_frame_ok = match read_se050_block_retry(&mut i2c, 120, 5) {
        Some((pcb, inf)) => {
            println!(
                "[SE050] Valid reset frame PCB=0x{pcb:02X} INF={} bytes CRC=OK -> PASS",
                inf.len()
            );
            true
        }
        None => {
            println!("[SE050] Reset frame missing or invalid -> FAIL");
            false
        }
    };

    let select_apdu_ok = if reset_frame_ok {
        select_se050_isd(&mut i2c)
    } else {
        false
    };

    Se050ProbeSummary {
        power_control_ok,
        reset_frame_ok,
        select_apdu_ok,
    }
}

fn select_se050_isd(i2c: &mut I2cDriver) -> bool {
    let request = build_se050_frame(0x00, &SE050_SELECT_ISD);
    if let Err(error) = i2c.write(SE050_ADDR, &request, 300) {
        println!("[SE050] SELECT ISD write failed: {error:?}");
        return false;
    }
    FreeRtos::delay_ms(5);

    for _ in 0..120 {
        let Some((pcb, inf)) = read_se050_block_retry(i2c, 1, 0) else {
            FreeRtos::delay_ms(5);
            continue;
        };

        if (pcb & 0xC0) == 0xC0 {
            let response = build_se050_frame(pcb | 0x20, &inf);
            if i2c.write(SE050_ADDR, &response, 300).is_err() {
                println!("[SE050] S-block response failed -> FAIL");
                return false;
            }
            continue;
        }

        if (pcb & 0xC0) == 0x80 {
            let _ = i2c.write(SE050_ADDR, &request, 300);
            FreeRtos::delay_ms(5);
            continue;
        }

        if inf.len() < 2 {
            println!("[SE050] SELECT ISD response too short -> FAIL");
            return false;
        }
        let sw = u16::from_be_bytes([inf[inf.len() - 2], inf[inf.len() - 1]]);
        let ok = sw == 0x9000;
        println!("[SE050] SELECT ISD SW=0x{sw:04X} -> {}", pass_fail(ok));
        return ok;
    }

    println!("[SE050] SELECT ISD timed out -> FAIL");
    false
}

fn read_se050_block_retry(
    i2c: &mut I2cDriver,
    attempts: usize,
    retry_delay_ms: u32,
) -> Option<(u8, Vec<u8>)> {
    for _ in 0..attempts {
        let mut raw = [0u8; 260];
        if i2c.read(SE050_ADDR, &mut raw, 300).is_err() {
            if retry_delay_ms > 0 {
                FreeRtos::delay_ms(retry_delay_ms);
            }
            continue;
        }

        let len = raw[2] as usize;
        let frame_len = 3 + len + 2;
        if raw[0] != SE050_SE_NAD || len > 254 || frame_len > raw.len() {
            if retry_delay_ms > 0 {
                FreeRtos::delay_ms(retry_delay_ms);
            }
            continue;
        }

        let got_crc = u16::from_le_bytes([raw[3 + len], raw[4 + len]]);
        let want_crc = crc16_x25(&raw[..3 + len]);
        if got_crc != want_crc {
            if retry_delay_ms > 0 {
                FreeRtos::delay_ms(retry_delay_ms);
            }
            continue;
        }

        return Some((raw[1], raw[3..3 + len].to_vec()));
    }
    None
}

fn build_se050_frame(pcb: u8, inf: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(3 + inf.len() + 2);
    frame.push(SE050_HOST_NAD);
    frame.push(pcb);
    frame.push(inf.len() as u8);
    frame.extend_from_slice(inf);
    let crc = crc16_x25(&frame);
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xFFFF
}

fn probe_tropic(
    spi3: esp_idf_svc::hal::spi::SPI3,
    cs_pin: i32,
    sclk_pin: i32,
    miso_pin: i32,
    mosi_pin: i32,
) -> CheckState {
    println!(
        "[TROPIC] SPI3 CSN=IO{cs_pin} SCK=IO{sclk_pin} MISO=IO{miso_pin} MOSI=IO{mosi_pin} @250kHz"
    );
    let spi_driver = match SpiDriver::new(
        spi3,
        unsafe { AnyIOPin::new(sclk_pin) },
        unsafe { AnyIOPin::new(mosi_pin) },
        Some(unsafe { AnyIOPin::new(miso_pin) }),
        &SpiDriverConfig::new(),
    ) {
        Ok(driver) => driver,
        Err(error) => {
            println!("[TROPIC] SPI bus initialization failed: {error:?}");
            return CheckState::Fail;
        }
    };
    let mut dev = match SpiDeviceDriver::new(
        spi_driver,
        Some(unsafe { AnyIOPin::new(cs_pin) }),
        &SpiDeviceConfig::new()
            .baudrate(250_u32.kHz().into())
            .data_mode(MODE_0),
    ) {
        Ok(device) => device,
        Err(error) => {
            println!("[TROPIC] SPI device initialization failed: {error:?}");
            return CheckState::Fail;
        }
    };

    FreeRtos::delay_ms(300);
    let mut ready = false;
    let mut saw_non_idle_status = false;
    let mut last_status = 0u8;
    for _ in 0..40 {
        let mut rx = [0u8];
        if dev.transfer(&mut rx, &[0u8]).is_ok() {
            last_status = rx[0];
            if last_status != 0x00 && last_status != 0xFF {
                saw_non_idle_status = true;
                if (last_status & 0x01) != 0 {
                    ready = true;
                    break;
                }
            }
        }
        FreeRtos::delay_ms(50);
    }

    if !ready {
        if saw_non_idle_status {
            println!(
                "[TROPIC] Activity seen but READY never asserted (last=0x{last_status:02X}) -> FAIL"
            );
            return CheckState::Fail;
        }
        println!("[TROPIC] No electrical response (00/FF only) -> NOT FITTED");
        return CheckState::NotFitted;
    }
    println!("[TROPIC] READY status=0x{last_status:02X}");

    let mut request = build_tropic_l2_request(TROPIC_UNKNOWN_REQUEST_ID, &[]);
    if let Err(error) = dev.transfer_in_place(&mut request) {
        println!("[TROPIC] L2 request failed: {error:?}");
        return CheckState::Fail;
    }

    for _ in 0..200 {
        let mut response = vec![0u8; TROPIC_MAX_L2_RSP];
        response[0] = TROPIC_REQ_ID_GET_RESPONSE;
        if dev.transfer_in_place(&mut response).is_err() {
            FreeRtos::delay_ms(5);
            continue;
        }
        if response[1] == TROPIC_STATUS_NO_RESP {
            FreeRtos::delay_ms(5);
            continue;
        }

        let payload_len = response[2] as usize;
        let crc_index = 3 + payload_len;
        if crc_index + 1 >= response.len() {
            println!("[TROPIC] Invalid L2 response length -> FAIL");
            return CheckState::Fail;
        }
        let got_crc = u16::from_le_bytes([response[crc_index], response[crc_index + 1]]);
        let want_crc = crc16_8005_init0(&response[1..crc_index]);
        if got_crc != want_crc {
            println!("[TROPIC] L2 CRC mismatch got=0x{got_crc:04X} want=0x{want_crc:04X} -> FAIL");
            return CheckState::Fail;
        }

        let status = response[1];
        println!(
            "[TROPIC] Valid L2 response status=0x{status:02X}{} CRC=OK -> PASS",
            if status == TROPIC_STATUS_UNKNOWN_REQ {
                " (UNKNOWN_REQ expected)"
            } else {
                ""
            }
        );
        return CheckState::Pass;
    }

    println!("[TROPIC] READY asserted but no valid L2 response -> FAIL");
    CheckState::Fail
}

fn setup_button(button_pin: Option<i32>) -> Option<DynInputPin> {
    let Some(button_pin) = button_pin else {
        println!("[BUTTON] No user button configured -> FAIL");
        return None;
    };
    let mut button = match PinDriver::input(unsafe { AnyIOPin::new(button_pin) }) {
        Ok(button) => button,
        Err(error) => {
            println!("[BUTTON] IO{button_pin} input setup failed: {error:?}");
            return None;
        }
    };
    if let Err(error) = button.set_pull(Pull::Up) {
        println!("[BUTTON] IO{button_pin} pull-up setup failed: {error:?}");
        return None;
    }
    println!("[BUTTON] IO{button_pin} ready (active low)");
    Some(button)
}

fn test_button(button: Option<&mut DynInputPin>) -> CheckState {
    let Some(button) = button else {
        return CheckState::Fail;
    };

    println!("[OPERATOR] Inspect RGBW bars, text, border, and backlight.");
    println!("[OPERATOR] If the display is correct, release then press the button.");
    if !wait_for_button_level(button, false, BUTTON_RELEASE_TIMEOUT_MS) {
        println!("[BUTTON] Stuck LOW or not released -> FAIL");
        return CheckState::Fail;
    }
    if !wait_for_button_level(button, true, BUTTON_TIMEOUT_MS) {
        println!("[BUTTON] No debounced press within 30 seconds -> FAIL");
        return CheckState::Fail;
    }
    if !wait_for_button_level(button, false, BUTTON_RELEASE_TIMEOUT_MS) {
        println!("[BUTTON] Press detected but release not detected -> FAIL");
        return CheckState::Fail;
    }
    println!("[BUTTON] Released -> pressed -> released -> PASS");
    CheckState::Pass
}

fn wait_for_button_level(button: &DynInputPin, pressed: bool, timeout_ms: u64) -> bool {
    let start = monotonic_ms();
    let mut stable_since = None;
    while monotonic_ms().saturating_sub(start) < timeout_ms {
        if button.is_low() == pressed {
            let since = stable_since.get_or_insert_with(monotonic_ms);
            if monotonic_ms().saturating_sub(*since) >= BUTTON_DEBOUNCE_MS {
                return true;
            }
        } else {
            stable_since = None;
        }
        FreeRtos::delay_ms(10);
    }
    false
}

fn monotonic_ms() -> u64 {
    (unsafe { esp_idf_sys::esp_timer_get_time() } as u64) / 1000
}

fn crc16_8005_init0(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if (crc & 0x8000) != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn build_tropic_l2_request(request_id: u8, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(2 + data.len() + 2);
    frame.push(request_id);
    frame.push(data.len() as u8);
    frame.extend_from_slice(data);
    let crc = crc16_8005_init0(&frame);
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

fn state_for(passed: bool) -> CheckState {
    if passed {
        CheckState::Pass
    } else {
        CheckState::Fail
    }
}

fn pass_fail(passed: bool) -> &'static str {
    state_for(passed).as_wire()
}
