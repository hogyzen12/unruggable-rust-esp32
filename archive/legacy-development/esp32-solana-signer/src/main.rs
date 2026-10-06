use ed25519_dalek::{SigningKey, Signer, VerifyingKey};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::hal::prelude::Peripherals;
use esp_idf_svc::hal::uart::UartDriver;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::sys::ESP_ERR_TIMEOUT;
use rand_core::OsRng;
use bs58;
use base64;
use base64::Engine;

// Add imports for deep sleep and GPIO isolation from ESP-IDF sys bindings
use esp_idf_sys::{rtc_gpio_isolate, esp_deep_sleep_start};

#[cfg(feature = "twofa")]
mod twofa;

fn load_or_generate_key(nvs: &mut EspNvs<NvsDefault>) -> anyhow::Result<SigningKey> {
    let key_name = "solana_key";
    let mut key_bytes = [0u8; 32];
    match nvs.get_raw(key_name, &mut key_bytes)? {
        Some(_) => Ok(SigningKey::from_bytes(&key_bytes)),
        None => {
            let mut csprng = OsRng;
            let signing_key = SigningKey::generate(&mut csprng);
            let key_bytes = signing_key.to_bytes();
            nvs.set_raw(key_name, &key_bytes)?;
            Ok(signing_key)
        }
    }
}

fn send_response(uart: &mut UartDriver, response: &str) -> anyhow::Result<()> {
    let response_with_newline = response.to_string() + "\n";
    let data = response_with_newline.as_bytes();
    let mut written = 0;
    while written < data.len() {
        written += uart.write(&data[written..])?;
    }
    Ok(())
}

#[cfg(feature = "twofa")]
fn device_unix_time() -> u64 {
    twofa::TwoFa::device_unix_time()
}

#[cfg(not(feature = "twofa"))]
fn device_unix_time() -> u64 {
    0
}

