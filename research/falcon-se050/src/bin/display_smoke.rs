#[path = "../board.rs"]
mod board;

use anyhow::{anyhow, Context, Result};
use board::active_board_config;
use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    text::Text,
};
use esp_idf_svc::hal::{
    delay::{Ets, FreeRtos},
    gpio::{AnyIOPin, Output, PinDriver},
    peripherals::Peripherals,
    spi::{
        config::{Config as SpiDeviceConfig, DriverConfig as SpiDriverConfig},
        Dma, SpiDeviceDriver, SpiDriver,
    },
};
use st7735_lcd::{Orientation, ST7735};

const DISPLAY_W: u32 = 128;
const DISPLAY_H: u32 = 160;
const SPI_HZ: u32 = 10_000_000;
const COLOR_HOLD_MS: u32 = 1200;
const ROTATE_180: bool = false;

type DynOutputPin = PinDriver<'static, AnyIOPin, Output>;

fn main() -> Result<()> {
    esp_idf_sys::link_patches();

    let board = active_board_config();
    let display = board
        .display
        .with_context(|| format!("board profile {} has no display pins", board.profile.as_wire()))?;

    println!("==== Unruggable Display Smoke ====");
    println!("Board profile: {}", board.profile.as_wire());
    println!("Board description: {}", board.description);
    println!(
        "TFT pins: CS=IO{} SCK=IO{} MOSI=IO{} DC=IO{} RST=IO{} BL=IO{} offset=({}, {})",
        display.cs,
        display.sck,
        display.mosi,
        display.dc,
        display.rst,
        display.bl,
        display.offset_x,
        display.offset_y
    );
    println!("SPI: {} Hz", SPI_HZ);
    println!("----------------------------------");

    let peripherals = Peripherals::take().context("failed to take peripherals")?;

    let mut bl = configure_output(display.bl, "BL")?;
    let dc = configure_output(display.dc, "DC")?;
    let rst = configure_output(display.rst, "RST")?;

    println!("[TFT] Blinking backlight");
    blink_backlight(&mut bl)?;

    println!("[TFT] Creating SPI bus");
    let spi_driver = SpiDriver::new(
        peripherals.spi2,
        unsafe { AnyIOPin::new(display.sck) },
        unsafe { AnyIOPin::new(display.mosi) },
        Option::<AnyIOPin>::None,
        &SpiDriverConfig::new().dma(Dma::Auto(4096)),
    )
    .context("SPI driver init failed")?;

    let spi_cfg = SpiDeviceConfig::new()
        .baudrate(esp_idf_svc::hal::units::Hertz(SPI_HZ))
        .write_only(true)
        .queue_size(4);

    let spi_dev = SpiDeviceDriver::new(
        spi_driver,
        Some(unsafe { AnyIOPin::new(display.cs) }),
        &spi_cfg,
    )
    .context("SPI device init failed")?;

    println!("[TFT] Initializing ST7735");
    let mut panel = ST7735::new(spi_dev, dc, rst, true, false, DISPLAY_W, DISPLAY_H);
    let mut delay = Ets;
    panel.init(&mut delay).map_err(|_| anyhow!("ST7735 init failed"))?;

    let orientation = if ROTATE_180 {
        Orientation::PortraitSwapped
    } else {
        Orientation::Portrait
    };
    panel
        .set_orientation(&orientation)
        .map_err(|_| anyhow!("set_orientation failed"))?;
    panel.set_offset(display.offset_x, display.offset_y);

    println!("[TFT] Panel init OK, starting color cycle");

    let steps = [
        ("BLACK", Rgb565::BLACK, Rgb565::WHITE),
        ("WHITE", Rgb565::WHITE, Rgb565::BLACK),
        ("RED", Rgb565::RED, Rgb565::WHITE),
        ("GREEN", Rgb565::GREEN, Rgb565::BLACK),
        ("BLUE", Rgb565::BLUE, Rgb565::WHITE),
        ("CYAN", Rgb565::CYAN, Rgb565::BLACK),
        ("MAGENTA", Rgb565::MAGENTA, Rgb565::WHITE),
        ("YELLOW", Rgb565::YELLOW, Rgb565::BLACK),
    ];

    loop {
        for (label, background, foreground) in steps {
            println!("[TFT] Showing {label}");
            panel
                .clear(background)
                .map_err(|_| anyhow!("clear({label}) failed"))?;

            let style = MonoTextStyle::new(&FONT_8X13, foreground);
            Text::new("DISPLAY OK", Point::new(10, 40), style)
                .draw(&mut panel)
                .map_err(|_| anyhow!("draw title failed"))?;
            Text::new(label, Point::new(10, 64), style)
                .draw(&mut panel)
                .map_err(|_| anyhow!("draw color label failed"))?;
            Text::new(board.profile.as_wire(), Point::new(10, 88), style)
                .draw(&mut panel)
                .map_err(|_| anyhow!("draw board label failed"))?;
            Text::new("RST/DC/SPI active", Point::new(10, 112), style)
                .draw(&mut panel)
                .map_err(|_| anyhow!("draw status failed"))?;

            FreeRtos::delay_ms(COLOR_HOLD_MS);
        }
    }
}

fn configure_output(pin: i32, label: &str) -> Result<DynOutputPin> {
    PinDriver::output(unsafe { AnyIOPin::new(pin) })
        .with_context(|| format!("failed to configure {label} pin IO{pin} as output"))
}

fn blink_backlight(bl: &mut DynOutputPin) -> Result<()> {
    bl.set_low().context("BL set_low failed")?;
    FreeRtos::delay_ms(250);
    bl.set_high().context("BL set_high failed")?;
    FreeRtos::delay_ms(250);
    bl.set_low().context("BL second set_low failed")?;
    FreeRtos::delay_ms(250);
    bl.set_high().context("BL second set_high failed")?;
    FreeRtos::delay_ms(250);
    Ok(())
}
