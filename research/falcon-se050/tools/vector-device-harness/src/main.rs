use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey};
use serialport::{ClearBuffer, SerialPort};
use sha2::{Digest, Sha256};
use solana_falcon512::{
    Falcon512Pubkey, Falcon512Signature, FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN,
};
use solana_pubkey::Pubkey;
use std::{
    collections::BTreeMap,
    env,
    str::FromStr,
    thread::sleep,
    time::{Duration, Instant},
};

const BAUD: u32 = 115_200;
const VECTOR_PDA_SEED: &[u8] = b"vector";
const DEFAULT_VECTOR_FALCON_PROGRAM_ID: &str = "FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU";
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
const INSTRUCTIONS_SYSVAR_ID: &str = "Sysvar1nstructions1111111111111111111111111";
const INITIALIZE_DISCRIMINATOR: u8 = 0;
const ADVANCE_DISCRIMINATOR: u8 = 1;
const BINDING_TAG: &[u8] = b"unruggable-bind-v2";
const ED25519_SIG_BYTES: usize = 64;

#[derive(Clone)]
struct Config {
    port: String,
    pin: String,
    payer: Option<Pubkey>,
    nonce: Option<[u8; 32]>,
    vector_program_id: Pubkey,
    read_timeout: Duration,
    action_timeout: Duration,
    open_wait: Duration,
    auto_init: bool,
}

#[derive(Clone, Debug)]
struct ProtoAccountMeta {
    pubkey: Pubkey,
    is_signer: bool,
    is_writable: bool,
}

#[derive(Clone, Debug)]
struct ProtoInstruction {
    program_id: Pubkey,
    accounts: Vec<ProtoAccountMeta>,
    data: Vec<u8>,
}

fn main() -> Result<()> {
    let config = Config::from_args()?;
    run(config)
}

impl Config {
    fn from_args() -> Result<Self> {
        let mut port: Option<String> = None;
        let mut pin = env::var("TEST_PIN").unwrap_or_else(|_| "246810".to_string());
        let mut payer: Option<Pubkey> = None;
        let mut nonce: Option<[u8; 32]> = None;
        let mut vector_program_id = env::var("VECTOR_PROGRAM_ID")
            .unwrap_or_else(|_| DEFAULT_VECTOR_FALCON_PROGRAM_ID.to_string());
        let mut read_timeout = Duration::from_millis(4_000);
        let mut action_timeout = Duration::from_millis(35_000);
        let mut open_wait = Duration::from_millis(2_000);
        let mut auto_init = true;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => port = Some(next_arg(&mut args, "--port")?),
                "--pin" => pin = next_arg(&mut args, "--pin")?,
                "--payer" => {
                    let raw = next_arg(&mut args, "--payer")?;
                    payer = Some(Pubkey::from_str(&raw).context("invalid --payer pubkey")?);
                }
                "--vector-program-id" => {
                    vector_program_id = next_arg(&mut args, "--vector-program-id")?;
                }
                "--nonce-hex" => {
                    let raw = next_arg(&mut args, "--nonce-hex")?;
                    nonce = Some(parse_nonce_hex(&raw)?);
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
                "--no-auto-init" => auto_init = false,
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
            payer,
            nonce,
            vector_program_id: Pubkey::from_str(&vector_program_id)
                .context("invalid Vector program id")?,
            read_timeout,
            action_timeout,
            open_wait,
            auto_init,
        })
    }
}

fn print_usage() {
    println!(
        "Usage:\n  cargo run --manifest-path tools/vector-device-harness/Cargo.toml -- --port /dev/cu.usbmodemXXXX [options]\n\
\nOptions:\n  --pin <digits>            6-digit PIN. Default: 246810\n  --payer <base58>          Optional payer pubkey for initialize instruction output\n  --nonce-hex <64 hex>      Optional current Vector nonce for empty advance signing\n  --vector-program-id <id>  Vector Falcon program id. Default: VECTOR_PROGRAM_ID / FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU\n  --read-timeout-ms <n>     Default command wait time in ms. Default: 4000\n  --action-timeout-ms <n>   Long command wait time in ms. Default: 35000\n  --open-wait-ms <n>        Delay after opening serial port in ms. Default: 2000\n  --no-auto-init            Fail instead of calling PQC_INIT when PQC is blank\n  -h, --help                Show this help text"
    );
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn parse_nonce_hex(raw: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(raw).context("failed to decode --nonce-hex")?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("nonce must be exactly 32 bytes"))
}

