//! USB bus and serial-reader lifetime, without application-protocol changes.
//!
//! Odd tokens identify live bus sessions. A loss or reset invalidates the token
//! permanently, even if the host reconnects before the command loop runs again.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbSession(pub u32);

impl UsbSession {
    #[inline(always)]
    pub fn connected(self) -> bool {
        self.0 & 1 != 0
    }

    #[inline(always)]
    pub fn permits(self, command_session: Self) -> bool {
        self.connected() && self.0 == command_session.0
    }
}

pub struct UsbBusMonitor {
    frame: u16,
    unchanged_ticks: u8,
    session: UsbSession,
}

/// The OS polls CDC IN endpoint 1 while the serial port has a reader. SOF alone
/// only proves the USB host is powered. A crashed/closed serial reader must also
/// invalidate authorization, even during a blocking physical review.
pub const SERIAL_POLL_TIMEOUT_TICKS: u16 = 50; // 500 ms at the pinned 100 Hz.

pub struct UsbHostMonitor {
    bus: UsbBusMonitor,
    bus_session: UsbSession,
    silent_ticks: u16,
    session: UsbSession,
}

impl UsbHostMonitor {
    pub const fn new() -> Self {
        Self {
            bus: UsbBusMonitor::new(),
            bus_session: UsbSession(0),
            silent_ticks: SERIAL_POLL_TIMEOUT_TICKS,
            session: UsbSession(0),
        }
    }

    #[inline(always)]
    pub fn observe(&mut self, frame: u16, reset: bool, serial_poll: bool) -> UsbSession {
        let bus = self.bus.observe(frame, reset);
        if bus != self.bus_session {
            self.invalidate();
            self.silent_ticks = SERIAL_POLL_TIMEOUT_TICKS;
            self.bus_session = bus;
        }
        if serial_poll && bus.connected() && !reset {
            self.silent_ticks = 0;
        } else if self.silent_ticks < SERIAL_POLL_TIMEOUT_TICKS {
            self.silent_ticks += 1;
        }
        if bus.connected() && self.silent_ticks < SERIAL_POLL_TIMEOUT_TICKS {
            if !self.session.connected() && self.session.0 < u32::MAX - 1 {
                self.session.0 += 1;
            }
        } else {
            self.invalidate();
        }
        self.session
    }

    #[inline(always)]
    fn invalidate(&mut self) {
        if self.session.connected() {
            self.session.0 += 1;
        }
    }
}

impl UsbBusMonitor {
    pub const fn new() -> Self {
        Self {
            frame: 0,
            unchanged_ticks: 0,
            session: UsbSession(0),
        }
    }

    /// Called at 100 Hz. Two unchanged samples tolerate a closely spaced tick
    /// after interrupt delay; a stopped bus normally locks within 20–30 ms.
    /// A latched hardware bus reset invalidates immediately on the next sample.
    #[inline(always)]
    pub fn observe(&mut self, frame: u16, bus_reset: bool) -> UsbSession {
        if bus_reset {
            self.invalidate();
            self.unchanged_ticks = 0;
        } else if frame != self.frame {
            self.unchanged_ticks = 0;
            if !self.session.connected() && self.session.0 < u32::MAX - 1 {
                self.session.0 += 1;
            }
        } else {
            if self.unchanged_ticks < 2 {
                self.unchanged_ticks += 1;
            }
            if self.unchanged_ticks == 2 && self.session.connected() {
                self.invalidate();
            }
        }
        self.frame = frame;
        self.session
    }

