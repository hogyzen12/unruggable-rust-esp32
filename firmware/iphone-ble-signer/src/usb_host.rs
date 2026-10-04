//! ESP32-S3 native USB Serial/JTAG bus monitor.
//!
//! The pinned ESP-IDF 5.2.3 driver exposes only a current connected boolean.
//! Sample the SOF frame counter, latched BUS_RESET and CDC IN polling, so a
//! disconnect/reconnect during button approval or chip I/O cannot be missed by
//! the main loop. Do not consume SOF interrupts used by IDF's own monitor.
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::usb_session::{UsbHostMonitor, UsbSession};

// ESP-IDF components/soc/esp32s3/include/soc/usb_serial_jtag_reg.h.
const INT_RAW: *const u32 = 0x6003_8008 as *const u32;
const INT_CLR: *mut u32 = 0x6003_8014 as *mut u32;
const FRAME_NUM: *const u32 = 0x6003_8024 as *const u32;
const BUS_RESET: u32 = 1 << 9;
const SERIAL_IN_TOKEN: u32 = 1 << 8;

struct TickMonitor(UnsafeCell<UsbHostMonitor>);
// Only the core-0 tick hook accesses this cell; other tasks read PUBLISHED.
unsafe impl Sync for TickMonitor {}
static MONITOR: TickMonitor = TickMonitor(UnsafeCell::new(UsbHostMonitor::new()));
static PUBLISHED: AtomicU32 = AtomicU32::new(0);

// Tick hooks also execute with flash cache disabled. Keep this entire path in
// IRAM: no allocation, locks, logging, SDK calls or out-of-line Rust helpers.
#[link_section = ".iram1.text"]
unsafe extern "C" fn sample_bus() {
    let raw = std::ptr::read_volatile(INT_RAW);
    let reset = raw & BUS_RESET != 0;
    let serial_poll = raw & SERIAL_IN_TOKEN != 0;
    if raw & (BUS_RESET | SERIAL_IN_TOKEN) != 0 {
        // The pinned IDF driver does not own either bit. Leave its actual
        // RX/TX-ready and SOF interrupts untouched. IN tokens include NAK polls,
        // so an idle open reader does not need to send application heartbeats.
        std::ptr::write_volatile(INT_CLR, raw & (BUS_RESET | SERIAL_IN_TOKEN));
    }
    let frame = (std::ptr::read_volatile(FRAME_NUM) & 0x7ff) as u16;
    let session = (*MONITOR.0.get()).observe(frame, reset, serial_poll);
    PUBLISHED.store(session.0, Ordering::Release);
}

pub fn start() -> anyhow::Result<()> {
    // These register addresses and timing are specific to the shipped target.
    const {
        assert!(esp_idf_sys::CONFIG_IDF_TARGET_ESP32S3 == 1);
    }
    const {
        assert!(esp_idf_sys::CONFIG_FREERTOS_HZ == 100);
    }
    esp_idf_sys::esp!(unsafe {
        esp_idf_sys::esp_register_freertos_tick_hook_for_cpu(Some(sample_bus), 0)
    })?;
    Ok(())
}

pub fn session() -> UsbSession {
    UsbSession(PUBLISHED.load(Ordering::Acquire))
}

pub fn is_active(command_session: UsbSession) -> bool {
    session().permits(command_session)
}
