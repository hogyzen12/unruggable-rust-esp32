//! Single-button local controls. No display cache or Debug implementation owns a PIN.
use zeroize::{Zeroize, Zeroizing};

pub const PIN_TIMEOUT_MS: u64 = 180_000;
pub const MENU_LEN: usize = 5;
const IDLE_TIMEOUT_MS: u64 = 45_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Tap,
    Select,
    Cancel,
}

/// Interpret on release, so a long cancel never first confirms a digit/action.
pub struct Button {
    since: Option<u64>,
    wait_for_release: bool,
}

impl Button {
    pub fn new() -> Self {
        Self {
            since: None,
            wait_for_release: true,
        }
    }
    pub fn poll(&mut self, pressed: bool, now: u64) -> Option<Input> {
        if self.wait_for_release {
            self.wait_for_release = pressed;
            return None;
        }
        if pressed {
            self.since.get_or_insert(now);
            return None;
        }
        let held = now.saturating_sub(self.since.take()?);
        match held {
            0..=49 => None,
            50..=799 => Some(Input::Tap),
            800..=2999 => Some(Input::Select),
            _ => Some(Input::Cancel),
        }
    }
}

pub struct PinEntry {
    digits: Zeroizing<[u8; 6]>,
    cursor: usize,
    started: u64,
    last_input: u64,
}

pub enum PinAction {
    Editing,
    Submit(Zeroizing<[u8; 6]>),
    Cancel,
}

impl PinEntry {
    pub fn new(now: u64) -> Self {
        Self {
            digits: Zeroizing::new([b'0'; 6]),
            cursor: 0,
            started: now,
            last_input: now,
        }
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn active_digit(&self) -> Option<u8> {
        self.digits.get(self.cursor).map(|d| d - b'0')
    }
    pub fn expired(&self, now: u64) -> bool {
        now.saturating_sub(self.started) >= PIN_TIMEOUT_MS
            || now.saturating_sub(self.last_input) >= IDLE_TIMEOUT_MS
    }
    pub fn input(&mut self, input: Input, now: u64) -> PinAction {
        if self.expired(now) || input == Input::Cancel {
            self.digits.zeroize();
            return PinAction::Cancel;
        }
        self.last_input = now;
        match input {
            Input::Tap if self.cursor == 6 => {
                self.digits.zeroize();
                *self.digits = [b'0'; 6];
                self.cursor = 0;
            }
            Input::Tap => {
                self.digits[self.cursor] = b'0' + (self.digits[self.cursor] - b'0' + 1) % 10
            }
            Input::Select if self.cursor == 6 => {
                let pin = Zeroizing::new(*self.digits);
                self.digits.zeroize();
                return PinAction::Submit(pin);
            }
            Input::Select => self.cursor += 1,
            Input::Cancel => unreachable!(),
        }
        PinAction::Editing
    }
}

pub enum Page {
    Home,
    Menu(usize),
    Pin(PinEntry),
    Forget,
    Notice(&'static str, &'static str),
    Receive,
    DeviceInfo,
}

pub struct LocalUi {
    pub page: Page,
    pub button: Button,
    pub changed: bool,
    last_input: u64,
}

impl LocalUi {
    pub fn new() -> Self {
        Self {
            page: Page::Home,
            button: Button::new(),
            changed: false,
            last_input: 0,
        }
    }
    pub fn active(&self) -> bool {
        !matches!(self.page, Page::Home)
    }
    pub fn show(&mut self, page: Page, now: u64) {
        self.page = page; // Dropping PinEntry clears its secret.
        self.last_input = now;
        self.changed = true;
    }
    pub fn close(&mut self, now: u64) {
        self.show(Page::Home, now);
        self.button = Button::new();
    }
    pub fn poll(&mut self, pressed: bool, now: u64) -> Option<Input> {
        let expired = match &self.page {
            Page::Pin(pin) => pin.expired(now),
            _ => self.active() && now.saturating_sub(self.last_input) >= IDLE_TIMEOUT_MS,
        };
        if expired {
            self.close(now);
            return None;
        }
        let input = self.button.poll(pressed, now);
        if input.is_some() {
            self.last_input = now;
        }
        input
    }
}

/// Offline authorization is never inherited by a newly arriving host.
pub fn host_must_relock<T: PartialEq>(authorized: bool, owner: Option<T>, source: T) -> bool {
    authorized && owner.as_ref() != Some(&source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_required_and_long_cancel_never_selects() {
        let mut b = Button::new();
        assert_eq!(b.poll(true, 0), None);
        assert_eq!(b.poll(false, 4000), None);
        assert_eq!(b.poll(true, 5000), None);
        assert_eq!(b.poll(true, 6000), None);
        assert_eq!(b.poll(true, 8000), None);
        assert_eq!(b.poll(false, 8100), Some(Input::Cancel));
        b.poll(true, 9000);
        assert_eq!(b.poll(false, 9020), None);
        b.poll(true, 10000);
        assert_eq!(b.poll(false, 11000), Some(Input::Select));
    }
    #[test]
    fn six_digits_need_separate_submit_and_restart_clears() {
        let mut p = PinEntry::new(0);
        for i in 0..6 {
            p.input(Input::Tap, i * 1000);
            assert!(matches!(
                p.input(Input::Select, i * 1000 + 100),
                PinAction::Editing
            ));
        }
        assert_eq!(p.active_digit(), None);
        assert!(matches!(p.input(Input::Tap, 7000), PinAction::Editing));
        assert_eq!(&*p.digits, b"000000");
        for i in 0..6 {
            p.input(Input::Select, 8000 + i * 1000);
        }
        let PinAction::Submit(pin) = p.input(Input::Select, 15000) else {
            panic!("missing submit")
        };
        assert_eq!(&*pin, b"000000");
        assert_eq!(&*p.digits, &[0; 6]);
    }
    #[test]
    fn cancel_timeout_and_digit_wrap_do_not_submit() {
        let mut p = PinEntry::new(0);
        for _ in 0..10 {
            p.input(Input::Tap, 1000);
        }
        assert_eq!(p.active_digit(), Some(0));
        assert!(matches!(p.input(Input::Select, 46_000), PinAction::Cancel));
        assert_eq!(&*p.digits, &[0; 6]);
        let mut p = PinEntry::new(0);
        assert!(matches!(p.input(Input::Cancel, 1000), PinAction::Cancel));
        assert_eq!(&*p.digits, &[0; 6]);
    }
    #[test]
    fn ui_idle_timeout_drops_pin() {
        let mut ui = LocalUi::new();
        ui.show(Page::Pin(PinEntry::new(0)), 0);
        ui.poll(false, 45_000);
        assert!(!ui.active());
    }
    #[test]
    fn only_same_host_can_keep_an_unlock() {
        assert!(host_must_relock(true, None, 1));
        assert!(host_must_relock(true, Some(1), 2));
        assert!(!host_must_relock(true, Some(1), 1));
        assert!(!host_must_relock(false, None, 1));
    }
}