    #[inline(always)]
    fn invalidate(&mut self) {
        // Exhaustion fails closed rather than ever reusing a live token.
        if self.session.0 < u32::MAX - 1 {
            self.session.0 = (self.session.0 + 2) & !1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powered_usb_without_an_open_serial_reader_is_not_a_host() {
        let mut host = UsbHostMonitor::new();
        for frame in 1..1000 {
            assert!(!host.observe(frame, false, false).connected());
        }
        assert!(host.observe(1001, false, true).connected());
    }

    #[test]
    fn killed_app_locks_while_bus_frames_keep_running() {
        let mut host = UsbHostMonitor::new();
        let unlocked = host.observe(1, false, true);
        for tick in 1..SERIAL_POLL_TIMEOUT_TICKS {
            assert!(host.observe(tick + 1, false, false).permits(unlocked));
        }
        assert!(!host.observe(51, false, false).permits(unlocked));
        let reopened = host.observe(52, false, true);
        assert!(reopened.connected());
        assert!(!reopened.permits(unlocked));
    }

    #[test]
    fn idle_app_can_review_without_sending_commands() {
        let mut host = UsbHostMonitor::new();
        let review = host.observe(1, false, true);
        for tick in 1..20_000u32 {
            assert!(host
                .observe((tick % 2048) as u16, false, tick % 10 == 0)
                .permits(review));
        }
    }

    #[test]
    fn bus_loss_and_reset_revoke_even_with_serial_poll_bits() {
        let mut host = UsbHostMonitor::new();
        let original = host.observe(1, false, true);
        host.observe(1, false, true);
        assert!(!host.observe(1, false, true).permits(original));
        let next = host.observe(2, false, true);
        assert!(next.connected());
        assert!(!next.permits(original));
        assert!(!host.observe(3, true, true).connected());
        assert!(!host.observe(4, false, true).permits(next));
    }

    #[test]
    fn repeated_closed_ticks_do_not_consume_generations() {
        let mut host = UsbHostMonitor::new();
        host.observe(1, false, true);
        for tick in 2..100 {
            host.observe(tick, false, false);
        }
        let closed = host.session;
        for tick in 100..1000 {
            assert_eq!(host.observe(tick, false, false), closed);
        }
    }

    #[test]
    fn serial_generation_exhaustion_fails_closed() {
        let mut host = UsbHostMonitor::new();
        host.observe(1, false, true);
        host.session = UsbSession(u32::MAX - 2);
        host.observe(2, true, false);
        assert!(!host.observe(3, false, true).connected());
    }

    #[test]
    fn cable_loss_revokes_idle_unlocked_session_without_a_host_command() {
        let mut bus = UsbBusMonitor::new();
        let unlocked = bus.observe(10, false);
        assert!(unlocked.connected());
        assert!(bus.observe(10, false).permits(unlocked));
        assert!(!bus.observe(10, false).permits(unlocked));
        assert!(!bus.observe(10, false).connected());
    }

    #[test]
    fn reconnect_cannot_revive_pending_approval_or_signature_reply() {
        let mut bus = UsbBusMonitor::new();
        let pending_command = bus.observe(10, false);
        bus.observe(10, false);
        bus.observe(10, false);
        let reconnected = bus.observe(20, false);
        assert!(reconnected.connected());
        assert!(!reconnected.permits(pending_command));
        assert!(reconnected.permits(reconnected));
    }

    #[test]
    fn reset_even_with_advancing_frames_invalidates_old_token() {
        let mut bus = UsbBusMonitor::new();
        let pending_command = bus.observe(10, false);
        assert!(!bus.observe(20, true).connected());
        assert!(!bus.observe(30, false).permits(pending_command));
    }

    #[test]
    fn idle_serial_port_and_frame_wrap_do_not_expire_live_usb() {
        let mut bus = UsbBusMonitor::new();
        let unlocked = bus.observe(2040, false);
        for i in 1..100_000 {
            assert!(bus
                .observe(((2040 + i * 10) % 2048) as u16, false)
                .permits(unlocked));
        }
    }

    #[test]
    fn single_close_tick_does_not_report_loss() {
        let mut bus = UsbBusMonitor::new();
        let active = bus.observe(10, false);
        assert!(bus.observe(10, false).permits(active));
        assert!(bus.observe(11, false).permits(active));
    }

    #[test]
    fn generation_exhaustion_stays_locked() {
        let mut bus = UsbBusMonitor::new();
        bus.session = UsbSession(u32::MAX - 2);
        assert!(!bus.observe(10, true).connected());
        assert!(!bus.observe(20, false).connected());
        assert!(!bus.observe(30, true).connected());
    }
}
