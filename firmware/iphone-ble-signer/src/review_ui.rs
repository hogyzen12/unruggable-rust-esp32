//! Native 128x160 layout, also rendered on the host for clipping checks.
use crate::signing_review::{CheckStatus, Choice, Feature, Page, PageKind};
use embedded_graphics::{
    mono_font::{
        ascii::{FONT_6X10, FONT_8X13},
        MonoTextStyleBuilder,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::Text,
};

pub fn feature_asset(feature: Feature) -> &'static [u8] {
    match feature {
        Feature::Send => include_bytes!("../assets/feature_send_128x160.rgb565"),
        Feature::Stake => include_bytes!("../assets/feature_stake_128x160.rgb565"),
        Feature::Swap => include_bytes!("../assets/feature_swap_128x160.rgb565"),
        Feature::Squads => include_bytes!("../assets/feature_squads_128x160.rgb565"),
    }
}
fn text<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    value: &str,
    x: i32,
    y: i32,
    large: bool,
    color: Rgb565,
) -> Result<(), D::Error> {
    text_on(d, value, x, y, large, color, Rgb565::BLACK)
}
fn text_on<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    value: &str,
    x: i32,
    y: i32,
    large: bool,
    color: Rgb565,
    background: Rgb565,
) -> Result<(), D::Error> {
    Text::new(
        value,
        Point::new(x, y),
        // Opaque glyphs use the driver's buffered rectangle path. Transparent
        // glyphs would issue a separate SPI transaction for every lit pixel.
        MonoTextStyleBuilder::new()
            .font(if large { &FONT_8X13 } else { &FONT_6X10 })
            .text_color(color)
            .background_color(background)
            .build(),
    )
    .draw(d)?;
    Ok(())
}
fn rule<D: DrawTarget<Color = Rgb565>>(d: &mut D) -> Result<(), D::Error> {
    Rectangle::new(Point::new(8, 128), Size::new(112, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
        .draw(d)?;
    Ok(())
}

pub fn local_menu<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    selected: usize,
    unlocked: bool,
    bonds: Option<usize>,
) -> Result<(), D::Error> {
    d.clear(Rgb565::BLACK)?;
    text(d, "WALLET", 8, 20, true, Rgb565::WHITE)?;
    let action = if unlocked { "Lock" } else { "Unlock" };
    let phones = bonds
        .map(|n| format!("Forget phones ({n})"))
        .unwrap_or_else(|| "Forget phones".into());
    let labels = [action, "Receive address", &phones, "Device info", "Back"];
    for (index, label) in labels.iter().enumerate() {
        let line = format!("{} {}", if index == selected { ">" } else { " " }, label);
        text(d, &line, 6, 44 + index as i32 * 18, false, Rgb565::WHITE)?;
    }
    text(d, "Tap: move", 8, 137, false, Rgb565::WHITE)?;
    text(d, "Hold 1s: select", 8, 150, false, Rgb565::WHITE)
}

/// Public support information remains available before setup or PIN unlock.
pub fn device_info<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    mac: &str,
    version: &str,
) -> Result<(), D::Error> {
    d.clear(Rgb565::BLACK)?;
    text(d, "DEVICE INFO", 8, 20, true, Rgb565::WHITE)?;
    text(d, "Hardware MAC", 8, 49, false, Rgb565::WHITE)?;
    text(d, mac, 8, 67, false, Rgb565::WHITE)?;
    text(d, "Firmware", 8, 99, false, Rgb565::WHITE)?;
    text(d, version, 8, 117, false, Rgb565::WHITE)?;
    text(d, "Tap: menu", 8, 150, false, Rgb565::WHITE)
}
pub fn feature_overlay<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    title: &str,
    seconds: u64,
) -> Result<(), D::Error> {
    let gray = Rgb565::new(29, 58, 29); // Exact background of the supplied artwork.
    text_on(d, title, 8, 9, false, Rgb565::BLACK, gray)?;
    timer(d, true, seconds)?;
    text_on(d, "Tap: review", 31, 147, false, Rgb565::BLACK, gray)?;
    text_on(d, "Hold 2s: reject", 19, 157, false, Rgb565::BLACK, gray)
}
pub fn timer<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    feature: bool,
    seconds: u64,
) -> Result<(), D::Error> {
    text_on(
        d,
        &format!("{seconds:2}s"),
        102,
        if feature { 9 } else { 35 },
        false,
        if feature {
            Rgb565::BLACK
        } else {
            Rgb565::WHITE
        },
        if feature {
            Rgb565::new(29, 58, 29)
        } else {
            Rgb565::BLACK
        },
    )
}
pub fn review_page<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    page: &Page,
    index: usize,
    total: usize,
    seconds: u64,
    choice: Choice,
) -> Result<(), D::Error> {
    d.clear(Rgb565::BLACK)?;
    let title = page.title;
    text(d, title, 8, 20, title.len() <= 14, Rgb565::WHITE)?;
    text(
        d,
        &format!("{index}/{}", total - 1),
        8,
        35,
        false,
        Rgb565::WHITE,
    )?;
    timer(d, false, seconds)?;
    if page.kind == PageKind::Checklist {
        for (i, check) in page.checks.iter().enumerate() {
            let y = 51 + i as i32 * 15;
            match check.status {
                CheckStatus::Verified => {
                    let stroke = PrimitiveStyle::with_stroke(Rgb565::WHITE, 1);
                    Line::new(Point::new(8, y - 4), Point::new(11, y - 1))
                        .into_styled(stroke)
                        .draw(d)?;
                    Line::new(Point::new(11, y - 1), Point::new(17, y - 8))
                        .into_styled(stroke)
                        .draw(d)?;
                }
                CheckStatus::Absent => text(d, "-", 9, y, false, Rgb565::WHITE)?,
                CheckStatus::Warning => text(d, "!", 9, y, false, Rgb565::WHITE)?,
            }
            text(d, check.label, 23, y, false, Rgb565::WHITE)?;
        }
    } else {
        for (i, line) in page.lines.iter().enumerate() {
            text(d, line, 8, 52 + i as i32 * 12, false, Rgb565::WHITE)?;
        }
    }
    if page.kind == PageKind::Checklist {
        Rectangle::new(Point::new(8, 109), Size::new(112, 15))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(d)?;
        let label = choice.label();
        text_on(
            d,
            label,
            (128 - label.len() as i32 * 6) / 2,
            120,
            false,
            Rgb565::BLACK,
            Rgb565::WHITE,
        )?;
    }
    if page.kind == PageKind::Feature(None) {
        text(d, "Effects unknown", 8, 67, false, Rgb565::WHITE)?;
        text(d, "May move funds", 8, 83, false, Rgb565::WHITE)?;
    }
    rule(d)?;
    let (tap, hold) = match page.kind {
        PageKind::Checklist => (
            "Tap: choose action",
            match choice {
                Choice::Sign => "Hold 2s: sign",
                Choice::Reject => "Hold 2s: reject",
                Choice::Review => "Hold 2s: review",
            },
        ),
        _ => ("Tap: next", "Hold 2s: reject"),
    };
    text(d, tap, 8, 140, false, Rgb565::WHITE)?;
    text(d, hold, 8, 153, false, Rgb565::WHITE)
}
pub fn hold_hint<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    page: &Page,
    held_ms: u64,
    choice: Choice,
) -> Result<(), D::Error> {
    Rectangle::new(Point::new(0, 122), Size::new(128, 38))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(d)?;
    let target = 2000;
    let fill = (held_ms.min(target) * 112 / target) as u32;
    Rectangle::new(Point::new(8, 124), Size::new(fill, 2))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
        .draw(d)?;
    let hint = match page.kind {
        PageKind::Checklist => match choice {
            Choice::Sign => "Holding to sign",
            Choice::Reject => "Holding to reject",
            Choice::Review => "Holding to review",
        },
        _ => "Holding to reject",
    };
    text(d, hint, 8, 140, false, Rgb565::WHITE)?;
    text(d, "Release to stop", 8, 153, false, Rgb565::WHITE)
}

/// Generic status screens use the same crisp typography; long diagnostic text
/// is explicitly shortened. Signing pages instead have bounded, complete fields.
pub fn message<D: DrawTarget<Color = Rgb565>>(
    d: &mut D,
    title: &str,
    lines: &[&str],
) -> Result<(), D::Error> {
    d.clear(Rgb565::BLACK)?;
    let title: String = title
        .chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .take(18)
        .collect();
    text(d, &title, 8, 20, title.len() <= 14, Rgb565::WHITE)?;
    let mut body = Vec::new();
    for line in lines {
        let ascii: String = line
            .chars()
            .map(|c| if c.is_ascii() { c } else { '?' })
            .collect();
        if ascii.is_empty() {
            body.push(String::new());
        }
        for chunk in ascii.as_bytes().chunks(18) {
            body.push(String::from_utf8(chunk.to_vec()).unwrap());
        }
    }
    let overflow = body.len() > 9;
    for (i, line) in body.iter().take(9).enumerate() {
        text(
            d,
            if i == 8 && overflow { "..." } else { line },
            8,
            46 + i as i32 * 12,
            false,
            Rgb565::WHITE,
        )?;
    }
    Ok(())
}
