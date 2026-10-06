use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey};
use fn_dsa::{VerifyingKey, VerifyingKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
use serialport::{ClearBuffer, SerialPort};
use sha2::{Digest, Sha256};
use solana_falcon512::{
    Falcon512Pubkey, Falcon512Signature, FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN,
};
use std::{
    collections::BTreeMap,
    env, fs,
    thread::sleep,
    time::{Duration, Instant},
};

const BAUD: u32 = 115_200;
const SE050_PUBKEY_BYTES: usize = 32;
const ED25519_SIG_BYTES: usize = 64;
const BINDING_TAG: &[u8] = b"unruggable-bind-v2";
const HYBRID_TX_TAG: &[u8] = b"unruggable-hybrid-solana-tx-v1";

struct Config {
    port: String,
    pin: String,
    sample_tx_b64: Option<String>,
    read_timeout: Duration,
    action_timeout: Duration,
    open_wait: Duration,
    initial_wipe: bool,
    final_wipe: bool,
}

fn main() -> Result<()> {
    let config = Config::from_args()?;
    run(config)
}

impl Config {
    fn from_args() -> Result<Self> {
        let mut port: Option<String> = None;
        let mut pin = env::var("TEST_PIN").unwrap_or_else(|_| "246810".to_string());
        let mut sample_tx_b64 = env::var("SAMPLE_TX_B64").ok();
        let mut read_timeout = Duration::from_millis(4_000);
        let mut action_timeout = Duration::from_millis(60_000);
        let mut open_wait = Duration::from_millis(2_000);
        let mut initial_wipe = false;
        let mut final_wipe = true;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => {
                    port = Some(next_arg(&mut args, "--port")?);
                }
                "--pin" => {
                    pin = next_arg(&mut args, "--pin")?;
                }
                "--sample-tx-b64" => {
                    sample_tx_b64 = Some(next_arg(&mut args, "--sample-tx-b64")?);
                }
                "--sample-tx-file" => {
                    let path = next_arg(&mut args, "--sample-tx-file")?;
                    sample_tx_b64 = Some(
                        fs::read_to_string(&path)
                            .with_context(|| format!("failed to read sample tx file: {path}"))?
                            .trim()
                            .to_string(),
                    );
                }
                "--read-timeout-ms" => {
                    read_timeout =
                        Duration::from_millis(next_arg(&mut args, "--read-timeout-ms")?.parse()?);
                }
                "--action-timeout-ms" => {
                    action_timeout =
                        Duration::from_millis(next_arg(&mut args, "--action-timeout-ms")?.parse()?);
                }
                "--open-wait-ms" => {
                    open_wait =
                        Duration::from_millis(next_arg(&mut args, "--open-wait-ms")?.parse()?);
                }
                "--initial-wipe" => {
                    initial_wipe = true;
                }
                "--no-final-wipe" => {
                    final_wipe = false;
                }
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                _ => bail!("unknown argument: {arg}"),
            }
        }

        let port = port
            .or_else(|| env::var("ESP_PORT").ok())
            .ok_or_else(|| anyhow!("missing required --port or ESP_PORT"))?;

        ensure!(
            pin.len() == 6 && pin.chars().all(|ch| ch.is_ascii_digit()),
            "PIN must be exactly 6 digits"
        );

        Ok(Self {
            port,
            pin,
            sample_tx_b64,
            read_timeout,
            action_timeout,
            open_wait,
            initial_wipe,
            final_wipe,
        })
    }
}

fn print_usage() {
    println!(
        "Usage:\n  cargo run --manifest-path tools/hybrid-harness/Cargo.toml -- --port /dev/cu.usbmodemXXXX [options]\n\
\nOptions:\n  --pin <digits>            6-digit PIN. Default: 246810\n  --sample-tx-b64 <b64>     Optional payload to test SIGN and SIGN_HYBRID\n  --sample-tx-file <path>   File containing base64 payload to test SIGN and SIGN_HYBRID\n  --read-timeout-ms <n>     Default command wait time in ms. Default: 4000\n  --action-timeout-ms <n>   Long command wait time in ms. Default: 60000\n  --open-wait-ms <n>        Delay after opening serial port in ms. Default: 2000\n  --initial-wipe            Wipe the device before verification. Default: preserve current state\n  --no-final-wipe           Leave device configured at the end\n  -h, --help                Show this help text"
    );
}