fn run(config: Config) -> Result<()> {
    println!("============================================================");
    println!("Vector Device Harness");
    println!("============================================================");
    println!("Port: {}", config.port);
    println!("Vector Program: {}", config.vector_program_id);

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

    let info = get_info(&mut *port, config.read_timeout)?;
    ensure!(
        info.get("AUTH_MODE").map(String::as_str) == Some("PIN"),
        "device must already be in PIN mode"
    );
    ensure!(
        info.get("FINALIZED").map(String::as_str) == Some("1"),
        "device must already be finalized"
    );
    ensure!(
        info.get("KEY_STATE").map(String::as_str) == Some("READY"),
        "SE050 wallet is not ready"
    );

    let response = send_command(
        &mut *port,
        &format!("UNLOCK:PIN:{}", config.pin),
        config.read_timeout,
    )?;
    expect_line_prefix("UNLOCK:PIN", &response, "UNLOCKED_UNTIL:")?;

    let se050_pubkey = get_se050_pubkey(&mut *port, config.read_timeout)?;
    let pqc = get_or_init_pqc(
        &mut *port,
        &se050_pubkey,
        config.read_timeout,
        config.action_timeout,
        config.auto_init,
    )?;

    let vector_identity = falcon512_identity(&pqc.wire_pubkey);
    let (vector_pda, bump) = find_vector_pda(&config.vector_program_id, &vector_identity);

    println!();
    println!("Device binding");
    println!("SE050 pubkey:         {}", se050_pubkey);
    println!("Falcon pubkey b64:    {}", pqc.wire_pubkey_b64);
    println!("Binding verified:     true");
    println!("Vector identity hex:  {}", hex::encode(vector_identity));
    println!("Vector identity b64:  {}", STANDARD.encode(vector_identity));
    println!("Vector PDA:           {}", vector_pda);
    println!("Vector PDA bump:      {}", bump);

    if let Some(payer) = config.payer {
        let init_ix =
            create_initialize_falcon512(config.vector_program_id, payer, &pqc.wire_pubkey)?;
        println!();
        println!("Initialize instruction");
        print_instruction(&init_ix);
    } else {
        println!();
        println!("Initialize instruction");
        println!("Pass --payer <base58> to print the initialize instruction shape.");
    }

    println!();
    println!("Nonce note");
    println!(
        "Vector initialize derives the first nonce on-chain from slot hashes. \
Fetch the created Vector account after initialize, then rerun this tool with --nonce-hex <current_nonce>."
    );

    if let Some(nonce) = config.nonce {
        let digest = advance_vector_digest(
            config.vector_program_id,
            &vector_identity,
            &nonce,
            &[],
            &[],
            None,
        )?;
        let response = send_command(
            &mut *port,
            &format!("PQC_SIGN_VECTOR_DIGEST:{}", STANDARD.encode(digest)),
            config.action_timeout,
        )?;
        let signature = STANDARD
            .decode(prefixed_value(&response, "PQC_VECTOR_SIGNATURE:")?)
            .context("failed to decode PQC_VECTOR_SIGNATURE")?;
        let returned_identity = STANDARD
            .decode(prefixed_value(&response, "PQC_VECTOR_IDENTITY:")?)
            .context("failed to decode PQC_VECTOR_IDENTITY")?;
        let returned_pubkey = STANDARD
            .decode(prefixed_value(&response, "PQC_PUBKEY:")?)
            .context("failed to decode PQC_PUBKEY")?;

        ensure!(
            returned_pubkey == pqc.wire_pubkey,
            "device returned a different Falcon pubkey"
        );
        ensure!(
            returned_identity == vector_identity,
            "device returned a different Vector identity"
        );
        verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

        let advance_ix =
            create_advance_instruction(config.vector_program_id, &vector_identity, &signature)?;
        println!();
        println!("Advance instruction (empty post-init advance)");
        println!("Current nonce hex:    {}", hex::encode(nonce));
        println!("Advance digest hex:   {}", hex::encode(digest));
        println!("Advance digest b64:   {}", STANDARD.encode(digest));
        println!("Signature verified:   true");
        println!("Next nonce hex:       {}", hex::encode(digest));
        print_instruction(&advance_ix);
    }

    Ok(())
}

