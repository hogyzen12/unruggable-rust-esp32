use anyhow::{anyhow, Context, Result};
use base64::Engine;
use clap::Parser;
use data_encoding::{BASE32, BASE32_NOPAD};
use ed25519_dalek::{Verifier, VerifyingKey, Signature};
use hmac::{Hmac, Mac};
use qrcode::{QrCode, render::svg};
use serialport::{SerialPort, SerialPortType};
use sha1::Sha1;
use std::fs;
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{str, thread};

type HmacSha1 = Hmac<Sha1>;

#[derive(Parser, Debug)]
#[command(version, about="ESP32 2FA integration tester")]
struct Args {
    /// Serial port to use (e.g., /dev/tty.usbserial-0001)
    #[arg(short, long)]
    port: Option<String>,

    /// Baud rate
    #[arg(long, default_value_t = 115200)]
    baud: u32,

    /// Issuer for otpauth URI
    #[arg(long, default_value = "unruggable")]
    issuer: String,

    /// Account label for otpauth URI
    #[arg(long, default_value = "user@unruggable.com")]
    account: String,

    /// Headless mode: auto-confirm/unlock without scanning, using local TOTP
    #[arg(long, default_value_t = false)]
    headless: bool,

    /// Message to sign
    #[arg(long, default_value = "hello from twofa tester")]
    message: String,

    /// Command read timeout (ms)
    #[arg(long, default_value_t = 2000)]
    timeout_ms: u64,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn open_serial(args: &Args) -> Result<Box<dyn SerialPort>> {
    let port = if let Some(p) = &args.port {
        p.clone()
    } else {
        // Try to auto-detect a likely USB serial port
        let ports = serialport::available_ports().context("list ports")?;
        let mut best = None::<String>;
        for p in &ports {
            if let SerialPortType::UsbPort(info) = &p.port_type {
                if p.port_name.contains("usbserial")
                    || p.port_name.contains("usbmodem")
                    || p.port_name.contains("SLAB")
                    || info.product.as_deref().unwrap_or("").contains("CP210")
                    || info.product.as_deref().unwrap_or("").contains("USB")
                {
                    best = Some(p.port_name.clone());
                    break;
                }
            } else if p.port_name.contains("SLAB") || p.port_name.contains("usbserial") {
                best = Some(p.port_name.clone());
            }
        }
        best.ok_or_else(|| anyhow!("No port auto-detected; pass --port"))?
    };

    let sp = serialport::new(&port, args.baud)
        .timeout(Duration::from_millis(args.timeout_ms))
        .open()
        .with_context(|| format!("open {}", port))?;

    println!("Opened {}", port);
    thread::sleep(Duration::from_millis(250));
    Ok(sp)
}

fn write_line(sp: &mut dyn SerialPort, line: &str) -> Result<()> {
    let mut s = line.as_bytes().to_vec();
    s.push(b'\n');
    sp.write_all(&s)?;
    sp.flush()?;
    Ok(())
}

fn read_line(sp: &mut dyn SerialPort, timeout_ms: u64) -> Result<String> {
    let start = std::time::Instant::now();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 64];
    loop {
        match sp.read(&mut tmp) {
            Ok(n) if n > 0 => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line = &buf[..pos];
                    return Ok(String::from_utf8_lossy(line).trim().to_string());
                }
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        if start.elapsed() > Duration::from_millis(timeout_ms) {
            return Err(anyhow!("timeout waiting for line"));
        }
    }
}

fn b32_decode_any(s: &str) -> Result<Vec<u8>> {
    if s.contains('=') {
        Ok(BASE32.decode(s.as_bytes())?)
    } else {
        Ok(BASE32_NOPAD.decode(s.as_bytes())?)
    }
}

fn totp(secret: &[u8], unix: u64, period: u64, _digits: u32) -> String {
    let counter = unix / period;
    let msg = counter.to_be_bytes();
    let mut mac = HmacSha1::new_from_slice(secret).unwrap();
    mac.update(&msg);
    let digest = mac.finalize().into_bytes();
    let off = (digest[19] & 0x0f) as usize;
    let dbc = ((u32::from(digest[off]) & 0x7f) << 24)
        | ((u32::from(digest[off + 1])) << 16)
        | ((u32::from(digest[off + 2])) << 8)
        | (u32::from(digest[off + 3]));
    let code = dbc % 1_000_000;
    format!("{:06}", code)
}

