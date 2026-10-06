use bs58;
use anyhow::{Result, anyhow};
use log::*;

// The minimal structures needed to parse Solana transactions
// We're not using the full Solana SDK to keep things lightweight

#[derive(Debug)]
pub struct AccountMeta {
    pub pubkey: [u8; 32],
    pub is_signer: bool, 
    pub is_writable: bool,
}

#[derive(Debug)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct MessageHeader {
    pub num_required_signatures: u8,
    pub num_readonly_signed_accounts: u8,
    pub num_readonly_unsigned_accounts: u8,
}

#[derive(Debug)]
pub struct Message {
    pub header: MessageHeader,
    pub account_keys: Vec<[u8; 32]>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledInstruction>,
}

// Basic enum to identify common Solana transaction types
#[derive(Debug)]
pub enum TransactionType {
    SystemTransfer { from: String, to: String, amount_lamports: u64 },
    TokenTransfer { from: String, to: String, mint: String, amount: u64 },
    Unknown { program_id: String },
}

pub struct TransactionInfo {
    pub fee_payer: String,
    pub tx_type: TransactionType,
    pub blockhash: String,
    pub num_signatures_required: u8,
}

// Parse a serialized message
pub fn parse_message(message_bytes: &[u8]) -> Result<Message> {
    // Very simplified parsing - in a real implementation, you would use
    // proper Solana transaction deserialization with borsh or bincode
    
    if message_bytes.len() < 3 {
        return Err(anyhow!("Message too short"));
    }
    
    // Parse header
    let header = MessageHeader {
        num_required_signatures: message_bytes[0],
        num_readonly_signed_accounts: message_bytes[1],
        num_readonly_unsigned_accounts: message_bytes[2],
    };
    
    // This is a simplified parsing logic - a real implementation would use
    // proper Solana transaction deserialization with borsh or bincode
    
    // For now, just return a dummy message structure
    // In a real implementation, you would parse the full message
    
    // Since we can't fully parse without the Solana SDK, this is a placeholder
    // that at least extracts the first account (fee payer)
    
    // This simplified implementation at least extracts the fee payer's pubkey
    // which is the first account in the accounts list
    let mut account_keys = Vec::new();
    let mut index = 3; // Skip header
    
    // This is a VERY simplified parser - in a real implementation you would use 
    // proper Solana transaction deserialization with borsh or bincode
    
    // Try to extract what looks like the fee payer pubkey (first 32 bytes after header)
    if message_bytes.len() >= index + 32 {
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&message_bytes[index..index+32]);
        account_keys.push(pubkey);
    } else {
        return Err(anyhow!("Message too short, can't extract fee payer"));
    }
    
    Ok(Message {
        header,
        account_keys,
        recent_blockhash: [0u8; 32], // Placeholder
        instructions: Vec::new(),    // Placeholder
    })
}

// Check if the fee payer matches the signer
pub fn is_fee_payer_signer(message: &Message, signer_pubkey: &[u8; 32]) -> bool {
    if message.account_keys.is_empty() {
        return false;
    }
    
    // Fee payer is always the first account
    &message.account_keys[0] == signer_pubkey
}

// Generate human-readable transaction info
pub fn introspect_transaction(message_bytes: &[u8], signer_pubkey: &[u8; 32]) -> Result<TransactionInfo> {
    let message = parse_message(message_bytes)?;
    
    // Check if fee payer matches signer
    if !is_fee_payer_signer(&message, signer_pubkey) {
        warn!("Fee payer does not match signer!");
    }
    
    let fee_payer = if !message.account_keys.is_empty() {
        bs58::encode(&message.account_keys[0]).into_string()
    } else {
        "Unknown".to_string()
    };
    
    // In a real implementation, you would decode the instruction data to determine
    // the actual transaction type and details
    
    // This is a simplified implementation that assumes a System Program transfer
    // In a real implementation, you would check program IDs and decode instruction data
    
    Ok(TransactionInfo {
        fee_payer: fee_payer.clone(),
        tx_type: TransactionType::Unknown { 
            program_id: "Unknown (can't fully decode without Solana SDK)".to_string() 
        },
        blockhash: "Unknown (simplified parsing)".to_string(),
        num_signatures_required: message.header.num_required_signatures,
    })
}

// Format transaction info for display
pub fn format_transaction_info(tx_info: &TransactionInfo) -> String {
    let mut output = String::new();
    
    output.push_str(&format!("Fee payer: {}\n", tx_info.fee_payer));
    output.push_str(&format!("Signatures required: {}\n", tx_info.num_signatures_required));
    
    match &tx_info.tx_type {
        TransactionType::SystemTransfer { from, to, amount_lamports } => {
            let sol_amount = *amount_lamports as f64 / 1_000_000_000.0;
            output.push_str(&format!("Transaction: SOL Transfer\n"));
            output.push_str(&format!("From: {}\n", from));
            output.push_str(&format!("To: {}\n", to));
            output.push_str(&format!("Amount: {} SOL ({} lamports)\n", sol_amount, amount_lamports));
        },
        TransactionType::TokenTransfer { from, to, mint, amount } => {
            output.push_str(&format!("Transaction: Token Transfer\n"));
            output.push_str(&format!("Token: {}\n", mint));
            output.push_str(&format!("From: {}\n", from));
            output.push_str(&format!("To: {}\n", to));
            output.push_str(&format!("Amount: {}\n", amount));
        },
        TransactionType::Unknown { program_id } => {
            output.push_str(&format!("Transaction: Unknown type\n"));
            output.push_str(&format!("Program ID: {}\n", program_id));
        }
    }
    
    output
}