fn get_info(port: &mut dyn SerialPort, timeout: Duration) -> Result<BTreeMap<String, String>> {
    let response = send_command(port, "GET_INFO", timeout)?;
    parse_info(&response)
}

fn get_se050_pubkey(port: &mut dyn SerialPort, timeout: Duration) -> Result<Pubkey> {
    let response = send_command(port, "GET_PUBKEY", timeout)?;
    let pubkey = prefixed_value(&response, "PUBKEY:")?;
    Pubkey::from_str(pubkey).context("failed to parse SE050 pubkey")
}

struct PqcState {
    wire_pubkey: Vec<u8>,
    wire_pubkey_b64: String,
}

fn get_or_init_pqc(
    port: &mut dyn SerialPort,
    se050_pubkey: &Pubkey,
    read_timeout: Duration,
    action_timeout: Duration,
    auto_init: bool,
) -> Result<PqcState> {
    let status = send_command(port, "PQC_STATUS", read_timeout)?;
    let response = if status.lines().any(|line| line == "PQC_OK") {
        let pubkey = send_command(port, "PQC_GET_PUBKEY", read_timeout)?;
        let binding = send_command(port, "PQC_GET_BINDING", read_timeout)?;
        format!("{pubkey}\n{binding}")
    } else if status.lines().any(|line| line == "PQC_BLANK") {
        ensure!(
            auto_init,
            "PQC key is blank; rerun without --no-auto-init or initialize it first"
        );
        send_command(port, "PQC_INIT", action_timeout)?
    } else {
        bail!("unexpected PQC_STATUS response: {status}");
    };

    let wire_pubkey_b64 = prefixed_value(&response, "PQC_PUBKEY:")?.to_string();
    let binding_sig = STANDARD
        .decode(prefixed_value(&response, "PQC_BINDING_SIG:")?)
        .context("failed to decode PQC binding signature")?;
    let wire_pubkey = STANDARD
        .decode(&wire_pubkey_b64)
        .context("failed to decode PQC wire pubkey")?;

    ensure!(
        wire_pubkey.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon pubkey length: {}",
        wire_pubkey.len()
    );
    verify_binding_signature(se050_pubkey, &wire_pubkey, &binding_sig)?;

    Ok(PqcState {
        wire_pubkey,
        wire_pubkey_b64,
    })
}

fn verify_binding_signature(
    se050_pubkey: &Pubkey,
    wire_pubkey: &[u8],
    signature: &[u8],
) -> Result<()> {
    ensure!(
        signature.len() == ED25519_SIG_BYTES,
        "unexpected binding signature length: {}",
        signature.len()
    );
    let message = build_binding_message(se050_pubkey, wire_pubkey);
    let verifying_key = Ed25519VerifyingKey::from_bytes(se050_pubkey.as_ref().try_into().unwrap())
        .context("failed to decode SE050 verifying key")?;
    let signature = Ed25519Signature::from_bytes(signature.try_into().unwrap());
    verifying_key
        .verify(&message, &signature)
        .context("binding signature verification failed")
}

fn build_binding_message(se050_pubkey: &Pubkey, falcon_pubkey: &[u8]) -> Vec<u8> {
    let pubkey_hash = Sha256::digest(falcon_pubkey);
    let mut out = Vec::with_capacity(
        BINDING_TAG.len() + 1 + se050_pubkey.as_ref().len() + 4 + pubkey_hash.len(),
    );
    out.extend_from_slice(BINDING_TAG);
    out.push(0);
    out.extend_from_slice(se050_pubkey.as_ref());
    append_len_prefixed(&mut out, pubkey_hash.as_slice());
    out
}

fn falcon512_identity(wire_pubkey: &[u8]) -> [u8; 32] {
    Sha256::digest(wire_pubkey).into()
}

fn pda_seed_from_identity(identity: &[u8]) -> [u8; 32] {
    if identity.len() <= 32 {
        let mut out = [0u8; 32];
        out[..identity.len()].copy_from_slice(identity);
        out
    } else {
        Sha256::digest(identity).into()
    }
}

