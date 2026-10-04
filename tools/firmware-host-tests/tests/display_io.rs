//! Exercise the shipping display driver: rendering must not monopolize the button loop.
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use embedded_hal::{digital, spi};
use firmware_host_tests::{
    review_ui,
    signing_review::{Check, CheckStatus, Choice, Page, PageKind},
};
use std::{cell::RefCell, convert::Infallible, rc::Rc};

#[derive(Clone, Default)]
struct Trace(Rc<RefCell<Vec<Vec<u8>>>>);
impl spi::ErrorType for Trace {
    type Error = Infallible;
}
impl spi::SpiDevice for Trace {
    fn transaction(
        &mut self,
        operations: &mut [spi::Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        for operation in operations {
            match operation {
                spi::Operation::Write(bytes) => self.0.borrow_mut().push(bytes.to_vec()),
                _ => panic!("display should only write"),
            }
        }
        Ok(())
    }
}
struct Pin;
impl digital::ErrorType for Pin {
    type Error = Infallible;
}
impl digital::OutputPin for Pin {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn display(trace: Trace) -> st7735_lcd::ST7735<Trace, Pin, Pin> {
    st7735_lcd::ST7735::new(trace, Pin, Pin, true, false, 128, 160)
}

#[test]
fn footer_clear_is_batched_and_clipped_fills_keep_the_correct_pixels() {
    let trace = Trace::default();
    let mut display = display(trace.clone());
    display
        .fill_solid(
            &Rectangle::new(Point::new(0, 122), Size::new(128, 38)),
            Rgb565::BLACK,
        )
        .unwrap();
    assert!(
        trace.0.borrow().len() < 30,
        "footer redraw must not send thousands of individual pixels"
    );
    trace.0.borrow_mut().clear();
    display
        .fill_solid(
            &Rectangle::new(Point::new(-2, -3), Size::new(6, 8)),
            Rgb565::WHITE,
        )
        .unwrap();
    let writes = trace.0.borrow();
    assert_eq!(writes[1], [0, 0, 0, 3]);
    assert_eq!(writes[3], [0, 0, 0, 4]);
    assert_eq!(writes[5..].concat(), vec![255; 4 * 5 * 2]);
    drop(writes);
    trace.0.borrow_mut().clear();
    display
        .fill_solid(
            &Rectangle::new(Point::new(128, 160), Size::new(10, 10)),
            Rgb565::WHITE,
        )
        .unwrap();
    assert!(trace.0.borrow().is_empty());
    for size in [Size::new(0, 8), Size::new(8, 0)] {
        display
            .fill_solid(&Rectangle::new(Point::new(1, 1), size), Rgb565::WHITE)
            .unwrap();
    }
    assert!(trace.0.borrow().is_empty());
}

#[test]
fn real_hold_and_countdown_rendering_use_bounded_spi_work() {
    let trace = Trace::default();
    let mut display = display(trace.clone());
    let page = Page {
        kind: PageKind::Checklist,
        title: "SIGN CHECKLIST",
        lines: vec![],
        checks: vec![
            Check {
                label: "Message decoded",
                status: CheckStatus::Verified,
            },
            Check {
                label: "Wallet signer",
                status: CheckStatus::Verified,
            },
            Check {
                label: "App markers",
                status: CheckStatus::Verified,
            },
        ],
    };
    review_ui::review_page(&mut display, &page, 2, 3, 60, Choice::Sign).unwrap();
    assert!(
        trace.0.borrow().len() < 1400,
        "screen text must use buffered glyphs"
    );
    trace.0.borrow_mut().clear();
    review_ui::hold_hint(&mut display, &page, 1500, Choice::Sign).unwrap();
    assert!(
        trace.0.borrow().len() < 250,
        "hold progress must remain responsive"
    );
    trace.0.borrow_mut().clear();
    review_ui::timer(&mut display, false, 59).unwrap();
    assert!(
        trace.0.borrow().len() <= 18,
        "timer must update only its three glyphs"
    );
}