fn prompt_hold(label: &str) {
    println!("--- device action --- {label}: press and hold the hardware button when prompted");
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn run(config: Config) -> Result<()> {
    println!("============================================================");
    println!("ESP32-S3 Hybrid Harness");
    println!("============================================================");
    println!("Port: {}", config.port);
    println!("PIN: {}", config.pin);

    let mut port = serialport::new(&config.port, BAUD)
        .timeout(Duration::from_millis(50))
        .open()
        .with_context(|| format!("failed to open serial port {}", config.port))?;

    sleep(config.open_wait);
    port.clear(ClearBuffer::Input).ok();

    let boot = read_response(&mut *port, Duration::from_millis(1_500))?;
    if !boot.is_empty() {
        println!("BOOT <<< {}", boot.replace('\n', " | "));
    }

    let mut info = if config.initial_wipe {
        section("TEST 1: WIPE TO BLANK");
        prompt_hold("wipe to blank");
        let response = send_command(&mut *port, "WIPE_KEYS", config.action_timeout)?;
        expect_contains("WIPE_KEYS", &response, "WIPED")?;

        let response = send_command(&mut *port, "GET_INFO", config.read_timeout)?;
        expect_info_field(&response, "AUTH_MODE", "UNSET")?;
        expect_info_field(&response, "FINALIZED", "0")?;
        expect_info_field(&response, "LOCKED", "0")?;
        expect_info_field(&response, "RETRIES_LEFT", "10")?;
        expect_info_field(&response, "KEY_STATE", "UNINITIALIZED")?;
        expect_info_field(&response, "PQC_STATE", "UNINITIALIZED")?;

        let response = send_command(&mut *port, "GET_PUBKEY", config.read_timeout)?;
        expect_contains("GET_PUBKEY", &response, "ERROR:WALLET_NOT_INITIALIZED")?;

        let response = send_command(&mut *port, "SE050_STATUS", config.read_timeout)?;
        expect_contains("SE050_STATUS", &response, "SE050_BLANK")?;

        let response = send_command(&mut *port, "PQC_STATUS", config.read_timeout)?;
        expect_contains("PQC_STATUS", &response, "PQC_BLANK")?;

        section("TEST 2: OTP DISABLED");
        let response = send_command(&mut *port, "SET_MODE:OTP_BEGIN", config.read_timeout)?;
        expect_contains("SET_MODE:OTP_BEGIN", &response, "ERROR:PIN_REQUIRED")?;

        parse_info(
            &send_command(&mut *port, "GET_INFO", config.read_timeout)
                .context("failed to fetch post-wipe device info")?,
        )?
    } else {
        section("TEST 1: DEVICE STATE");
        let response = send_command(&mut *port, "GET_INFO", config.read_timeout)?;
        let info = parse_info(&response)?;
        println!(
            "Detected AUTH_MODE={}, KEY_STATE={}, PQC_STATE={}",
            info.get("AUTH_MODE").map(String::as_str).unwrap_or("?"),
            info.get("KEY_STATE").map(String::as_str).unwrap_or("?"),
            info.get("PQC_STATE").map(String::as_str).unwrap_or("?"),
        );
        info
    };

    let wallet_blank = info
        .get("AUTH_MODE")
        .map(|value| value == "UNSET")
        .unwrap_or(false);

    if wallet_blank {
        section("TEST 3: PIN SETUP CREATES WALLET");
        prompt_hold("pin setup");
        let response = send_command(
            &mut *port,
            &format!("SET_MODE:PIN:{}", config.pin),
            config.action_timeout,
        )?;
        expect_contains("SET_MODE:PIN", &response, "MODE_SET:PIN")?;

        let response = send_command(&mut *port, "GET_INFO", config.read_timeout)?;
        expect_info_field(&response, "AUTH_MODE", "PIN")?;
        expect_info_field(&response, "FINALIZED", "1")?;
        expect_info_field(&response, "LOCKED", "0")?;
        expect_info_field(&response, "RETRIES_LEFT", "10")?;
        expect_info_field(&response, "KEY_STATE", "READY")?;
        expect_info_field(&response, "PQC_STATE", "UNINITIALIZED")?;
        info = parse_info(&response)?;
    } else {
        ensure!(
            info.get("AUTH_MODE").map(String::as_str) == Some("PIN"),
            "unsupported AUTH_MODE for harness: {:?}",
            info.get("AUTH_MODE")
        );
        ensure!(
            info.get("KEY_STATE").map(String::as_str) == Some("READY"),
            "device wallet is not ready: {:?}",
            info.get("KEY_STATE")
        );
    }

    let response = send_command(&mut *port, "GET_PUBKEY", config.read_timeout)?;
    let se050_pubkey_b58 = prefixed_value(&response, "PUBKEY:")?;
    let se050_pubkey = bs58::decode(se050_pubkey_b58)
        .into_vec()
        .context("failed to decode base58 SE050 pubkey")?;
    ensure!(
        se050_pubkey.len() == SE050_PUBKEY_BYTES,
        "unexpected SE050 pubkey length: {}",
        se050_pubkey.len()
    );
    let se050_pubkey: [u8; SE050_PUBKEY_BYTES] = se050_pubkey
        .try_into()
        .map_err(|_| anyhow!("failed to convert SE050 pubkey to array"))?;

    let response = send_command(&mut *port, "SE050_STATUS", config.read_timeout)?;
    expect_contains("SE050_STATUS", &response, "SE050_OK")?;

    section("TEST 4: PIN UNLOCK");
    let response = send_command(
        &mut *port,
        &format!("UNLOCK:PIN:{}", config.pin),
        config.read_timeout,
    )?;
    expect_line_prefix("UNLOCK:PIN", &response, "UNLOCKED_UNTIL:")?;

    section("TEST 5: INIT PQC KEY");
    let response = send_command(&mut *port, "PQC_STATUS", config.read_timeout)?;
    let (pqc_pubkey_b64, binding_sig_b64, pqc_pubkey, binding_sig) =
        if response.contains("PQC_BLANK") {
            prompt_hold("PQC init");
            let response = send_command(&mut *port, "PQC_INIT", config.action_timeout)?;
            expect_contains("PQC_INIT", &response, "PQC_READY")?;
            let pqc_pubkey_b64 = prefixed_value(&response, "PQC_PUBKEY:")?.to_string();
            let binding_sig_b64 = prefixed_value(&response, "PQC_BINDING_SIG:")?.to_string();
            let pqc_pubkey = STANDARD
                .decode(&pqc_pubkey_b64)
                .context("failed to decode PQC public key")?;
            let binding_sig = STANDARD
                .decode(&binding_sig_b64)
                .context("failed to decode PQC binding signature")?;
            ensure!(
                binding_sig.len() == ED25519_SIG_BYTES,
                "unexpected binding signature length: {}",
                binding_sig.len()
            );
            (pqc_pubkey_b64, binding_sig_b64, pqc_pubkey, binding_sig)
        } else {
            expect_contains("PQC_STATUS", &response, "PQC_OK")?;
            let response = send_command(&mut *port, "PQC_GET_PUBKEY", config.read_timeout)?;
            let pqc_pubkey_b64 = prefixed_value(&response, "PQC_PUBKEY:")?.to_string();
            let pqc_pubkey = STANDARD
                .decode(&pqc_pubkey_b64)
                .context("failed to decode PQC public key")?;

            let response = send_command(&mut *port, "PQC_GET_BINDING", config.read_timeout)?;
            let binding_sig_b64 = prefixed_value(&response, "PQC_BINDING_SIG:")?.to_string();
            let binding_sig = STANDARD
                .decode(&binding_sig_b64)
                .context("failed to decode PQC binding signature")?;
            ensure!(
                binding_sig.len() == ED25519_SIG_BYTES,
                "unexpected binding signature length: {}",
                binding_sig.len()
            );
            (pqc_pubkey_b64, binding_sig_b64, pqc_pubkey, binding_sig)
        };

    let response = send_command(&mut *port, "GET_INFO", config.read_timeout)?;
    expect_info_field(&response, "PQC_STATE", "READY")?;

    let response = send_command(&mut *port, "PQC_GET_PUBKEY", config.read_timeout)?;
    ensure!(
        prefixed_value(&response, "PQC_PUBKEY:")? == pqc_pubkey_b64,
        "PQC_GET_PUBKEY returned a different key"
    );

    let response = send_command(&mut *port, "PQC_GET_BINDING", config.read_timeout)?;
    ensure!(
        prefixed_value(&response, "PQC_BINDING_SIG:")? == binding_sig_b64,
        "PQC_GET_BINDING returned a different binding signature"
    );

    verify_binding_signature(&se050_pubkey, &pqc_pubkey, &binding_sig)?;
    println!("Binding signature verified.");

    section("TEST 6: VECTOR FALCON DIGEST");
    let vector_digest = build_vector_demo_digest(&pqc_pubkey);
    let vector_digest_b64 = STANDARD.encode(vector_digest);
    prompt_hold("Vector Falcon digest sign");
    let response = send_command(
        &mut *port,
        &format!("PQC_SIGN_VECTOR_DIGEST:{vector_digest_b64}"),
        config.action_timeout,
    )?;
    let vector_sig = STANDARD
        .decode(prefixed_value(&response, "PQC_VECTOR_SIGNATURE:")?)
        .context("failed to decode vector Falcon signature")?;
    let returned_vector_identity = STANDARD
        .decode(prefixed_value(&response, "PQC_VECTOR_IDENTITY:")?)
        .context("failed to decode vector identity")?;
    let returned_vector_pubkey_b64 = prefixed_value(&response, "PQC_PUBKEY:")?;

    ensure!(
        returned_vector_pubkey_b64 == pqc_pubkey_b64,
        "PQC_SIGN_VECTOR_DIGEST returned a different PQC public key"
    );

    let expected_vector_identity = build_vector_identity(&pqc_pubkey);
    ensure!(
        returned_vector_identity == expected_vector_identity,
        "PQC_SIGN_VECTOR_DIGEST returned a different vector identity"
    );
    verify_vector_signature(&pqc_pubkey, &vector_digest, &vector_sig)?;
    println!("Vector-compatible Falcon digest signature verified.");

    if let Some(sample_tx_b64) = &config.sample_tx_b64 {
        let payload = STANDARD
            .decode(sample_tx_b64.trim())
            .context("failed to decode sample tx base64")?;

        section("TEST 7: SIGN");
        prompt_hold("Ed25519 sign");
        let response = send_command(
            &mut *port,
            &format!("SIGN:{}", sample_tx_b64.trim()),
            config.action_timeout,
        )?;
        let ed25519_sig_b64 = prefixed_value(&response, "SIGNATURE:")?;
        let ed25519_sig = STANDARD
            .decode(ed25519_sig_b64)
            .context("failed to decode SIGN signature")?;
        verify_ed25519_signature(&se050_pubkey, &payload, &ed25519_sig)?;
        println!("SIGN Ed25519 signature verified.");

        section("TEST 8: SIGN_HYBRID");
        prompt_hold("hybrid sign");
        let response = send_command(
            &mut *port,
            &format!("SIGN_HYBRID:{}", sample_tx_b64.trim()),
            config.action_timeout,
        )?;

        let hybrid_ed25519_sig = STANDARD
            .decode(prefixed_value(&response, "SIGNATURE:")?)
            .context("failed to decode hybrid Ed25519 signature")?;
        let pqc_sig = STANDARD
            .decode(prefixed_value(&response, "PQC_SIGNATURE:")?)
            .context("failed to decode PQC signature")?;
        let returned_pqc_pubkey_b64 = prefixed_value(&response, "PQC_PUBKEY:")?;
        let returned_binding_sig_b64 = prefixed_value(&response, "PQC_BINDING_SIG:")?;

        ensure!(
            returned_pqc_pubkey_b64 == pqc_pubkey_b64,
            "SIGN_HYBRID returned a different PQC public key"
        );
        ensure!(
            returned_binding_sig_b64 == binding_sig_b64,
            "SIGN_HYBRID returned a different binding signature"
        );

        verify_ed25519_signature(&se050_pubkey, &payload, &hybrid_ed25519_sig)?;
        verify_binding_signature(&se050_pubkey, &pqc_pubkey, &binding_sig)?;
        verify_pqc_signature(&se050_pubkey, &payload, &pqc_pubkey, &pqc_sig)?;
        println!("SIGN_HYBRID Ed25519 and FN-DSA signatures verified.");
    } else {
        println!("SIGN and SIGN_HYBRID skipped. Pass --sample-tx-b64 or --sample-tx-file to exercise them.");
    }

    if config.final_wipe {
        section("TEST 9: FINAL WIPE");
        prompt_hold("final wipe");
        let response = send_command(&mut *port, "WIPE_KEYS", config.action_timeout)?;
        expect_contains("WIPE_KEYS", &response, "WIPED")?;

        let response = send_command(&mut *port, "GET_INFO", config.read_timeout)?;
        expect_info_field(&response, "AUTH_MODE", "UNSET")?;
        expect_info_field(&response, "KEY_STATE", "UNINITIALIZED")?;
        expect_info_field(&response, "PQC_STATE", "UNINITIALIZED")?;
    }

    println!("\nHybrid harness passed.");
    Ok(())
}

fn section(title: &str) {
    println!("\n============================================================");
    println!("{title}");
    println!("============================================================");
}

fn send_command(port: &mut dyn SerialPort, command: &str, timeout: Duration) -> Result<String> {
    println!("\n>>> {command}");
    port.write_all(command.as_bytes())
        .with_context(|| format!("failed to write command: {command}"))?;
    port.write_all(b"\n")
        .with_context(|| format!("failed to terminate command: {command}"))?;
    port.flush()
        .with_context(|| format!("failed to flush command: {command}"))?;

    let mut combined = read_response(port, timeout)?;
    let barrier = ping_barrier(port, timeout)?;
    if !barrier.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&barrier);
    }

    let response = strip_pong_lines(&combined);
    if !response.is_empty() {
        println!("<<< {}", response.replace('\n', "\n<<< "));
    }
    Ok(response)
}