fn find_vector_pda(program_id: &Pubkey, identity: &[u8]) -> (Pubkey, u8) {
    let seed_bytes = pda_seed_from_identity(identity);
    let seed_len = identity.len().min(32);
    Pubkey::find_program_address(&[VECTOR_PDA_SEED, &seed_bytes[..seed_len]], program_id)
}

fn create_initialize_falcon512(
    program_id: Pubkey,
    payer: Pubkey,
    wire_pubkey: &[u8],
) -> Result<ProtoInstruction> {
    ensure!(
        wire_pubkey.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon pubkey length: {}",
        wire_pubkey.len()
    );
    let identity = falcon512_identity(wire_pubkey);
    let (vector_pda, _) = find_vector_pda(&program_id, &identity);

    let mut data = Vec::with_capacity(1 + wire_pubkey.len());
    data.push(INITIALIZE_DISCRIMINATOR);
    data.extend_from_slice(wire_pubkey);

    Ok(ProtoInstruction {
        program_id,
        accounts: vec![
            ProtoAccountMeta {
                pubkey: payer,
                is_signer: true,
                is_writable: true,
            },
            ProtoAccountMeta {
                pubkey: vector_pda,
                is_signer: false,
                is_writable: true,
            },
            ProtoAccountMeta {
                pubkey: Pubkey::from_str(SYSTEM_PROGRAM_ID).unwrap(),
                is_signer: false,
                is_writable: false,
            },
        ],
        data,
    })
}

fn create_advance_instruction(
    program_id: Pubkey,
    identity: &[u8],
    signature: &[u8],
) -> Result<ProtoInstruction> {
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );
    let (vector_pda, _) = find_vector_pda(&program_id, identity);
    let mut data = Vec::with_capacity(1 + signature.len());
    data.push(ADVANCE_DISCRIMINATOR);
    data.extend_from_slice(signature);

    Ok(ProtoInstruction {
        program_id,
        accounts: vec![
            ProtoAccountMeta {
                pubkey: vector_pda,
                is_signer: false,
                is_writable: true,
            },
            ProtoAccountMeta {
                pubkey: Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID).unwrap(),
                is_signer: false,
                is_writable: false,
            },
        ],
        data,
    })
}

fn advance_vector_digest(
    program_id: Pubkey,
    identity: &[u8],
    nonce: &[u8; 32],
    pre_instructions: &[ProtoInstruction],
    post_instructions: &[ProtoInstruction],
    fee_payer: Option<Pubkey>,
) -> Result<[u8; 32]> {
    let placeholder = vec![0u8; FALCON_512_SIGNATURE_LEN];
    let advance_ix = create_advance_instruction(program_id, identity, &placeholder)?;
    let target_index = pre_instructions.len();

    let mut all = Vec::with_capacity(pre_instructions.len() + 1 + post_instructions.len());
    all.extend_from_slice(pre_instructions);
    all.push(advance_ix);
    all.extend_from_slice(post_instructions);

    let promoted = promote_to_message_flags(&all, fee_payer);
    let buffer = construct_instructions_data(&promoted, target_index)?;

    let ix_offset_pos = 2 + 2 * target_index;
    let ix_offset = read_u16_le(&buffer, ix_offset_pos) as usize;
    let num_accounts = read_u16_le(&buffer, ix_offset) as usize;
    let sig_start = ix_offset + 2 + 33 * num_accounts + 32 + 2 + 1;
    let sig_end = sig_start + FALCON_512_SIGNATURE_LEN;

    let mut hasher = Sha256::new();
    hasher.update(&buffer[..sig_start]);
    hasher.update(nonce);
    hasher.update(identity);
    hasher.update(&buffer[sig_end..]);
    Ok(hasher.finalize().into())
}

