//! Render the actual firmware draw functions, asserting that no pixel is clipped.
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use firmware_host_tests::{
    review_ui,
    signing_review::{Choice, PageKind, Review},
};
use std::{convert::Infallible, fs, path::Path};
struct Screen {
    rgb: Vec<u8>,
}
impl OriginDimensions for Screen {
    fn size(&self) -> Size {
        Size::new(128, 160)
    }
}
impl DrawTarget for Screen {
    type Color = Rgb565;
    type Error = Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            assert!(
                (0..128).contains(&p.x) && (0..160).contains(&p.y),
                "clipped pixel {p:?}"
            );
            let offset = (p.y as usize * 128 + p.x as usize) * 3;
            self.rgb[offset..offset + 3].copy_from_slice(&[
                (u16::from(c.r()) * 255 / 31) as u8,
                (u16::from(c.g()) * 255 / 63) as u8,
                (u16::from(c.b()) * 255 / 31) as u8,
            ]);
        }
        Ok(())
    }
}
fn save(path: &Path, screen: &Screen) {
    let mut bytes = b"P6\n128 160\n255\n".to_vec();
    bytes.extend(&screen.rgb);
    fs::write(path, bytes).unwrap();
}
fn main() {
    let directory = std::env::args().nth(1).expect("output directory");
    let directory = Path::new(&directory);
    fs::create_dir_all(directory).unwrap();
    let mut support = Screen { rgb: vec![0; 128 * 160 * 3] };
    review_ui::device_info(&mut support, "E0:72:A1:E4:E9:D4", "0.3.1").unwrap();
    save(&directory.join("device-info.ppm"), &support);
    for selected in 0..5 {
        review_ui::local_menu(&mut support, selected, false, Some(0)).unwrap();
        save(&directory.join(format!("local-menu-{selected}.ppm")), &support);
    }
    let wallet = bs58::encode([7; 32]).into_string();
    let recipient = bs58::encode([8; 32]).into_string();
    let mut message = vec![1, 0, 1, 3];
    message.extend([7; 32]);
    message.extend([8; 32]);
    message.extend([0; 32]);
    message.extend([0; 32]);
    message.extend([1, 2, 2, 0, 1, 12]);
    message.extend(2u32.to_le_bytes());
    message.extend(1_000_000u64.to_le_bytes());
    // Same four-instruction shape as the finalized mainnet Send, using the
    // real on-device renderer. This only renders pixels; it never signs/sends.
    let app_wallet = "3Yepu6XLfTcZHauTiPHgS5ces7btANdj6gKYkCZPk8ku";
    let mut app_message = vec![1, 0, 3, 7];
    for key in [
        app_wallet,
        "6tBou5MHL5aWpDy6cgf3wiwGGK2mR8qs68ujtpaoWrf2",
        "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
        "juLesoSmdTcRtzjCzYzRoHrnF8GhVu6KCV7uxq7nJGp",
        "11111111111111111111111111111111",
        "SysvarC1ock11111111111111111111111111111111",
        "23MzuyVH6EKGbUHq7GjBY6ydSCVoZQYDmzeKVdDBKWNQ",
    ] {
        app_message.extend(bs58::decode(key).into_vec().unwrap());
    }
    app_message.extend([0; 32]);
    app_message.extend([4, 6, 1, 5, 8]);
    app_message.extend(451_496_745u64.to_le_bytes());
    for (to, amount) in [(1, 1_000_000u64), (2, 4_200), (3, 100_000)] {
        app_message.extend([4, 2, 0, to, 12]);
        app_message.extend(2u32.to_le_bytes());
        app_message.extend(amount.to_le_bytes());
    }
    let mut mismatch = app_message.clone();
    let amount_index = mismatch.len() - 8;
    mismatch[amount_index] += 1;
    let cases = [
        ("app-send", Review::from_message(&app_message, app_wallet)),
        (
            "markers-differ",
            Review::from_message(&mismatch, app_wallet),
        ),
        ("send", Review::from_message(&message, &wallet)),
        (
            "blind",
            Review::from_message(b"not a decoded transaction", &wallet),
        ),
    ];
    for (name, review) in cases {
        for (i, page) in review.pages.iter().enumerate() {
            assert!(page.lines.len() <= 6);
            assert!(page.lines.iter().all(|l| l.len() <= 18));
            assert!(page.checks.len() <= 3);
            assert!(page.checks.iter().all(|check| check.label.len() <= 16));
            let mut screen = Screen {
                rgb: vec![0; 128 * 160 * 3],
            };
            if let PageKind::Feature(Some(feature)) = page.kind {
                for (i, bytes) in review_ui::feature_asset(feature)
                    .chunks_exact(2)
                    .enumerate()
                {
                    let raw = u16::from_le_bytes([bytes[0], bytes[1]]);
                    let r = ((raw >> 11) & 31) as u32;
                    let g = ((raw >> 5) & 63) as u32;
                    let b = (raw & 31) as u32;
                    screen.rgb[i * 3..i * 3 + 3].copy_from_slice(&[
                        (r * 255 / 31) as u8,
                        (g * 255 / 63) as u8,
                        (b * 255 / 31) as u8,
                    ]);
                }
                review_ui::feature_overlay(&mut screen, page.title, 60).unwrap();
            } else {
                review_ui::review_page(&mut screen, page, i, review.pages.len(), 60, Choice::Sign)
                    .unwrap();
            }
            save(&directory.join(format!("{name}-{i}.ppm")), &screen);
            for held in [50, 800, 2000, 3000] {
                review_ui::hold_hint(&mut screen, page, held, Choice::Sign).unwrap();
            }
        }
    }
    for text in [
        recipient,
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".into(),
    ] {
        let mut screen = Screen {
            rgb: vec![0; 128 * 160 * 3],
        };
        review_ui::message(&mut screen, "PHONES REMOVED", &[&text, "Hold 3s: home"]).unwrap();
        save(&directory.join("status.ppm"), &screen);
    }
    println!(
        "Actual firmware screens rendered with every pixel in bounds: {}",
        directory.display()
    );
}
