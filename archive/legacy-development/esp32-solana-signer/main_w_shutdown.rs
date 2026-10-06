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
    loop {
        let mut byte = [0u8; 1];
        match uart.read(&mut byte, 1000) {
            Ok(1) => {
                let ch = byte[0] as char;
                if ch == '\n' {
                    let input = buffer.trim();
                    if input == "GET_PUBKEY" {
                        // During pubkey request: Double flash
                        for _ in 0..2 {
                            led.set_high()?;
                            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                            led.set_low()?;
                            esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                        }
                        
                        // Send response in the format our protocol expects
                        let response = format!("PUBKEY:{}", pubkey_base58);
                        send_response(&mut uart, &response)?;
                    } else if input.starts_with("SIGN:") {
                        // Extract the base64 message after "SIGN:"
                        let base64_message = &input[5..];
                        match base64::engine::general_purpose::STANDARD.decode(base64_message) {
                            Ok(message_bytes) => {
                                // Waiting for button: Fast blinking pattern
                                let mut led_state = false;
                                
                                // Wait for the BOOT button to be pressed
                                while !button.is_low() {
                                    // Toggle LED state every 200ms for a fast blink pattern
                                    led_state = !led_state;
                                    if led_state {
                                        led.set_high()?;
                                    } else {
                                        led.set_low()?;
                                    }
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(200);
                                }
                                
                                // Sign the message (no LED change during signing as it's quick)
                                let signature = signing_key.sign(&message_bytes);
                                let signature_bytes = signature.to_bytes();
                                let base64_signature = base64::engine::general_purpose::STANDARD.encode(&signature_bytes);
                                
                                // Success: Triple flash with longer third pulse
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_low()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                led.set_low()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(150);
                                
                                // Third, longer flash
                                led.set_high()?;
                                esp_idf_svc::hal::delay::FreeRtos::delay_ms(450); // 3x longer
                                led.set_low()?;
                                
                                // Send the response in the format our protocol expects
                                let response = format!("SIGNATURE:{}", base64_signature);
                                send_response(&mut uart, &response)?;
                            }
                            Err(_) => {
                                // Error: Rapid blinking
                                for _ in 0..5 {
                                    led.set_high()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                    led.set_low()?;
                                    esp_idf_svc::hal::delay::FreeRtos::delay_ms(100);
                                }
                                send_response(&mut uart, "ERROR:Invalid base64 encoding")?;
                            }
                        }
                    } else if input == "SHUTDOWN" {
                        // Shutdown command received: Prepare for safe disconnection by entering indefinite deep sleep
                    
                        // Step 1: Removed UART flush - unnecessary as writes are blocking and complete
                    
                        // Step 2: Turn off LED and signal shutdown with a long blink for user feedback
                        led.set_high()?;
                        esp_idf_svc::hal::delay::FreeRtos::delay_ms(1000); // 1-second blink to indicate shutdown starting
                        led.set_low()?;
                    
                        // Step 3: Isolate GPIOs to minimize current leakage in sleep
                        // GPIO0 (button) and GPIO2 (LED) are RTC GPIOs; isolate them as they're not needed for wake-up
                        unsafe {
                            rtc_gpio_isolate(0);
                            rtc_gpio_isolate(2);
                        }
                    
                        // Optional: If other GPIOs are used, isolate them similarly.
                        // UART pins (GPIO1 TX, GPIO3 RX) are not RTC GPIOs, so no isolation needed; they'll be powered down automatically.
                    
                        // New: Send confirmation response before sleeping
                        send_response(&mut uart, "SHUTDOWN_OK")?;
                    
                        // Step 4: Enter deep sleep indefinitely (no wake-up sources configured)
                        // This halts execution; device is now safe to disconnect
                        unsafe {
                            esp_deep_sleep_start();
                        }

                        // Code after this won't execute; the device resets on wake-up/power cycle
                    } else if !input.is_empty() {
                        // Unknown command - log what we received for debugging
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