use solana_sdk::{
    message::Message,
    pubkey::Pubkey,
    signature::Signature,
    system_instruction::{self, SystemInstruction},
    transaction::Transaction,
    instruction::CompiledInstruction,
    message::MessageHeader,
};
use solana_client::rpc_client::RpcClient;
use serialport::SerialPort;
use std::str::FromStr;
use base64::Engine;
use anyhow::Result;
use bincode;

const RECIPIENT_PUBLIC_KEY: &str = "6tBou5MHL5aWpDy6cgf3wiwGGK2mR8qs68ujtpaoWrf2";
const LAMPORTS_TO_SEND: u64 = 1_000_000; // 0.001 SOL
const SERIAL_PORT: &str = "/dev/tty.usbserial-0001"; // Update if USB-C port changes this
const RPC_URL: &str = "api";

fn get_esp32_public_key(port: &mut Box<dyn SerialPort>) -> Result<Pubkey> {
    port.write_all("GET_PUBKEY\n".as_bytes())?;
    port.flush()?;
    println!("Requested public key from ESP32");

    let mut buffer = String::new();
    let mut byte = [0u8; 1];
    let mut timeout_count = 0;
    while timeout_count < 10 {
        match port.read(&mut byte) {
            Ok(1) => {
                let ch = byte[0] as char;
                if ch == '\n' { break; }
                buffer.push(ch);
            }
            Ok(0) => {
                timeout_count += 1;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(_) => {
                timeout_count += 1;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Ok(n) => unreachable!("Unexpected read size: {}", n),
        }
    }
    let pubkey_str = buffer.trim();
    if pubkey_str.is_empty() {
        return Err(anyhow::anyhow!("No public key received from ESP32"));
    }
    println!("Received ESP32 public key: {}", pubkey_str);
    Pubkey::from_str(pubkey_str).map_err(Into::into)
}

fn create_unsigned_transaction(client: &RpcClient, esp32_pubkey: Pubkey) -> Result<Transaction> {
    let recipient_pubkey = Pubkey::from_str(RECIPIENT_PUBLIC_KEY)?;
    let system_program_id = solana_sdk::system_program::id();
    
    let account_keys = vec![esp32_pubkey, recipient_pubkey, system_program_id];
    
    let instructions = vec![CompiledInstruction {
        program_id_index: 2,
        accounts: vec![0, 1],
        data: bincode::serialize(&SystemInstruction::Transfer { lamports: LAMPORTS_TO_SEND })?,
    }];
    
    let recent_blockhash = client.get_latest_blockhash()?;
    let message = Message {
        header: MessageHeader {
            num_required_signatures: 1,
            num_readonly_signed_accounts: 0,
            num_readonly_unsigned_accounts: 1,
        },
        account_keys,
        recent_blockhash,
        instructions,
    };
    
    Ok(Transaction::new_unsigned(message))
}

fn send_to_esp32_and_get_signature(port: &mut Box<dyn SerialPort>, message: &str) -> Result<String> {
    port.write_all((message.to_string() + "\n").as_bytes())?;
    port.flush()?;
    println!("Sent to ESP32: {}", message);

    let mut buffer = String::new();
    let mut byte = [0u8; 1];
    let mut timeout_count = 0;
    while timeout_count < 10 {
        match port.read(&mut byte) {
            Ok(1) => {
                let ch = byte[0] as char;
                if ch == '\n' { break; }
                buffer.push(ch);
            }
            Ok(0) => {
                timeout_count += 1;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(_) => {
                timeout_count += 1;
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Ok(n) => unreachable!("Unexpected read size: {}", n),
        }
    }
    let response = buffer.trim().to_string();
    if response.is_empty() {
        return Err(anyhow::anyhow!("No signature received from ESP32"));
    }
    println!("Received from ESP32: {}", response);
    Ok(response)
}

fn main() -> Result<()> {
    let client = RpcClient::new(RPC_URL.to_string());
    let mut port = serialport::new(SERIAL_PORT, 115_200)
        .timeout(std::time::Duration::from_secs(1))
        .open()?;

    let esp32_pubkey = get_esp32_public_key(&mut port)?;
    let mut transaction = create_unsigned_transaction(&client, esp32_pubkey)?;
    
    let message_bytes = transaction.message.serialize();
    let base64_message = base64::engine::general_purpose::STANDARD.encode(&message_bytes);
    println!("Serialized Transaction Message (Base64): {}", base64_message);

    let base64_signature = send_to_esp32_and_get_signature(&mut port, &base64_message)?;
    let signature_bytes = base64::engine::general_purpose::STANDARD.decode(&base64_signature)?;
    let signature = Signature::try_from(signature_bytes.as_slice())?;

    transaction.signatures = vec![signature];
    let txid = client.send_and_confirm_transaction(&transaction)?;
    println!("Transaction submitted with ID: {}", txid);

    Ok(())
}