fn read_response(port: &mut dyn SerialPort, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    let mut saw_data = false;
    let mut last_data = Instant::now();

    loop {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                saw_data = true;
                last_data = Instant::now();
            }
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => return Err(err).context("serial read failed"),
        }

        let now = Instant::now();
        if saw_data && now.duration_since(last_data) >= Duration::from_millis(200) {
            break;
        }
        if now >= deadline {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&out)
        .replace('\r', "")
        .trim()
        .to_string())
}

fn ping_barrier(port: &mut dyn SerialPort, timeout: Duration) -> Result<String> {
    port.write_all(b"PING\n")
        .context("failed to write PING barrier")?;
    port.flush().context("failed to flush PING barrier")?;
    read_response(port, timeout)
}

fn strip_pong_lines(response: &str) -> String {
    response
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "PONG")
        .collect::<Vec<_>>()
        .join("\n")
}

fn prefixed_value<'a>(response: &'a str, prefix: &str) -> Result<&'a str> {
    for line in response
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix(prefix) {
            return Ok(value.trim());
        }
    }
    bail!("response did not contain prefix {prefix:?}: {response:?}")
}

fn expect_line_prefix<'a>(label: &str, response: &'a str, prefix: &str) -> Result<&'a str> {
    prefixed_value(response, prefix).with_context(|| format!("{label} missing {prefix}"))
}

