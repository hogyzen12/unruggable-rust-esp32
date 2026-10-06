use anyhow::{anyhow, Result};

use crate::board::DisplayPins;

use embedded_graphics::{
    mono_font::{ascii::FONT_8X13, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
    Pixel,
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
const ROTATE_180: bool = false;

// Display improvements
const FRAME_MARGIN: i32 = 6; // Safe border around all content
const FRAME_THICKNESS: u32 = 1; // Border line thickness
const CHAR_WIDTH: i32 = 8; // FONT_8X13 character width
const CHAR_HEIGHT: i32 = 13; // FONT_8X13 character height
const LINE_SPACING: i32 = 16; // Space between text lines
const IDLE_TIMEOUT_MS: u64 = 30_000; // 30 seconds until screensaver
const SCREENSAVER_W: u32 = 128;
const SCREENSAVER_H: u32 = 160;
const SCREENSAVER_STILL_BYTES: usize = (SCREENSAVER_W as usize) * (SCREENSAVER_H as usize) * 2;
const SCREENSAVER_STILL: &[u8] = include_bytes!("../assets/screensaver_still_128x160.rgb565");
const FRAME_COLOR: Rgb565 = Rgb565::new(4, 10, 7);
const MUTED_COLOR: Rgb565 = Rgb565::new(12, 24, 18);
const PANEL_BG: Rgb565 = Rgb565::new(2, 4, 3);
const PANEL_CARD: Rgb565 = Rgb565::new(4, 8, 6);
const PANEL_ACCENT: Rgb565 = Rgb565::new(0, 42, 18);

// Connection states for status bar
#[derive(Clone, PartialEq, Debug)]
pub enum ConnectionState {
    Disconnected, // No app activity
    Ready,        // Connected and ready
    Signing,      // Transaction signing in progress
    Waiting,      // Waiting for button press
    Locked,       // PIN or 2FA locked
}

// Display state - tracks what's currently shown to avoid unnecessary redraws
#[derive(Clone, PartialEq)]
enum DisplayState {
    None,
    WalletReady {
        pubkey: String,
        extra: Option<String>,
        connection: ConnectionState,
    },
    Keygen {
        progress: u8,
        total: u8,
    },
    QrCode {
        title: String,
        uri: String,
        footer: String,
    },
    Message {
        title: String,
        lines: Vec<String>,
    },
    YesNo {
        title: String,
        is_yes: bool,
    },
    HoldAction {
        title: String,
        subtitle: String,
        target_ms: u32,
        bucket: u16,
        accent: Rgb565,
    },
    PinEntry {
        title: String,
        subtitle: String,
        digits: [u8; 6],
        cursor: usize,
        phase: String,
    },
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
    pub fn new(
        spi2: SPI2,
        pins: DisplayPins,
    ) -> Result<Self> {
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
            .baudrate(esp_idf_svc::hal::units::Hertz(80_000_000))
            .write_only(true)
            .queue_size(6);
        let spi_dev = SpiDeviceDriver::new(
            spi_driver,
            Some(unsafe { AnyIOPin::new(pins.cs) }),
            &spi_cfg,
        )?;

        let dc = PinDriver::output(unsafe { AnyIOPin::new(pins.dc) })?;
        let rst = PinDriver::output(unsafe { AnyIOPin::new(pins.rst) })?;

        let mut tft = ST7735::new(spi_dev, dc, rst, true, false, W, H);

        let mut delay = Ets;
        tft.init(&mut delay)
            .map_err(|_| anyhow!("ST7735 init failed"))?;
        let orientation = if ROTATE_180 {
            Orientation::PortraitSwapped
        } else {
            Orientation::Portrait
        };
        tft.set_orientation(&orientation)
            .map_err(|_| anyhow!("ST7735 set_orientation failed"))?;

        tft.set_offset(pins.offset_x, pins.offset_y);
        tft.clear(PANEL_BG)
            .map_err(|_| anyhow!("clear failed"))?;

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

    /// Check if display has been idle long enough for screensaver
    pub fn check_idle(&mut self) -> Result<()> {
        let now_ms = Self::now_ms();
        if now_ms.saturating_sub(self.last_activity_ms) <= IDLE_TIMEOUT_MS {
            return Ok(());
        }

        if self.current_state != DisplayState::Screensaver {
            self.show_screensaver()?;
        }
        Ok(())
    }

    // ========== Utility Methods ==========

    /// Returns the safe drawable rectangle (with margins)
    fn safe_rect(&self) -> Rectangle {
        Rectangle::new(
            Point::new(FRAME_MARGIN, FRAME_MARGIN),
            Size::new(
                (W as i32 - 2 * FRAME_MARGIN) as u32,
                (H as i32 - 2 * FRAME_MARGIN) as u32,
            ),
        )
    }

    /// Wrap text to fit within max_chars per line
    fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current = String::new();

        for word in text.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() <= max_chars {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current);
                current = word.to_string();
            }
        }

        if !current.is_empty() {
            lines.push(current);
        }

        lines
    }

    /// Draw text with automatic wrapping within safe area
    fn draw_text_wrapped(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        style: MonoTextStyle<Rgb565>,
    ) -> Result<i32> {
        let safe_width = (W as i32) - 2 * FRAME_MARGIN - x;
        let max_chars = (safe_width / CHAR_WIDTH) as usize;
        let lines = Self::wrap_text(text, max_chars);

        let mut current_y = y;
        for line in lines {
            Text::new(&line, Point::new(x, current_y), style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw text failed"))?;
            current_y += LINE_SPACING;
        }

        Ok(current_y)
    }

    /// Clear screen only if state changed
    fn update_state(&mut self, new_state: DisplayState) -> Result<bool> {
        if self.current_state != new_state {
            self.tft
                .clear(PANEL_BG)
                .map_err(|_| anyhow!("clear failed"))?;
            self.draw_frame_border()?;
            self.current_state = new_state;
            self.update_activity();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Draw visible frame border around content area
    fn draw_frame_border(&mut self) -> Result<()> {
        // Draw border rectangle
        let border = Rectangle::new(
            Point::new(FRAME_MARGIN - 1, FRAME_MARGIN - 1),
            Size::new(
                (W as i32 - 2 * FRAME_MARGIN + 2) as u32,
                (H as i32 - 2 * FRAME_MARGIN + 2) as u32,
            ),
        );

        border
            .into_styled(PrimitiveStyle::with_stroke(FRAME_COLOR, FRAME_THICKNESS))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw frame border failed"))?;

        Rectangle::new(
            Point::new(FRAME_MARGIN, FRAME_MARGIN),
            Size::new(
                (W as i32 - 2 * FRAME_MARGIN) as u32,
                (H as i32 - 2 * FRAME_MARGIN) as u32,
            ),
        )
        .into_styled(PrimitiveStyle::with_fill(PANEL_CARD))
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("draw frame card failed"))?;

        Ok(())
    }

    // ========== Public Display Methods ==========

    pub fn show_lines(&mut self, title: &str, lines: &[&str]) -> Result<()> {
        let new_state = DisplayState::Message {
            title: title.to_string(),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        };

        self.update_state(new_state)?;

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);

        let x = FRAME_MARGIN + 2;
        let mut y = FRAME_MARGIN + 12;

        // Draw title with wrapping
        y = self.draw_text_wrapped(title, x, y, title_style)?;
        y += 4; // Space after title

        // Calculate max lines that fit
        let available_height = (H as i32) - FRAME_MARGIN - y;
        let max_lines = (available_height / LINE_SPACING) as usize;

        for line in lines.iter().take(max_lines.min(8)) {
            if y + LINE_SPACING > (H as i32) - FRAME_MARGIN - 4 {
                break; // Stop if we'd overflow
            }
            y = self.draw_text_wrapped(line, x, y, style)?;
        }

        Ok(())
    }

    pub fn show_pin_entry(
        &mut self,
        title: &str,
        subtitle: &str,
        digits: &[u8; 6],
        cursor: usize,
        phase: &str,
    ) -> Result<()> {
        let new_state = DisplayState::PinEntry {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            digits: *digits,
            cursor,
            phase: phase.to_string(),
        };

        self.update_state(new_state)?;

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let subtitle_style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
        let hint_style = MonoTextStyle::new(&FONT_8X13, MUTED_COLOR);
        let digit_style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
        let active_digit_style = MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK);

        let x = FRAME_MARGIN + 6;
        let mut y = FRAME_MARGIN + 14;

        Text::new(title, Point::new(x, y), title_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin title failed"))?;
        y += LINE_SPACING;

        Text::new(subtitle, Point::new(x, y), subtitle_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin subtitle failed"))?;
        y += LINE_SPACING + 4;

        Text::new(phase, Point::new(x, y), hint_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin phase failed"))?;
        y += LINE_SPACING + 6;

        let box_w = 16i32;
        let box_h = 22i32;
        let gap = 3i32;
        let total_w = (box_w * 6) + (gap * 5);
        let start_x = ((W as i32) - total_w) / 2;

        for (idx, digit) in digits.iter().enumerate() {
            let box_x = start_x + (idx as i32 * (box_w + gap));
            let box_y = y;
            let is_active = idx == cursor;
            let fill = if is_active { PANEL_ACCENT } else { PANEL_BG };
            let stroke = if is_active { Rgb565::WHITE } else { MUTED_COLOR };

            Rectangle::new(
                Point::new(box_x, box_y),
                Size::new(box_w as u32, box_h as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(fill))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin fill failed"))?;
            Rectangle::new(
                Point::new(box_x, box_y),
                Size::new(box_w as u32, box_h as u32),
            )
            .into_styled(PrimitiveStyle::with_stroke(stroke, 1))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin box failed"))?;

            let digit_text = digit.to_string();
            let digit_x = box_x + 4;
            let digit_y = box_y + 15;
            Text::new(
                &digit_text,
                Point::new(digit_x, digit_y),
                if is_active { active_digit_style } else { digit_style },
            )
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pin digit failed"))?;
        }

        y += box_h + 14;
        Text::new("Tap +1", Point::new(x, y), hint_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw tap hint failed"))?;
        y += LINE_SPACING;
        Text::new("Hold to confirm digit", Point::new(x, y), hint_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw hold hint failed"))?;

        Ok(())
    }

    pub fn show_setup_hold(&mut self) -> Result<()> {
        self.show_lines("SETUP", &["Hold to start", "Tap to skip"])
    }

    pub fn show_keygen_progress(&mut self, n: u8, total: u8) -> Result<()> {
        let new_state = DisplayState::Keygen { progress: n, total };

        // Only redraw if progress actually changed
        if !self.update_state(new_state)? {
            return Ok(());
        }

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);

        let x = FRAME_MARGIN + 4;
        let mut y = FRAME_MARGIN + 14;

        Text::new("KEYGEN", Point::new(x, y), title_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw title failed"))?;

        y += LINE_SPACING + 6;

        let line = format!("Press {}/{}", n, total);
        Text::new(&line, Point::new(x, y), style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw text failed"))?;

        // Progress bar with safe margins
        let bar_x = FRAME_MARGIN + 4;
        let bar_y = y + LINE_SPACING + 4;
        let bar_w = (W as i32) - 2 * FRAME_MARGIN - 8;
        let bar_h = 14i32;

        Rectangle::new(
            Point::new(bar_x, bar_y),
            Size::new(bar_w as u32, bar_h as u32),
        )
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("draw bar border failed"))?;

        let filled = (bar_w - 2) * (n as i32) / (total as i32);
        if filled > 0 {
            Rectangle::new(
                Point::new(bar_x + 1, bar_y + 1),
                Size::new(filled as u32, (bar_h - 2) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw bar fill failed"))?;
        }

        Ok(())
    }

    pub fn show_hold_to_sign_progress(&mut self, held_ms: u32, target_ms: u32) -> Result<()> {
        self.show_hold_action_progress(
            "HOLD TO SIGN",
            "Press and hold button",
            held_ms,
            target_ms,
            Rgb565::GREEN,
        )
    }

    pub fn show_hold_action_progress(
        &mut self,
        title: &str,
        subtitle: &str,
        held_ms: u32,
        target_ms: u32,
        accent: Rgb565,
    ) -> Result<()> {
        let target = target_ms.max(1);
        let clamped = held_ms.min(target);
        let bucket = (clamped / 20) as u16;

        let x = FRAME_MARGIN + 4;
        let title_y = FRAME_MARGIN + 14;
        let info_y = title_y + LINE_SPACING + 6;
        let bar_x = FRAME_MARGIN + 4;
        let bar_y = info_y + LINE_SPACING + 6;
        let bar_w = (W as i32) - 2 * FRAME_MARGIN - 8;
        let bar_h = 14i32;
        let status_y = bar_y + LINE_SPACING + 10;

        let needs_full_redraw = !matches!(
            &self.current_state,
            DisplayState::HoldAction {
                title: existing_title,
                subtitle: existing_subtitle,
                target_ms: t,
                accent: existing_accent,
                ..
            } if existing_title == title && existing_subtitle == subtitle && *t == target && *existing_accent == accent
        );

        if needs_full_redraw {
            self.tft
                .clear(Rgb565::BLACK)
                .map_err(|_| anyhow!("clear failed"))?;
            self.draw_frame_border()?;

            let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
            let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);

            Text::new(title, Point::new(x, title_y), title_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw title failed"))?;
            Text::new(subtitle, Point::new(x, info_y), style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw line failed"))?;

            Rectangle::new(
                Point::new(bar_x, bar_y),
                Size::new(bar_w as u32, bar_h as u32),
            )
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw bar border failed"))?;
        } else if matches!(self.current_state, DisplayState::HoldAction { bucket: b, .. } if b == bucket)
        {
            return Ok(());
        }

        // Update only dynamic regions to prevent flicker.
        Rectangle::new(
            Point::new(bar_x + 1, bar_y + 1),
            Size::new((bar_w - 2) as u32, (bar_h - 2) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(PANEL_CARD))
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("clear bar fill failed"))?;

        let filled = (bar_w - 2) * (clamped as i32) / (target as i32);
        if filled > 0 {
            Rectangle::new(
                Point::new(bar_x + 1, bar_y + 1),
                Size::new(filled as u32, (bar_h - 2) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(accent))
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw bar fill failed"))?;
        }

        Rectangle::new(
            Point::new(x, status_y - CHAR_HEIGHT + 1),
            Size::new((bar_w as u32).saturating_sub(2), (CHAR_HEIGHT + 4) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(PANEL_CARD))
        .draw(&mut self.tft)
        .map_err(|_| anyhow!("clear progress text failed"))?;

        let progress_style = MonoTextStyle::new(&FONT_8X13, accent);
        let pct = (clamped.saturating_mul(100) / target).min(100);
        let status = format!("{}ms / {}ms ({}%)", clamped, target, pct);
        Text::new(&status, Point::new(x, status_y), progress_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw progress text failed"))?;

        self.current_state = DisplayState::HoldAction {
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            target_ms: target,
            bucket,
            accent,
        };
        self.update_activity();
        Ok(())
    }

    pub fn show_yes_no(&mut self, title: &str, is_yes: bool) -> Result<()> {
        let new_state = DisplayState::YesNo {
            title: title.to_string(),
            is_yes,
        };

        self.update_state(new_state)?;

        let choice = if is_yes { "YES" } else { "NO" };
        let choice_color = if is_yes { Rgb565::GREEN } else { Rgb565::RED };

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
        let choice_style = MonoTextStyle::new(&FONT_8X13, choice_color);

        let x = FRAME_MARGIN + 4;
        let mut y = FRAME_MARGIN + 14;

        y = self.draw_text_wrapped(title, x, y, title_style)?;
        y += 6;

        Text::new("Tap=toggle", Point::new(x, y), style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw text failed"))?;
        y += LINE_SPACING;

        Text::new("Hold=confirm", Point::new(x, y), style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw text failed"))?;
        y += LINE_SPACING + 8;

        Text::new("Choice:", Point::new(x, y), style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw text failed"))?;
        y += LINE_SPACING;

        Text::new(choice, Point::new(x, y), choice_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw choice failed"))?;

        Ok(())
    }

    pub fn show_wallet_ready(
        &mut self,
        pubkey: &str,
        extra: Option<&str>,
        connection: ConnectionState,
    ) -> Result<()> {
        let new_state = DisplayState::WalletReady {
            pubkey: pubkey.to_string(),
            extra: extra.map(|s| s.to_string()),
            connection: connection.clone(),
        };

        self.update_state(new_state)?;

        let x = FRAME_MARGIN + 6;
        let mut y = FRAME_MARGIN + 16;

        // ========== STATUS BAR ==========
        // Draw status indicator based on connection state
        let (status_text, status_color) = match connection {
            ConnectionState::Disconnected => ("DISCONNECTED", Rgb565::RED),
            ConnectionState::Ready => ("READY", Rgb565::GREEN),
            ConnectionState::Signing => ("SIGNING", Rgb565::YELLOW),
            ConnectionState::Waiting => ("WAITING", Rgb565::BLUE),
            ConnectionState::Locked => ("LOCKED", Rgb565::YELLOW),
        };

        let status_style = MonoTextStyle::new(&FONT_8X13, status_color);
        Text::new(status_text, Point::new(x, y), status_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw status failed"))?;
        y += LINE_SPACING + 4;

        // Draw separator line under status
        let sep_y = y;
        let sep_w = (W as i32) - 2 * FRAME_MARGIN - 4;
        for px in 0..sep_w {
            if px % 2 == 0 {
                // Dotted line
                Pixel(Point::new(x + px, sep_y), MUTED_COLOR)
                    .draw(&mut self.tft)
                    .map_err(|_| anyhow!("draw separator failed"))?;
            }
        }
        y += 12;

        // ========== WALLET SECTION ==========
        let section_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let label_style = MonoTextStyle::new(&FONT_8X13, MUTED_COLOR);
        let value_style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);

        Text::new("WALLET", Point::new(x, y), section_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw section failed"))?;
        y += LINE_SPACING + 4;

        // Shortened pubkey display: first 6 + "..." + last 6
        let short_pubkey = if pubkey.len() > 15 {
            format!("{}...{}", &pubkey[..6], &pubkey[pubkey.len() - 6..])
        } else {
            pubkey.to_string()
        };

        Text::new("ADDRESS", Point::new(x + 4, y), label_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw label failed"))?;
        y += LINE_SPACING;

        Text::new(&short_pubkey, Point::new(x + 4, y), value_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw pubkey failed"))?;
        y += LINE_SPACING + 6;

        // ========== STATUS/EXTRA SECTION ==========
        if let Some(e) = extra {
            let warning_style = MonoTextStyle::new(&FONT_8X13, Rgb565::YELLOW);
            y = self.draw_text_wrapped(e, x + 4, y, warning_style)?;
            y += 6;
        }

        let footer_style = MonoTextStyle::new(&FONT_8X13, MUTED_COLOR);
        let footer_y = (H as i32) - FRAME_MARGIN - 26;
        Text::new("USB + button wallet", Point::new(x + 4, footer_y), footer_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw footer failed"))?;
        Text::new("Hold for prompts", Point::new(x + 4, footer_y + LINE_SPACING), footer_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw footer failed"))?;

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
        let footer_style = MonoTextStyle::new(&FONT_8X13, Rgb565::BLACK);

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

    /// Show transaction verification results with visual indicators
    pub fn show_verification_results(
        &mut self,
        check_lines: &[String],
        overall_safe: bool,
    ) -> Result<()> {
        // Force redraw by clearing first
        self.tft
            .clear(PANEL_BG)
            .map_err(|_| anyhow!("clear failed"))?;
        self.draw_frame_border()?;
        self.update_activity();

        let title_style = MonoTextStyle::new(&FONT_8X13, Rgb565::CYAN);
        let pass_style = MonoTextStyle::new(&FONT_8X13, Rgb565::GREEN);
        let fail_style = MonoTextStyle::new(&FONT_8X13, Rgb565::RED);
        let warn_style = MonoTextStyle::new(&FONT_8X13, Rgb565::YELLOW);
        let normal_style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);

        let x = FRAME_MARGIN + 2;
        let mut y = FRAME_MARGIN + 12;

        // Title
        Text::new("VERIFY TX", Point::new(x, y), title_style)
            .draw(&mut self.tft)
            .map_err(|_| anyhow!("draw title failed"))?;
        y += LINE_SPACING + 4;

        // Show each check with colored indicator
        for line in check_lines.iter().take(6) {
            if y + LINE_SPACING > (H as i32) - FRAME_MARGIN - 20 {
                break; // Stop if we'd overflow
            }

            let style = if line.starts_with("✓") {
                pass_style
            } else if line.starts_with("✗") {
                fail_style
            } else if line.starts_with("!") {
                warn_style
            } else {
                normal_style
            };

            Text::new(line, Point::new(x + 2, y), style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw check failed"))?;
            y += LINE_SPACING;
        }

        y += 8;

        // Status message and approval prompt
        if overall_safe {
            Text::new("All checks OK", Point::new(x + 2, y), pass_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw status failed"))?;
            y += LINE_SPACING + 4;
            Text::new("Hold to sign", Point::new(x + 2, y), normal_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw prompt failed"))?;
        } else {
            Text::new("CHECKS FAILED", Point::new(x + 2, y), fail_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw status failed"))?;
            y += LINE_SPACING + 4;
            Text::new("Review before", Point::new(x + 2, y), warn_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw warning failed"))?;
            y += LINE_SPACING;
            Text::new("signing!", Point::new(x + 2, y), warn_style)
                .draw(&mut self.tft)
                .map_err(|_| anyhow!("draw warning failed"))?;
        }

        Ok(())
    }

    pub fn show_screensaver(&mut self) -> Result<()> {
        if self.current_state != DisplayState::Screensaver {
            self.tft
                .clear(PANEL_BG)
                .map_err(|_| anyhow!("clear failed"))?;
            self.current_state = DisplayState::Screensaver;
            self.draw_screensaver_still()?;
        }
        Ok(())
    }

    fn draw_screensaver_still(&mut self) -> Result<()> {
        if SCREENSAVER_STILL.len() < SCREENSAVER_STILL_BYTES {
            return Err(anyhow!("screensaver image asset has invalid size"));
        }

        let colors = SCREENSAVER_STILL[..SCREENSAVER_STILL_BYTES]
            .chunks_exact(2)
            .map(|px| u16::from_le_bytes([px[0], px[1]]));
        self.tft
            .set_pixels_buffered(
                0,
                0,
                (SCREENSAVER_W - 1) as u16,
                (SCREENSAVER_H - 1) as u16,
                colors,
            )
            .map_err(|_| anyhow!("draw screensaver still image failed"))?;

        Ok(())
    }
}

fn short_pubkey(pk: &str) -> (String, String) {
    if pk.len() <= 28 {
        return (pk.to_string(), "".into());
    }
    let head = &pk[..14];
    let tail = &pk[pk.len() - 14..];
    (format!("{head}..."), format!("...{tail}"))
}
