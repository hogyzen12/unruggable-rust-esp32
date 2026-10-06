#[path = "../board.rs"]
mod board;

use board::active_board_config;
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{AnyIOPin, Output, PinDriver},
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
const TROPIC_REQ_ID_GET_RESPONSE: u8 = 0xAA;
const TROPIC_STATUS_NO_RESP: u8 = 0xFF;
const TROPIC_STATUS_UNKNOWN_REQ: u8 = 0x7E;
const TROPIC_UNKNOWN_REQUEST_ID: u8 = 0x00;
const TROPIC_MAX_L2_RSP: usize = 260;

type DynOutputPin = PinDriver<'static, AnyIOPin, Output>;

#[derive(Clone, Copy, Debug, Default)]
struct Se050ProbeSummary {
    seen_on_bus: bool,
    atr_looks_valid: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct TropicProbeSummary {
    ready_seen: bool,
    l2_transport_ok: bool,
}

fn main() {
    esp_idf_sys::link_patches();
    let board = active_board_config();

    println!("==== Unruggable Peripheral Probe ====");
    println!("Board profile: {}", board.profile.as_wire());
    println!("Board description: {}", board.description);
    println!(
        "I2C(se050): SDA=IO{} SCL=IO{} EN={}",
        board.se050.sda,
        board.se050.scl,
        board
            .se050
            .enable
            .map(|pin| format!("IO{pin}"))
            .unwrap_or_else(|| "none".to_string())
    );
    if let Some(tropic) = board.tropic {
        println!(
            "Tropic: CSN=IO{} SCK=IO{} MISO=IO{} MOSI=IO{}",
            tropic.csn, tropic.sck, tropic.miso, tropic.mosi
        );
    } else {
        println!("Tropic: disabled in this board profile");
    }
    if let Some(display) = board.display {
        println!(
            "TFT: CS=IO{} SCL=IO{} SDA=IO{} DC=IO{} RES=IO{} BL=IO{}",
            display.cs, display.sck, display.mosi, display.dc, display.rst, display.bl
        );
    } else {
        println!("TFT: disabled in this board profile");
    }
    println!("-------------------------------------");

    let p = Peripherals::take().unwrap();

    let _se_enable = probe_se050_enable_pin(board.se050.enable);
    let se050 = probe_i2c_se050(p.i2c0, board.se050.sda, board.se050.scl);
    let tropic = if let Some(tropic) = board.tropic {
        probe_tropic(
            p.spi3,
            tropic.csn,
            tropic.sck,
            tropic.miso,
            tropic.mosi,
        )
    } else {
        TropicProbeSummary::default()
    };
    if let Some(display) = board.display {
        probe_tft_bus_and_control(
            p.spi2,
            display.cs,
            display.sck,
            display.mosi,
            display.dc,
            display.rst,
            display.bl,
        );
    }

    println!("-------------------------------------");
    println!(
        "Summary: SE050 bus={} ATR={} | Tropic READY={} L2={}",
        yes_no(se050.seen_on_bus),
        yes_no(se050.atr_looks_valid),
        yes_no(tropic.ready_seen),
        yes_no(tropic.l2_transport_ok),
    );
    println!("Probe complete. Staying alive for monitor.");
    loop {
        FreeRtos::delay_ms(1000);
    }
}

fn probe_se050_enable_pin(enable_pin: Option<i32>) -> Option<DynOutputPin> {
    let Some(enable_pin) = enable_pin else {
        println!("[SE050] No explicit enable pin in this board profile");
        return None;
    };

    println!("[SE050] Asserting SE_ENA (IO{}) HIGH", enable_pin);

    let mut se_ena = match PinDriver::output(unsafe { AnyIOPin::new(enable_pin) }) {
        Ok(pin) => pin,
        Err(e) => {
            println!("[SE050] SE_ENA pin setup failed: {:?}", e);
            return None;
        }
    };

    if let Err(e) = se_ena.set_high() {
        println!("[SE050] SE_ENA set HIGH failed: {:?}", e);
    } else {
        println!("[SE050] SE_ENA HIGH");
    }

    FreeRtos::delay_ms(5);
    Some(se_ena)
}

fn probe_i2c_se050(
    i2c0: esp_idf_svc::hal::i2c::I2C0,
    sda_pin: i32,
    scl_pin: i32,
) -> Se050ProbeSummary {
    println!("[I2C] Init I2C0 on SDA=IO{} SCL=IO{} @100kHz", sda_pin, scl_pin);

    let mut i2c = match I2cDriver::new(
        i2c0,
        unsafe { AnyIOPin::new(sda_pin) },
        unsafe { AnyIOPin::new(scl_pin) },
        &I2cConfig::new().baudrate(100.kHz().into()),
    ) {
        Ok(driver) => driver,
        Err(e) => {
            println!("[I2C] Init failed: {:?}", e);
            return Se050ProbeSummary::default();
        }
    };

    let mut found = Vec::new();
    for addr in 1u8..=126 {
        if i2c.write(addr, &[], 8).is_ok() {
            found.push(addr);
        }
    }

    let mut summary = Se050ProbeSummary {
        seen_on_bus: found.contains(&SE050_ADDR),
        atr_looks_valid: false,
    };

    if found.is_empty() {
        println!("[I2C] No devices found");
    } else {
        for addr in &found {
            if *addr == SE050_ADDR {
                println!("[I2C] Found 0x{:02X} (SE050 expected)", addr);
            } else {
                println!("[I2C] Found 0x{:02X}", addr);
            }
        }
    }

    println!("[SE050] ATR probe on 0x{:02X}", SE050_ADDR);
    if let Err(e) = i2c.write(SE050_ADDR, &SE050_SOFT_RESET, 100) {
        println!("[SE050] Soft reset write failed: {:?}", e);
        return summary;
    }

    FreeRtos::delay_ms(10);

    let mut atr = [0u8; 35];
    match i2c.read(SE050_ADDR, &mut atr, 200) {
        Ok(_) => {
            print_hex("[SE050] ATR", &atr);
            let looks_valid = atr.iter().any(|b| *b != 0x00 && *b != 0xFF);
            if looks_valid {
                println!("[SE050] ATR looks valid -> SE050 responding");
                summary.atr_looks_valid = true;
            } else {
                println!("[SE050] ATR all 00/FF -> check power/wiring");
            }
        }
        Err(e) => println!("[SE050] ATR read failed: {:?}", e),
    }

    summary
}

fn probe_tropic(
    spi3: esp_idf_svc::hal::spi::SPI3,
    cs_pin: i32,
    sclk_pin: i32,
    miso_pin: i32,
    mosi_pin: i32,
) -> TropicProbeSummary {
    println!(
        "[TROPIC] Init SPI3 on CSN=IO{} SCK=IO{} MISO=IO{} MOSI=IO{} @250kHz",
        cs_pin, sclk_pin, miso_pin, mosi_pin
    );

    let spi_driver = match SpiDriver::new(
        spi3,
        unsafe { AnyIOPin::new(sclk_pin) },
        unsafe { AnyIOPin::new(mosi_pin) },
        Some(unsafe { AnyIOPin::new(miso_pin) }),
        &SpiDriverConfig::new(),
    ) {
        Ok(driver) => driver,
        Err(e) => {
            println!("[TROPIC] SPI driver init failed: {:?}", e);
            return TropicProbeSummary::default();
        }
    };

    let dev_cfg = SpiDeviceConfig::new()
        .baudrate(250_u32.kHz().into())
        .data_mode(MODE_0);

    let mut dev = match SpiDeviceDriver::new(
        spi_driver,
        Some(unsafe { AnyIOPin::new(cs_pin) }),
        &dev_cfg,
    ) {
        Ok(dev) => dev,
        Err(e) => {
            println!("[TROPIC] SPI device init failed: {:?}", e);
            return TropicProbeSummary::default();
        }
    };

    FreeRtos::delay_ms(300);

    let mut summary = TropicProbeSummary::default();
    let mut last_chip_status = None;
    for attempt in 0..40 {
        let tx = [0x00u8];
        let mut rx = [0x00u8];
        match dev.transfer(&mut rx, &tx) {
            Ok(()) => {
                let chip_status = rx[0];
                let ready = (chip_status & 0x01) != 0;
                let alarm = (chip_status & 0x02) != 0;
                let start = (chip_status & 0x04) != 0;

                last_chip_status = Some(chip_status);
                println!(
                    "[TROPIC] READY poll {:02}: CHIP_STATUS=0x{:02X} READY={} ALARM={} START={}",
                    attempt + 1,
                    chip_status,
                    ready as u8,
                    alarm as u8,
                    start as u8
                );

                if ready {
                    println!("[TROPIC] READY=1 -> chip responded");
                    summary.ready_seen = true;
                    break;
                }
            }
            Err(e) => println!("[TROPIC] READY poll {:02} failed: {:?}", attempt + 1, e),
        }

        FreeRtos::delay_ms(50);
    }

    if !summary.ready_seen {
        match last_chip_status {
            Some(status) => println!(
                "[TROPIC] Never saw READY=1; last CHIP_STATUS=0x{:02X}",
                status
            ),
            None => println!("[TROPIC] Never received a CHIP_STATUS byte"),
        }
        return summary;
    }

    let mut l2_req = build_tropic_l2_request(TROPIC_UNKNOWN_REQUEST_ID, &[]);
    match dev.transfer_in_place(&mut l2_req) {
        Ok(()) => println!(
            "[TROPIC] Sent L2 unknown request 0x{:02X}; polling Get_Response",
            TROPIC_UNKNOWN_REQUEST_ID
        ),
        Err(e) => {
            println!("[TROPIC] L2 request write failed: {:?}", e);
            return summary;
        }
    }

    for attempt in 0..200 {
        let mut buf = vec![0u8; TROPIC_MAX_L2_RSP];
        buf[0] = TROPIC_REQ_ID_GET_RESPONSE;

        match dev.transfer_in_place(&mut buf) {
            Ok(()) => {
                let chip_status = buf[0];
                let status = buf[1];
                if status == TROPIC_STATUS_NO_RESP {
                    if attempt % 20 == 0 {
                        println!("[TROPIC] Get_Response pending (STATUS=0xFF)");
                    }
                    FreeRtos::delay_ms(5);
                    continue;
                }

                let rsp_len = buf[2] as usize;
                let crc_index = 3 + rsp_len;
                if crc_index + 1 >= buf.len() {
                    println!(
                        "[TROPIC] Invalid L2 length: rsp_len={} frame_len={}",
                        rsp_len,
                        buf.len()
                    );
                    return summary;
                }

                let rsp_crc = (buf[crc_index] as u16) | ((buf[crc_index + 1] as u16) << 8);
                let calc_crc = crc16_8005_init0(&buf[1..crc_index]);
                println!(
                    "[TROPIC] Get_Response: CHIP_STATUS=0x{:02X} STATUS=0x{:02X} RSP_LEN={} CRC=0x{:04X} calc=0x{:04X}",
                    chip_status, status, rsp_len, rsp_crc, calc_crc
                );

                if rsp_crc != calc_crc {
                    println!("[TROPIC] CRC mismatch on Get_Response");
                    return summary;
                }

                summary.l2_transport_ok = true;
                if status == TROPIC_STATUS_UNKNOWN_REQ {
                    println!("[TROPIC] L2 transport verified: UNKNOWN_REQ (0x7E)");
                } else {
                    println!("[TROPIC] L2 transport responded with status 0x{:02X}", status);
                }
                return summary;
            }
            Err(e) => {
                println!(
                    "[TROPIC] Get_Response poll {:03} failed: {:?}",
                    attempt + 1,
                    e
                );
                FreeRtos::delay_ms(5);
            }
        }
    }

    println!("[TROPIC] No valid L2 response observed");
    summary
}

fn probe_tft_bus_and_control(
    spi2: esp_idf_svc::hal::spi::SPI2,
    cs_pin: i32,
    sclk_pin: i32,
    mosi_pin: i32,
    dc_pin: i32,
    rst_pin: i32,
    bl_pin: i32,
) {
    println!("[TFT] Probing control pins and SPI bus");

    let mut bl = match PinDriver::output(unsafe { AnyIOPin::new(bl_pin) }) {
        Ok(pin) => pin,
        Err(e) => {
            println!("[TFT] BL pin setup failed: {:?}", e);
            return;
        }
    };

    let mut rst = match PinDriver::output(unsafe { AnyIOPin::new(rst_pin) }) {
        Ok(pin) => pin,
        Err(e) => {
            println!("[TFT] RST pin setup failed: {:?}", e);
            return;
        }
    };

    let _dc = match PinDriver::output(unsafe { AnyIOPin::new(dc_pin) }) {
        Ok(pin) => pin,
        Err(e) => {
            println!("[TFT] DC pin setup failed: {:?}", e);
            return;
        }
    };

    let _ = bl.set_high();
    let _ = rst.set_low();
    FreeRtos::delay_ms(20);
    let _ = rst.set_high();
    println!("[TFT] BL HIGH + RST pulse sent");

    let spi_driver = match SpiDriver::new(
        spi2,
        unsafe { AnyIOPin::new(sclk_pin) },
        unsafe { AnyIOPin::new(mosi_pin) },
        Option::<AnyIOPin>::None,
        &SpiDriverConfig::new(),
    ) {
        Ok(driver) => driver,
        Err(e) => {
            println!("[TFT] SPI driver init failed: {:?}", e);
            return;
        }
    };

    let _spi_dev = match SpiDeviceDriver::new(
        spi_driver,
        Some(unsafe { AnyIOPin::new(cs_pin) }),
        &SpiDeviceConfig::new(),
    ) {
        Ok(dev) => dev,
        Err(e) => {
            println!("[TFT] SPI device init failed: {:?}", e);
            return;
        }
    };

    println!("[TFT] SPI bus/device init OK on configured pins");
}

fn print_hex(label: &str, data: &[u8]) {
    print!("{} ({} bytes):", label, data.len());
    for (i, b) in data.iter().enumerate() {
        if i % 16 == 0 {
            print!("\n  ");
        }
        print!("{:02X} ", b);
    }
    println!();
}

fn crc16_8005_init0(data: &[u8]) -> u16 {
    let mut crc: u16 = 0x0000;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x8005;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn build_tropic_l2_request(req_id: u8, req_data: &[u8]) -> Vec<u8> {
    let req_len = req_data.len() as u8;
    let mut frame = Vec::with_capacity(2 + req_data.len() + 2);
    frame.push(req_id);
    frame.push(req_len);
    frame.extend_from_slice(req_data);

    let crc = crc16_8005_init0(&frame);
    frame.push((crc & 0x00FF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