fn expect_contains(label: &str, response: &str, needle: &str) -> Result<()> {
    ensure!(
        response.contains(needle),
        "{label} did not contain {needle:?}: {response:?}"
    );
    Ok(())
}

fn expect_info_field(response: &str, key: &str, expected: &str) -> Result<()> {
    let info = parse_info(response)?;
    let actual = info
        .get(key)
        .ok_or_else(|| anyhow!("GET_INFO missing field {key}"))?;
    ensure!(
        actual == expected,
        "GET_INFO field {key} expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

fn parse_info(response: &str) -> Result<BTreeMap<String, String>> {
    let line = response
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("INFO;"))
        .ok_or_else(|| anyhow!("GET_INFO response missing INFO line: {response:?}"))?;

    let mut out = BTreeMap::new();
    for part in line["INFO;".len()..].split(';') {
        let mut pieces = part.splitn(2, '=');
        let key = pieces
            .next()
            .ok_or_else(|| anyhow!("malformed GET_INFO key segment"))?;
        let value = pieces
            .next()
            .ok_or_else(|| anyhow!("malformed GET_INFO value for {key}"))?;
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn verify_binding_signature(
    se050_pubkey: &[u8; SE050_PUBKEY_BYTES],
    pqc_pubkey: &[u8],
    signature: &[u8],
) -> Result<()> {
    let message = build_binding_message(se050_pubkey, pqc_pubkey);
    verify_ed25519_signature(se050_pubkey, &message, signature)
        .context("binding signature verification failed")
}

fn verify_pqc_signature(
    se050_pubkey: &[u8; SE050_PUBKEY_BYTES],
    payload: &[u8],
    pqc_pubkey: &[u8],
    signature: &[u8],
) -> Result<()> {
    let message = build_hybrid_message(se050_pubkey, payload);
    let verifying_key = VerifyingKeyStandard::decode(pqc_pubkey)
        .ok_or_else(|| anyhow!("failed to decode FN-DSA verifying key"))?;
    ensure!(
        verifying_key.verify(signature, &DOMAIN_NONE, &HASH_ID_RAW, &message),
        "FN-DSA verification failed"
    );
    Ok(())
}

fn verify_vector_signature(pqc_pubkey: &[u8], digest: &[u8], signature: &[u8]) -> Result<()> {
    ensure!(
        pqc_pubkey.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon public key length: {}",
        pqc_pubkey.len()
    );
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );

    let pubkey = Falcon512Pubkey::try_from_slice(pqc_pubkey)
        .map_err(|err| anyhow!("solana-falcon512 rejected wire pubkey: {err:?}"))?;
    let prepared = pubkey
        .try_prepare_pubkey()
        .map_err(|err| anyhow!("solana-falcon512 failed to prepare pubkey: {err:?}"))?;
    let signature = Falcon512Signature::try_from_slice(signature)
        .map_err(|err| anyhow!("solana-falcon512 rejected signature: {err:?}"))?;

    ensure!(
        signature.verify_with_prepared(digest, &prepared),
        "solana-falcon512 verification failed"
    );
    Ok(())
}

fn verify_ed25519_signature(
    se050_pubkey: &[u8; SE050_PUBKEY_BYTES],
    message: &[u8],
    signature: &[u8],
) -> Result<()> {
    ensure!(
        signature.len() == ED25519_SIG_BYTES,
        "unexpected Ed25519 signature length: {}",
        signature.len()
    );
    let verifying_key = Ed25519VerifyingKey::from_bytes(se050_pubkey)
        .context("failed to decode SE050 Ed25519 public key")?;
    let signature_bytes: [u8; ED25519_SIG_BYTES] = signature
        .try_into()
        .map_err(|_| anyhow!("failed to convert Ed25519 signature to array"))?;
    let signature = Ed25519Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(message, &signature)
        .context("Ed25519 verification failed")
}

fn build_binding_message(se050_pubkey: &[u8; SE050_PUBKEY_BYTES], pqc_pubkey: &[u8]) -> Vec<u8> {
    let pubkey_hash = Sha256::digest(pqc_pubkey);
    let mut out =
        Vec::with_capacity(BINDING_TAG.len() + 1 + se050_pubkey.len() + 4 + pubkey_hash.len());
    out.extend_from_slice(BINDING_TAG);
    out.push(0);
    out.extend_from_slice(se050_pubkey);
    append_len_prefixed(&mut out, pubkey_hash.as_slice());
    out
}

fn build_hybrid_message(se050_pubkey: &[u8; SE050_PUBKEY_BYTES], payload: &[u8]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(HYBRID_TX_TAG.len() + 1 + se050_pubkey.len() + 4 + payload.len());
    out.extend_from_slice(HYBRID_TX_TAG);
    out.push(0);
    out.extend_from_slice(se050_pubkey);
    append_len_prefixed(&mut out, payload);
    out
}

fn build_vector_identity(pqc_pubkey: &[u8]) -> Vec<u8> {
    Sha256::digest(pqc_pubkey).to_vec()
}

fn build_vector_demo_digest(pqc_pubkey: &[u8]) -> [u8; 32] {
    let identity = build_vector_identity(pqc_pubkey);
    let mut hasher = Sha256::new();
    hasher.update(b"vector-demo-v1");
    hasher.update(identity);
    hasher.finalize().into()
}

fn append_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}