fn promote_to_message_flags(
    instructions: &[ProtoInstruction],
    fee_payer: Option<Pubkey>,
) -> Vec<ProtoInstruction> {
    #[derive(Clone, Copy, Default)]
    struct Flags {
        is_signer: bool,
        is_writable: bool,
    }

    let mut flags = BTreeMap::<[u8; 32], Flags>::new();
    if let Some(fee_payer) = fee_payer {
        flags.insert(
            fee_payer.to_bytes(),
            Flags {
                is_signer: true,
                is_writable: true,
            },
        );
    }

    for ix in instructions {
        for meta in &ix.accounts {
            let entry = flags.entry(meta.pubkey.to_bytes()).or_default();
            entry.is_signer |= meta.is_signer;
            entry.is_writable |= meta.is_writable;
        }
    }

    instructions
        .iter()
        .map(|ix| ProtoInstruction {
            program_id: ix.program_id,
            accounts: ix
                .accounts
                .iter()
                .map(|meta| {
                    let promoted = flags
                        .get(&meta.pubkey.to_bytes())
                        .copied()
                        .unwrap_or_default();
                    ProtoAccountMeta {
                        pubkey: meta.pubkey,
                        is_signer: promoted.is_signer,
                        is_writable: promoted.is_writable,
                    }
                })
                .collect(),
            data: ix.data.clone(),
        })
        .collect()
}

fn construct_instructions_data(
    instructions: &[ProtoInstruction],
    current_instruction_index: usize,
) -> Result<Vec<u8>> {
    ensure!(
        instructions.len() <= u16::MAX as usize,
        "too many instructions: {}",
        instructions.len()
    );
    ensure!(
        current_instruction_index <= u16::MAX as usize,
        "current instruction index too large: {}",
        current_instruction_index
    );

    let mut data = Vec::with_capacity(instructions.len() * 128);
    append_u16_le(&mut data, instructions.len() as u16);
    for _ in 0..instructions.len() {
        append_u16_le(&mut data, 0);
    }

    for (idx, ix) in instructions.iter().enumerate() {
        ensure!(
            ix.accounts.len() <= u16::MAX as usize,
            "too many instruction accounts: {}",
            ix.accounts.len()
        );
        ensure!(
            ix.data.len() <= u16::MAX as usize,
            "instruction data too long: {}",
            ix.data.len()
        );

        let start = data.len() as u16;
        let header_offset = 2 + 2 * idx;
        data[header_offset..header_offset + 2].copy_from_slice(&start.to_le_bytes());

        append_u16_le(&mut data, ix.accounts.len() as u16);
        for meta in &ix.accounts {
            let mut flags = 0u8;
            if meta.is_signer {
                flags |= 1;
            }
            if meta.is_writable {
                flags |= 2;
            }
            data.push(flags);
            data.extend_from_slice(meta.pubkey.as_ref());
        }
        data.extend_from_slice(ix.program_id.as_ref());
        append_u16_le(&mut data, ix.data.len() as u16);
        data.extend_from_slice(&ix.data);
    }

    append_u16_le(&mut data, current_instruction_index as u16);
    Ok(data)
}

fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

fn append_u16_le(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn verify_vector_signature(wire_pubkey: &[u8], digest: &[u8], signature: &[u8]) -> Result<()> {
    ensure!(
        wire_pubkey.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon pubkey length: {}",
        wire_pubkey.len()
    );
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );

    let pubkey = Falcon512Pubkey::try_from_slice(wire_pubkey)
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

fn print_instruction(ix: &ProtoInstruction) {
    println!("Program:             {}", ix.program_id);
    for (idx, meta) in ix.accounts.iter().enumerate() {
        println!(
            "Account {idx}:           {}  signer={} writable={}",
            meta.pubkey, meta.is_signer, meta.is_writable
        );
    }
    println!("Data hex:            {}", hex::encode(&ix.data));
    println!("Data b64:            {}", STANDARD.encode(&ix.data));
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
        if saw_data && now.duration_since(last_data) > Duration::from_millis(150) {
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

fn strip_pong_lines(raw: &str) -> String {
    raw.lines()
        .filter(|line| !line.trim().is_empty() && line.trim() != "PONG")
        .collect::<Vec<_>>()
        .join("\n")
}

fn prefixed_value<'a>(response: &'a str, prefix: &str) -> Result<&'a str> {
    response
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .ok_or_else(|| anyhow!("response missing prefix {prefix}: {response:?}"))
}

fn expect_line_prefix(context: &str, response: &str, prefix: &str) -> Result<()> {
    if response.lines().any(|line| line.starts_with(prefix)) {
        Ok(())
    } else {
        bail!("{context} missing expected prefix {prefix}: {response:?}")
    }
}

fn parse_info(response: &str) -> Result<BTreeMap<String, String>> {
    let line = response
        .lines()
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

fn append_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}
