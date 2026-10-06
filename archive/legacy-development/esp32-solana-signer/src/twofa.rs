#![cfg(feature = "twofa")]

use anyhow::{anyhow, Result};
use data_encoding::BASE32_NOPAD;
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use esp_idf_sys as sys;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore}; // <-- bring RngCore into scope for fill_bytes
use sha1::Sha1;
use subtle::ConstantTimeEq;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

pub const OTP_BYTES: usize = 20;
pub const OTP_DIGITS: u32 = 6;
pub const OTP_PERIOD: u64 = 30;
pub const OTP_WINDOW: i32 = 1;
pub const UNLOCK_SECS: u64 = 120;

const OTP_SECRET_KEY: &str = "otp_secret";     // raw 20 bytes
const OTP_LASTSTEP_KEY: &str = "otp_last";     // raw u64 (LE)
const OTP_ENROLLED_KEY: &str = "otp_enrolled"; // raw u8 (0/1)

pub struct TwoFa;

impl TwoFa {
    /// ESP32 time (seconds). Uses RTC if set; falls back to SystemTime.
    pub fn device_unix_time() -> u64 {
        unsafe {
            let mut tv = sys::timeval { tv_sec: 0, tv_usec: 0 };
            if sys::gettimeofday(&mut tv, core::ptr::null_mut()) == 0 {
                if tv.tv_sec > 0 {
                    return tv.tv_sec as u64;
                }
            }
        }
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    /// Generate and persist a new secret, reset last step/enrolled.
    /// Returns Base32 (no padding, uppercase) for QR building on host.
    pub fn begin(nvs: &mut EspNvs<NvsDefault>) -> Result<String> {
        if Self::is_enrolled(nvs)? {
            return Err(anyhow!("already enrolled"));
        }
        let mut secret = [0u8; OTP_BYTES];
        OsRng.fill_bytes(&mut secret);

        nvs.set_raw(OTP_SECRET_KEY, &secret)?;
        set_u64(nvs, OTP_LASTSTEP_KEY, 0)?;
        set_u8(nvs, OTP_ENROLLED_KEY, 0)?;

        let b32 = BASE32_NOPAD.encode(&secret).to_uppercase();
        Ok(b32)
    }

    /// Confirm enrollment by verifying a single code.
    pub fn confirm(nvs: &mut EspNvs<NvsDefault>, code: &str, unix_opt: Option<u64>) -> Result<()> {
        let secret = get_secret(nvs)?.ok_or_else(|| anyhow!("secret missing"))?;
        let now = unix_opt.unwrap_or_else(Self::device_unix_time);
        let last = get_u64(nvs, OTP_LASTSTEP_KEY)?.unwrap_or(0);
        if let Some(accepted) = verify_code(code, &secret, now, last) {
            set_u64(nvs, OTP_LASTSTEP_KEY, accepted)?;
            set_u8(nvs, OTP_ENROLLED_KEY, 1)?;
            Ok(())
        } else {
            Err(anyhow!("bad code"))
        }
    }

    /// Verify a code and return an unlock-until timestamp on success.
    pub fn unlock(
        nvs: &mut EspNvs<NvsDefault>,
        code: &str,
        unix_opt: Option<u64>,
    ) -> Result<u64> {
        if !Self::is_enrolled(nvs)? {
            return Err(anyhow!("not enrolled"));
        }
        let secret = get_secret(nvs)?.ok_or_else(|| anyhow!("secret missing"))?;
        let now = unix_opt.unwrap_or_else(Self::device_unix_time);
        let last = get_u64(nvs, OTP_LASTSTEP_KEY)?.unwrap_or(0);

        if let Some(accepted) = verify_code(code, &secret, now, last) {
            set_u64(nvs, OTP_LASTSTEP_KEY, accepted)?;
            Ok(now + UNLOCK_SECS)
        } else {
            Err(anyhow!("bad code"))
        }
    }

    pub fn is_enrolled(nvs: &mut EspNvs<NvsDefault>) -> Result<bool> {
        Ok(get_u8(nvs, OTP_ENROLLED_KEY)?.unwrap_or(0) == 1)
    }
}

/* ---------------- internal helpers ---------------- */

fn get_secret(nvs: &mut EspNvs<NvsDefault>) -> Result<Option<[u8; OTP_BYTES]>> {
    let mut buf = [0u8; OTP_BYTES];
    match nvs.get_raw(OTP_SECRET_KEY, &mut buf)? {
        Some(slice) => {
            if slice.len() == OTP_BYTES {
                let mut out = [0u8; OTP_BYTES];
                out.copy_from_slice(slice);
                Ok(Some(out))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

fn set_u64(nvs: &mut EspNvs<NvsDefault>, key: &str, v: u64) -> Result<()> {
    nvs.set_raw(key, &v.to_le_bytes())?;
    Ok(())
}
fn get_u64(nvs: &mut EspNvs<NvsDefault>, key: &str) -> Result<Option<u64>> {
    let mut b = [0u8; 8];
    match nvs.get_raw(key, &mut b)? {
        Some(slice) if slice.len() == 8 => Ok(Some(u64::from_le_bytes(b))),
        _ => Ok(None),
    }
}
fn set_u8(nvs: &mut EspNvs<NvsDefault>, key: &str, v: u8) -> Result<()> {
    nvs.set_raw(key, &[v])?;
    Ok(())
}
fn get_u8(nvs: &mut EspNvs<NvsDefault>, key: &str) -> Result<Option<u8>> {
    let mut b = [0u8; 1];
    match nvs.get_raw(key, &mut b)? {
        Some(slice) if slice.len() == 1 => Ok(Some(b[0])),
        _ => Ok(None),
    }
}

fn hotp(secret: &[u8], counter: u64) -> u32 {
    let msg = counter.to_be_bytes();
    let mut mac = HmacSha1::new_from_slice(secret).unwrap();
    mac.update(&msg);
    let digest = mac.finalize().into_bytes();

    let off = (digest[19] & 0x0f) as usize;
    let dbc = ((u32::from(digest[off]) & 0x7f) << 24)
        | ((u32::from(digest[off + 1])) << 16)
        | ((u32::from(digest[off + 2])) << 8)
        | (u32::from(digest[off + 3]));
    // 6 digits
    dbc % 1_000_000
}

fn verify_code(code: &str, secret: &[u8], now: u64, last_step: u64) -> Option<u64> {
    if code.len() != OTP_DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let step_now = now / OTP_PERIOD;
    for w in -OTP_WINDOW..=OTP_WINDOW {
        let step = (step_now as i64 + w as i64) as u64;
        if step == last_step {
            continue; // prevent replay in window
        }
        let expected = format!("{:06}", hotp(secret, step));
        if expected.as_bytes().ct_eq(code.as_bytes()).into() {
            return Some(step);
        }
    }
    None
}