fn main() -> anyhow::Result<()> {
    let peripherals = Peripherals::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take()?;
    let mut nvs = EspNvs::new(nvs_partition, "solana_signer", true)?;
    let signing_key = load_or_generate_key(&mut nvs)?;
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let pubkey_bytes = verifying_key.to_bytes();
    let pubkey_base58 = bs58::encode(pubkey_bytes).into_string();

    let mut uart = UartDriver::new(
        peripherals.uart0,
        peripherals.pins.gpio1,
        peripherals.pins.gpio3,
        Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,
        Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,
        &Default::default(),
    )?;

    // Configure BOOT button (GPIO 0) as input with pull-up
    let mut button = PinDriver::input(peripherals.pins.gpio0)?;
    button.set_pull(Pull::Up)?;

    // Configure built-in LED on GPIO 2 as output
    let mut led = PinDriver::output(peripherals.pins.gpio2)?;

    // Initial LED state - off when idle
    led.set_low()?;

    // Startup: Brief blink when ready
    led.set_high()?;
    esp_idf_svc::hal::delay::FreeRtos::delay_ms(300);
    led.set_low()?;

    let mut buffer = String::new();

    #[cfg(feature = "twofa")]
    let mut unlocked_until: u64 = 0;

    loop {
        let mut byte = [0u8; 1];
        match uart.read(&mut byte, 1000) {
            Ok(1) => {
                let ch = byte[0] as char;
                if ch == '\n' {
                    let input = buffer.trim();

                    // ======== PUBKEY ========
                    if input == "GET_PUBKEY" {
                        // During pubkey request: Double flash
                        for _ in 0..2 {
                            led.set_high()?;
                            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                            led.set_low()?;
                            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                        }
                        let response = format!("PUBKEY:{}", pubkey_base58);
                        send_response(&mut uart, &response)?;

                    // ======== 2FA: OTP_BEGIN ========
                    } else if input == "OTP_BEGIN" {
                        #[cfg(feature = "twofa")]
                        {
                            match twofa::TwoFa::begin(&mut nvs) {
                                Ok(b32) => {
                                    // short blink
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(180);
                                    led.set_low()?;
                                    let resp = format!(
                                        "OTP_SECRET:{};ALGO=SHA1;DIGITS={};PERIOD={}",
                                        b32, twofa::OTP_DIGITS, twofa::OTP_PERIOD
                                    );
                                    send_response(&mut uart, &resp)?;
                                }
                                Err(e) => {
                                    for _ in 0..3 {
                                        led.set_high()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                        led.set_low()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    }
                                    send_response(&mut uart, &format!("ERROR:{}", e))?;
                                }
                            }
                        }
                        #[cfg(not(feature = "twofa"))]
                        {
                            send_response(&mut uart, "ERROR:OTP_DISABLED")?;
                        }

                    // ======== 2FA: OTP_CONFIRM:CODE[:UNIX] ========
                    } else if input.starts_with("OTP_CONFIRM:") {
                        #[cfg(feature = "twofa")]
                        {
                            let rest = &input["OTP_CONFIRM:".len()..];
                            let parts: Vec<&str> = rest.split(':').collect();
                            let code = parts.get(0).copied().unwrap_or("");
                            let unix = parts.get(1).and_then(|s| s.parse::<u64>().ok());
                            match twofa::TwoFa::confirm(&mut nvs, code, unix) {
                                Ok(()) => {
                                    // confirm blink (short, short, long)
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(300);
                                    led.set_low()?;
                                    send_response(&mut uart, "OTP_CONFIRMED")?;
                                }
                                Err(_) => {
                                    for _ in 0..4 {
                                        led.set_high()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(80);
                                        led.set_low()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(80);
                                    }
                                    send_response(&mut uart, "ERROR:OTP_BAD_CODE")?;
                                }
                            }
                        }
                        #[cfg(not(feature = "twofa"))]
                        {
                            send_response(&mut uart, "ERROR:OTP_DISABLED")?;
                        }

                    // ======== 2FA: OTP_UNLOCK:CODE[:UNIX] ========
                    } else if input.starts_with("OTP_UNLOCK:") {
                        #[cfg(feature = "twofa")]
                        {
                            let rest = &input["OTP_UNLOCK:".len()..];
                            let parts: Vec<&str> = rest.split(':').collect();
                            let code = parts.get(0).copied().unwrap_or("");
                            let unix = parts.get(1).and_then(|s| s.parse::<u64>().ok());

                            match twofa::TwoFa::unlock(&mut nvs, code, unix) {
                                Ok(until) => {
                                    unlocked_until = until;
                                    // Two short + one long blink
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(120);
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(350);
                                    led.set_low()?;
                                    let resp = format!("UNLOCKED_UNTIL:{}", unlocked_until);
                                    send_response(&mut uart, &resp)?;
                                }
                                Err(_) => {
                                    for _ in 0..4 {
                                        led.set_high()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(80);
                                        led.set_low()?;
                                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(80);
                                    }
                                    send_response(&mut uart, "ERROR:OTP_BAD_CODE")?;
                                }
                            }
                        }
                        #[cfg(not(feature = "twofa"))]
                        {
                            send_response(&mut uart, "ERROR:OTP_DISABLED")?;
                        }

                    // ======== SIGN (gated by 2FA window if enabled) ========
                    } else if input.starts_with("SIGN:") {
                        // If 2FA is enabled, require unlocked session
                        #[cfg(feature = "twofa")]
                        {
                            let now = twofa::TwoFa::device_unix_time();
                            if now > unlocked_until {
                                for _ in 0..3 {
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                }
                                send_response(&mut uart, "ERROR:LOCKED")?;
                                buffer.clear();
                                continue;
                            }
                        }

                        // Extract the base64 message after "SIGN:"
                        let base64_message = &input[5..];
                        match base64::engine::general_purpose::STANDARD.decode(base64_message) {
                            Ok(message_bytes) => {
                                // Waiting for the BOOT button: fast blink until pressed
                                let mut led_state = false;
                                while !button.is_low() {
                                    led_state = !led_state;
                                    if led_state { led.set_high()?; } else { led.set_low()?; }
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(200);
                                }

                                // Sign
                                let signature = signing_key.sign(&message_bytes);
                                let signature_bytes = signature.to_bytes();
                                let base64_signature = base64::engine::general_purpose::STANDARD.encode(&signature_bytes);

                                // Success: triple flash with longer third
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_low()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_low()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(450);
                                led.set_low()?;

                                let response = format!("SIGNATURE:{}", base64_signature);
                                send_response(&mut uart, &response)?;
                            }
                            Err(_) => {
                                for _ in 0..5 {
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                }
                                send_response(&mut uart, "ERROR:Invalid base64 encoding")?;
                            }
                        }

                    // ======== SHUTDOWN ========
                    } else if input == "SHUTDOWN" {
                        // Long blink then deep sleep
                        led.set_high()?;
                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000);
                        led.set_low()?;

                        unsafe {
                            rtc_gpio_isolate(0);
                            rtc_gpio_isolate(2);
                        }

                        send_response(&mut uart, "SHUTDOWN_OK")?;
                        unsafe {
                            esp_deep_sleep_start();
                        }

                    } else if !input.is_empty() {
                        // Unknown command
                        println!("Received unknown command: '{}'", input);
                        send_response(&mut uart, "ERROR:Unknown command")?;
                    }

                    buffer.clear();
                } else {
                    buffer.push(ch);
                }
            }
            Ok(0) => {}
            Ok(n) => unreachable!("Unexpected read size: {}", n),
            Err(e) => {
                if e.code() != ESP_ERR_TIMEOUT {
                    // Simplified error state: Rapid blinking
                    for _ in 0..10 {
                        led.set_high()?;
                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                        led.set_low()?;
                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                    }
                }
            }
        }
    }
}