fn save_qr_svg(uri: &str, path: &str) -> Result<()> {
    let code = QrCode::new(uri.as_bytes())?;
    // Specify the Pixel type for the renderer to fix type inference
    let svg_txt: String = code
        .render::<svg::Color>()
        .min_dimensions(256, 256)
        .build();
    fs::write(path, svg_txt)?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut sp = open_serial(&args)?;

    // 1) GET_PUBKEY
    write_line(&mut *sp, "GET_PUBKEY")?;
    let pubkey_line = read_line(&mut *sp, args.timeout_ms)?;
    println!("< {}", pubkey_line);
    let base58_pk = pubkey_line
        .strip_prefix("PUBKEY:")
        .ok_or_else(|| anyhow!("unexpected GET_PUBKEY response"))?;
    let pk_bytes = bs58::decode(base58_pk).into_vec()?;
    if pk_bytes.len() != 32 {
        return Err(anyhow!("verifying key must be 32 bytes"));
    }
    let verifying_key = VerifyingKey::from_bytes(&pk_bytes.try_into().unwrap())
        .map_err(|e| anyhow!("bad pubkey: {:?}", e))?;

    // 2) OTP_BEGIN → returns secret + metadata
    write_line(&mut *sp, "OTP_BEGIN")?;
    let begin_line = read_line(&mut *sp, args.timeout_ms)?;
    println!("< {}", begin_line);

    let secret_b32 = begin_line
        .strip_prefix("OTP_SECRET:")
        .and_then(|s| s.split(';').next())
        .ok_or_else(|| anyhow!("bad OTP_BEGIN response"))?
        .to_string();

    // parse optional metadata
    let mut digits = 6u32;
    let mut period = 30u64;
    for kv in begin_line.split(';').skip(1) {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "DIGITS" => digits = v.parse().unwrap_or(6),
                "PERIOD" => period = v.parse().unwrap_or(30),
                _ => {}
            }
        }
    }

    // Build otpauth URI + QR (SVG)
    let label_raw = format!("{}:{}", args.issuer, args.account);
    let label = urlencoding::encode(&label_raw).into_owned();
    let issuer_q = urlencoding::encode(&args.issuer).into_owned();
    let uri = format!(
        "otpauth://totp/{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        label, secret_b32, issuer_q, digits, period
    );
    println!("otpauth URI:\n{}", uri);
    save_qr_svg(&uri, "totp-setup.svg")?;
    println!("Saved QR to totp-setup.svg");
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("totp-setup.svg").status();
    }

    // 3) Confirm: either manual or headless
    let secret_bytes = b32_decode_any(&secret_b32)?;
    let unix = now_unix();
    let confirm_code = if args.headless {
        let code = totp(&secret_bytes, unix, period, digits);
        println!("(headless) confirm code = {}", code);
        code
    } else {
        print!("Enter code from your authenticator: ");
        std::io::stdout().flush().unwrap();
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        s.trim().to_string()
    };

    write_line(&mut *sp, &format!("OTP_CONFIRM:{}:{}", confirm_code, unix))?;
    let conf_line = read_line(&mut *sp, args.timeout_ms)?;
    println!("< {}", conf_line);
    if conf_line.trim() != "OTP_CONFIRMED" {
        return Err(anyhow!("confirmation failed: {}", conf_line));
    }

    // 4) Unlock (ensure a fresh step; wait if needed)
    let mut unix2 = now_unix();
    if unix2 / period == unix / period {
        let sleep_ms = (period - (unix2 % period) + 1) * 1000;
        println!("Waiting {} ms for next TOTP step...", sleep_ms);
        thread::sleep(Duration::from_millis(sleep_ms));
        unix2 = now_unix();
    }
    let unlock_code = if args.headless {
        let code = totp(&secret_bytes, unix2, period, digits);
        println!("(headless) unlock code = {}", code);
        code
    } else {
        print!("Enter a fresh code to unlock: ");
        std::io::stdout().flush().unwrap();
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        s.trim().to_string()
    };

    write_line(&mut *sp, &format!("OTP_UNLOCK:{}:{}", unlock_code, unix2))?;
    let unl_line = read_line(&mut *sp, args.timeout_ms)?;
    println!("< {}", unl_line);
    let _ = unl_line
        .strip_prefix("UNLOCKED_UNTIL:")
        .ok_or_else(|| anyhow!("unlock failed"))?;

    // 5) SIGN test (press BOOT on the device)
    let msg_bytes = args.message.as_bytes();
    let msg_b64 = base64::engine::general_purpose::STANDARD.encode(msg_bytes);
    println!("Requesting SIGN (press BOOT on device)...");
    write_line(&mut *sp, &format!("SIGN:{}", msg_b64))?;
    let sig_line = read_line(&mut *sp, args.timeout_ms * 10)?; // allow time for button
    println!("< {}", sig_line);

    let sig_b64 = sig_line
        .strip_prefix("SIGNATURE:")
        .ok_or_else(|| anyhow!("bad SIGN response"))?;
    let sig_bytes = base64::engine::general_purpose::STANDARD.decode(sig_b64)?;
    if sig_bytes.len() != 64 {
        return Err(anyhow!("signature must be 64 bytes"));
    }
    let sig = Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow!("bad signature: {:?}", e))?;

    verifying_key
        .verify(msg_bytes, &sig)
        .map_err(|_| anyhow!("signature verification failed"))?;
    println!("✅ Signature verified with device pubkey.");
    println!("All tests passed.");
    Ok(())
}
