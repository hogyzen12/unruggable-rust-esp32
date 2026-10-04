use anyhow::{anyhow, Result};

use crate::board::{DisplayColorOrder, DisplayPins};

use embedded_graphics::{
    mono_font::{
        ascii::{FONT_6X10, FONT_8X13},
        MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};

use esp_idf_svc::hal::{
    delay::Ets,
    gpio::{AnyIOPin, Output, PinDriver},
    spi::{
        config::{Config as DeviceConfig, DriverConfig},
        Dma, SpiDeviceDriver, SpiDriver, SPI2,
    },
};

use qrcodegen::{QrCode, QrCodeEcc};
use st7735_lcd::{Orientation, ST7735};

const W: u32 = 128;
const H: u32 = 160;

const FRAME_MARGIN: i32 = 6; // Safe border around all content
const SCREENSAVER_W: u32 = 128;
const SCREENSAVER_H: u32 = 160;
const SCREENSAVER_STILL_BYTES: usize = (SCREENSAVER_W as usize) * (SCREENSAVER_H as usize) * 2;
const SCREENSAVER_STILL: &[u8] = include_bytes!("../assets/idle_128x160.rgb565");
const PIN_BACKGROUND: &[u8] = include_bytes!("../assets/pin_background_128x160.rgb565");
const PANEL_BG: Rgb565 = Rgb565::BLACK;

// Display state - tracks what's currently shown to avoid unnecessary redraws
#[derive(Clone, PartialEq)]
enum DisplayState {
    None,
    #[allow(dead_code)]
    FactoryTest,
    QrCode {
        title: String,
        uri: String,
        footer: String,
    },
    Message {
        title: String,
        lines: Vec<String>,
    },
    HoldAction {
        title: String,
        subtitle: String,
        target_ms: u32,
        bucket: u16,
        accent: Rgb565,
    },
    PinEntry,
    Screensaver,
}

pub struct Display {
    tft: ST7735<
        SpiDeviceDriver<'static, SpiDriver<'static>>,
        PinDriver<'static, AnyIOPin, Output>, // DC
        PinDriver<'static, AnyIOPin, Output>, // RST
    >,
    _bl: PinDriver<'static, AnyIOPin, Output>,
    current_state: DisplayState,
    last_activity_ms: u64,
}

impl Display {
    pub fn new(spi2: SPI2, pins: DisplayPins) -> Result<Self> {
        // Backlight ON
        let mut bl = PinDriver::output(unsafe { AnyIOPin::new(pins.bl) })?;
        bl.set_high()?; // flip if your BL is inverted

        let spi_driver = SpiDriver::new(
            spi2,
            unsafe { AnyIOPin::new(pins.sck) },
            unsafe { AnyIOPin::new(pins.mosi) },
            Option::<AnyIOPin>::None, // MISO not used
            &DriverConfig::new().dma(Dma::Auto(4096)),
        )?;

        let spi_cfg = DeviceConfig::new()
            .baudrate(esp_idf_svc::hal::units::Hertz(pins.spi_hz))
            .write_only(true)
            .queue_size(6);
        let spi_dev = SpiDeviceDriver::new(
            spi_driver,
            Some(unsafe { AnyIOPin::new(pins.cs) }),
            &spi_cfg,
        )?;

        let dc = PinDriver::output(unsafe { AnyIOPin::new(pins.dc) })?;
        let rst = PinDriver::output(unsafe { AnyIOPin::new(pins.rst) })?;

        let rgb_color_order = matches!(pins.color_order, DisplayColorOrder::Rgb);
        let mut tft = ST7735::new(spi_dev, dc, rst, rgb_color_order, pins.inverted, W, H);

        let mut delay = Ets;
        tft.init(&mut delay)
            .map_err(|_| anyhow!("ST7735 init failed"))?;
        let orientation = if pins.rotate_180 {
            Orientation::Portrait
        } else {
            Orientation::PortraitSwapped
        };
        tft.set_orientation(&orientation)
            .map_err(|_| anyhow!("ST7735 set_orientation failed"))?;

        tft.set_offset(pins.offset_x, pins.offset_y);
        tft.clear(PANEL_BG).map_err(|_| anyhow!("clear failed"))?;

        Ok(Self {
            tft,
            _bl: bl,
            current_state: DisplayState::None,
            last_activity_ms: 0,
        })
    }

    fn now_ms() -> u64 {
        (unsafe { esp_idf_sys::esp_timer_get_time() } as u64) / 1000
    }

    // ========== Activity & Idle Management ==========

    /// Update last activity timestamp (call on UART or button events)
    pub fn update_activity(&mut self) {
        self.last_activity_ms = Self::now_ms();
    }

    // ========== Public Display Methods ==========

    /// Draw a human-verifiable panel test. The four colour bars exercise every
    /// RGB channel, while the one-pixel border makes clipping/offset faults
    /// visible. Factory-test firmware asks the operator to press the hardware
    /// button only when this pattern looks correct.
    #[allow(dead_code)]
    pub fn show_factory_test_pattern(&mut self) -> Result<()> {
        self.tft
            .clear(Rgb565::BLACK)
            .map_err(|_| anyhow!("factory pattern clear failed"))?;

        let colors = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::WHITE];
        for (index, color) in colors.iter().copied().enumerate() {
            Rectangle::new(Point::new((index as i32) * 32, 0), Size::new(32, 72))
                .into_styled(PrimitiveStyle::with_fill(color))
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("factory colour bar draw failed"))?;
        }

        Rectangle::new(Point::new(0, 0), Size::new(W, H))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("factory border draw failed"))?;

        let title = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
        let hint = MonoTextStyle::new(&FONT_8X13, Rgb565::YELLOW);
        Text::new("DISPLAY TEST", Point::new(16, 94), title)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("factory title draw failed"))?;
        Text::new("RGBW + BORDER", Point::new(12, 116), title)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("factory label draw failed"))?;
        Text::new("PRESS BUTTON", Point::new(16, 143), hint)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("factory hint draw failed"))?;

        self.current_state = DisplayState::FactoryTest;
        self.update_activity();
        Ok(())
    }

    pub fn show_lines(&mut self, title: &str, lines: &[&str]) -> Result<()> {
        let state = DisplayState::Message {
            title: title.to_string(),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        };
        if self.current_state != state {
            crate::review_ui::message(&mut self.tft, title, lines)
                .map_err(|_| anyhow!("message draw failed"))?;
            self.current_state = state;
        }
        self.update_activity();
        Ok(())
    }

    pub fn show_review_page(
        &mut self,
        review: &crate::signing_review::Review,
        index: usize,
        seconds: u64,
        choice: crate::signing_review::Choice,
    ) -> Result<()> {
        use crate::signing_review::PageKind;
        self.current_state = DisplayState::None;
        let page = &review.pages[index];
        if let PageKind::Feature(Some(feature)) = page.kind {
            self.draw_rgb565_fullscreen(crate::review_ui::feature_asset(feature), W, H, "request")?;
            crate::review_ui::feature_overlay(&mut self.tft, page.title, seconds)
                .map_err(|_| anyhow!("request overlay draw failed"))?;
        } else {
            crate::review_ui::review_page(
                &mut self.tft,
                page,
                index,
                review.pages.len(),
                seconds,
                choice,
            )
            .map_err(|_| anyhow!("review draw failed"))?;
        }
        self.update_activity();
        Ok(())
    }

    pub fn show_review_hold(
        &mut self,
        page: &crate::signing_review::Page,
        held_ms: u64,
        choice: crate::signing_review::Choice,
    ) -> Result<()> {
        crate::review_ui::hold_hint(&mut self.tft, page, held_ms, choice)
            .map_err(|_| anyhow!("review progress draw failed"))?;
        self.update_activity();
        Ok(())
    }

    pub fn show_review_timer(
        &mut self,
        page: &crate::signing_review::Page,
        seconds: u64,
    ) -> Result<()> {
        crate::review_ui::timer(
            &mut self.tft,
            matches!(page.kind, crate::signing_review::PageKind::Feature(Some(_))),
            seconds,
        )
        .map_err(|_| anyhow!("review timer draw failed"))
    }

    pub fn show_pairing_code(&mut self, code: u32) -> Result<()> {
        self.current_state = DisplayState::None;
        crate::review_ui::message(&mut self.tft, "PAIR PHONE", &[])
            .map_err(|_| anyhow!("pairing draw failed"))?;
        Text::new(
            &format!("{code:06}"),
            Point::new(40, 65),
            MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE),
        )
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("pairing code draw failed"))?;
        let small = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
        for (line, y) in [
            ("Enter on your phone", 96),
            ("Bluetooth code", 110),
            ("Hold 3s: cancel", 153),
        ] {
            Text::new(line, Point::new(8, y), small)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("pairing hint draw failed"))?;
        }
        self.update_activity();
        Ok(())
    }

    /// Cache no PIN digits. Only the currently selected digit is passed in;
    /// previous digits are masked and never copied into a display state/string.
    pub fn show_pin_entry(&mut self, cursor: usize, active_digit: Option<u8>) -> Result<()> {
        self.current_state = DisplayState::PinEntry;
        self.draw_rgb565_fullscreen(PIN_BACKGROUND, W, H, "pin")?;
        self.update_activity();
        let digit_style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
        let hint_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
        for index in 0..6 {
            let glyph = [if index < cursor {
                b'*'
            } else if index == cursor {
                b'0' + active_digit.unwrap_or(0).min(9)
            } else {
                b'_'
            }];
            let text = core::str::from_utf8(&glyph)?;
            Text::new(text, Point::new(16 + index as i32 * 18, 110), digit_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw pin digit failed"))?;
            if index == cursor {
                Rectangle::new(Point::new(15 + index as i32 * 18, 114), Size::new(10, 1))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                    .draw(&mut self.tft)
                    .map_err(|_| anyhow!("draw pin cursor failed"))?;
            }
        }
        let hints = if cursor == 6 {
            ["Hold 1s: unlock", "Tap: start again", "Hold 3s: cancel"]
        } else {
            ["Tap: change digit", "Hold 1s: next", "Hold 3s: cancel"]
        };
        for (index, hint) in hints.iter().enumerate() {
            Text::new(hint, Point::new(10, 132 + index as i32 * 11), hint_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw pin hint failed"))?;
        }
        Ok(())
    }

    pub fn show_local_menu(
        &mut self,
        selected: usize,
        unlocked: bool,
        bonds: Option<usize>,
    ) -> Result<()> {
        self.current_state = DisplayState::None;
        crate::review_ui::local_menu(&mut self.tft, selected, unlocked, bonds)
            .map_err(|_| anyhow!("draw local menu failed"))?;
        self.update_activity();
        Ok(())
    }

    pub fn show_device_info(&mut self, mac: &str, version: &str) -> Result<()> {
        self.current_state = DisplayState::None;
        crate::review_ui::device_info(&mut self.tft, mac, version)
            .map_err(|_| anyhow!("draw device info failed"))?;
        self.update_activity();
        Ok(())
    }

    pub fn show_hold_action_progress(
        &mut self,
        title: &str,
        subtitle: &str,
        held_ms: u32,
        target_ms: u32,
        _accent: Rgb565,
    ) -> Result<()> {
        let target = target_ms.max(1);
        let clamped = held_ms.min(target);
        let bucket = (clamped / 50) as u16;
        let state = DisplayState::HoldAction {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            target_ms: target,
            bucket,
            accent: Rgb565::WHITE,
        };
        if self.current_state == state {
            return Ok(());
        }
        let same_page = matches!(&self.current_state, DisplayState::HoldAction { title: old, subtitle: sub, target_ms: t, .. }
            if old == title && sub == subtitle && *t == target);
        if !same_page {
            crate::review_ui::message(&mut self.tft, title, &[subtitle])
                .map_err(|_| anyhow!("hold prompt draw failed"))?;
        }
        Rectangle::new(Point::new(8, 92), Size::new(112, 12))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("clear hold bar failed"))?;
        Rectangle::new(Point::new(8, 92), Size::new(112, 12))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("hold bar draw failed"))?;
        let fill = clamped.saturating_mul(108) / target;
        Rectangle::new(Point::new(10, 94), Size::new(fill, 8))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("hold fill draw failed"))?;
        Rectangle::new(Point::new(8, 112), Size::new(112, 16))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("clear hold text failed"))?;
        let text = format!("Hold to confirm {}%", clamped.saturating_mul(100) / target);
        Text::new(
            &text,
            Point::new(8, 122),
            MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE),
        )
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("hold text draw failed"))?;
        self.current_state = state;
        self.update_activity();
        Ok(())
    }

    pub fn show_qr(&mut self, title: &str, uri: &str, footer: &str) -> Result<()> {
        let new_state = DisplayState::QrCode {
            title: title.to_string(),
            uri: uri.to_string(),
            footer: footer.to_string(),
        };

        // QR codes need white background - don't draw border for QR
        if self.current_state != new_state {
            self.tft
                .clear(Rgb565::WHITE)
                .map_err(|_| anyhow!("clear failed"))?;
            self.current_state = new_state;
            self.update_activity();
        } else {
            return Ok(());
        }

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK);
        let footer_style = MonoTextStyle::new(&FONT_6X10, Rgb565::BLACK);

        let x = FRAME_MARGIN + 2;
        Text::new(title, Point::new(x, FRAME_MARGIN + 12), title_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw title failed"))?;

        let code =
            QrCode::encode_text(uri, QrCodeEcc::Medium).map_err(|_| anyhow!("QR too long"))?;
        let size = code.size();
        let quiet: i32 = 2;
        let total = size + quiet * 2;

        let reserved_top = 26i32;
        let reserved_bottom = 38i32;

        let avail_w = W as i32;
        let avail_h = (H as i32) - reserved_top - reserved_bottom;

        let scale_x = avail_w / total;
        let scale_y = avail_h / total;
        let mut scale = scale_x.min(scale_y);
        if scale < 1 {
            scale = 1;
        }

        let qr_px = total * scale;
        let x0 = (avail_w - qr_px) / 2;
        let y0 = reserved_top + (avail_h - qr_px) / 2;

        for y in 0..total {
            for x in 0..total {
                let dark = if x >= quiet && x < quiet + size && y >= quiet && y < quiet + size {
                    code.get_module(x - quiet, y - quiet)
                } else {
                    false
                };

                if dark {
                    let px = x0 + x * scale;
                    let py = y0 + y * scale;
                    Rectangle::new(Point::new(px, py), Size::new(scale as u32, scale as u32))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut self.tft)
                        .map_err(|_| anyhow!("draw qr module failed"))?;
                }
            }
        }

        Text::new(
            footer,
            Point::new(FRAME_MARGIN + 4, (H as i32) - FRAME_MARGIN - 6),
            footer_style,
        )
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("draw footer failed"))?;

        Ok(())
    }

    pub fn show_screensaver(&mut self) -> Result<()> {
        if self.current_state != DisplayState::Screensaver {
            self.current_state = DisplayState::Screensaver;
            self.draw_screensaver_still()?;
        }
        Ok(())
    }

    fn draw_rgb565_fullscreen(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<()> {
        let expected_len = (width as usize) * (height as usize) * 2;
        if pixels.len() < expected_len {
            return Err(anyhow!("{label} asset has invalid size"));
        }

        let colors = pixels[..expected_len]
            .chunks_exact(2)
            .map(|px| u16::from_le_bytes([px[0], px[1]]));
        self.tft
            .set_pixels_buffered(0, 0, (width - 1) as u16, (height - 1) as u16, colors)
            .map_err(|_| anyhow!("{label} image draw failed"))?;

        Ok(())
    }

    fn draw_screensaver_still(&mut self) -> Result<()> {
        if SCREENSAVER_STILL.len() < SCREENSAVER_STILL_BYTES {
            return Err(anyhow!("screensaver image asset has invalid size"));
        }
        self.draw_rgb565_fullscreen(
            SCREENSAVER_STILL,
            SCREENSAVER_W,
            SCREENSAVER_H,
            "screensaver",
        )
    }
}
