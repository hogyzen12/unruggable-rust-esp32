// src/transaction_verify.rs
// Extensible transaction verification system for clear signing

use anyhow::{anyhow, Result};

/// Result of a verification check
#[derive(Debug, Clone, PartialEq)]
pub enum CheckResult {
    Pass,
    Fail,
    Warning,
}

/// A single verification check with its result and display message
#[derive(Debug, Clone)]
pub struct VerificationCheck {
    pub name: &'static str,
    pub result: CheckResult,
    pub message: String,
    pub details: Option<String>, // Additional details for display
}

/// Type of transfer
#[derive(Debug, Clone)]
pub enum TransferType {
    Sol {
        lamports: u64,
    },
    Token {
        mint: String,
        amount: u64,
        decimals: u8,
    },
}

/// Information about a transfer in the transaction
#[derive(Debug, Clone)]
pub struct TransferInfo {
    pub from: String,
    pub to: String,
    pub transfer_type: TransferType,
}

impl TransferInfo {
    pub fn format_short(&self) -> String {
        match &self.transfer_type {
            TransferType::Sol { lamports } => {
                let sol = *lamports as f64 / 1_000_000_000.0;
                format!("{} SOL", sol)
            }
            TransferType::Token {
                mint,
                amount,
                decimals,
            } => {
                let token_amount = *amount as f64 / 10_f64.powi(*decimals as i32);
                let symbol = get_token_symbol(mint);
                format!("{} {}", token_amount, symbol)
            }
        }
    }
}

/// Get token symbol from mint address
fn get_token_symbol(mint: &str) -> &'static str {
    match mint {
        USDC_MINT => "USDC",
        JLP_MINT => "JLP",
        _ => "???",
    }
}

/// Complete verification results for a transaction
#[derive(Debug, Clone)]
pub struct VerificationResults {
    pub checks: Vec<VerificationCheck>,
    pub overall_safe: bool,
    pub instruction_count: usize,
    pub transfers: Vec<TransferInfo>,
}

impl VerificationResults {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            overall_safe: true,
            instruction_count: 0,
            transfers: Vec::new(),
        }
    }

    pub fn add_check(&mut self, check: VerificationCheck) {
        if check.result == CheckResult::Fail {
            self.overall_safe = false;
        }
        self.checks.push(check);
    }

    pub fn set_instruction_count(&mut self, count: usize) {
        self.instruction_count = count;
    }

    pub fn add_transfer(&mut self, transfer: TransferInfo) {
        self.transfers.push(transfer);
    }

    /// Format for display on screen - ULTRA COMPACT for 128x160 screen
    /// Target: Max 9 lines total (4 checks + up to 5 for transfers)
    pub fn format_for_display(&self) -> Vec<String> {
        let mut lines = Vec::new();

        // Verification checks (4 lines maximum)
        for check in &self.checks {
            let symbol = match check.result {
                CheckResult::Pass => "✓",
                CheckResult::Fail => "✗",
                CheckResult::Warning => "!",
            };

            if let Some(details) = &check.details {
                lines.push(format!("{}{}", symbol, details));
            } else {
                lines.push(format!("{}{}", symbol, check.message));
            }
        }

        // User transfers - ultra compact (just amount + symbol)
        if !self.transfers.is_empty() {
            // Show up to 3 transfers max
            for transfer in self.transfers.iter().take(3) {
                lines.push(format!(">{}", transfer.format_short()));
            }

            // If more than 3, show count
            if self.transfers.len() > 3 {
                lines.push(format!("+{} more", self.transfers.len() - 3));
            }
        }

        lines
    }
}

// Known program IDs for verification
pub const ASSEMBLY_PROGRAM: &str = "23MzuyVH6EKGbUHq7GjBY6ydSCVoZQYDmzeKVdDBKWNQ";
pub const JULESO_ADDRESS: &str = "juLesoSmdTcRtzjCzYzRoHrnF8GhVu6KCV7uxq7nJGp";
pub const JITO_ADDRESS: &str = "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL";
pub const EXPECTED_TRANSFER_LAMPORTS: u64 = 100_000; // 0.0001 SOL

// SPL Token Program
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

// Known token mints
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const JLP_MINT: &str = "27G8MtK7VtTcCHkpASjSDdkWWYfoqT6ggEuKidVJidD4";

/// Minimal Solana transaction parser
/// Parses just enough to extract program IDs and transfer amounts
pub struct TransactionParser {
    data: Vec<u8>,
}

impl TransactionParser {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Verify the transaction against MVP checks
    pub fn verify_mvp_transaction(&self) -> Result<VerificationResults> {
        let mut results = VerificationResults::new();

        // Parse transaction to extract accounts and instructions
        let (accounts, instructions) = self.parse_transaction()?;

        // Set instruction count
        results.set_instruction_count(instructions.len());

        // Extract all transfers (SOL and tokens) for display
        let transfers = self.extract_all_transfers(&accounts, &instructions);
        let token_transfers = self.extract_token_transfers(&accounts, &instructions);

        for transfer in &transfers {
            results.add_transfer(transfer.clone());
        }
        for transfer in &token_transfers {
            results.add_transfer(transfer.clone());
        }

        // Check 1: Slot replay protection (Assembly program)
        let assembly_check = self.check_assembly_program(&instructions);
        results.add_check(assembly_check);

        // Check 2: SOL transfer to juLeso
        let juleso_check =
            self.check_sol_transfer(&accounts, &instructions, JULESO_ADDRESS, "juLe");
        results.add_check(juleso_check);

        // Check 3: SOL transfer to Jito tip address
        let jito_check = self.check_sol_transfer(&accounts, &instructions, JITO_ADDRESS, "Jito");
        results.add_check(jito_check);

        // Check 4: Instruction accountability - verify all instructions are accounted for
        let accountability_check = self.check_instruction_accountability(&instructions);
        results.add_check(accountability_check);

        Ok(results)
    }

    /// Verify that all instructions in the transaction are accounted for
    fn check_instruction_accountability(&self, instructions: &[Instruction]) -> VerificationCheck {
        let system_program = "11111111111111111111111111111111";

        // Count known instruction types
        let mut known_count = 0;

        // Count Assembly/Slot Replay instructions
        known_count += instructions
            .iter()
            .filter(|ix| ix.program_id == ASSEMBLY_PROGRAM)
            .count();

        // Count system transfer instructions (juLe + Jito + user transfers)
        let transfer_count = instructions
            .iter()
            .filter(|ix| ix.program_id == system_program && ix.data.len() >= 12 && ix.data[0] == 2)
            .count();
        known_count += transfer_count;

        // All instructions should be accounted for
        if known_count == instructions.len() {
            VerificationCheck {
                name: "Instructions",
                result: CheckResult::Pass,
                message: "All accounted".to_string(),
                details: Some("All ix OK".to_string()),
            }
        } else {
            VerificationCheck {
                name: "Instructions",
                result: CheckResult::Fail,
                message: format!(
                    "Unknown: {}/{}",
                    instructions.len() - known_count,
                    instructions.len()
                ),
                details: Some(format!("Unknown ix: {}", instructions.len() - known_count)),
            }
        }
    }

    /// Extract all SOL transfer instructions from the transaction
    /// Excludes transfers already shown in verification checks (juLe and Jito)
    fn extract_all_transfers(
        &self,
        accounts: &[String],
        instructions: &[Instruction],
    ) -> Vec<TransferInfo> {
        let mut transfers = Vec::new();
        let system_program = "11111111111111111111111111111111";

        for ix in instructions {
            if ix.program_id == system_program && ix.data.len() >= 12 {
                // Check if it's a transfer instruction (type 2)
                if ix.data[0] == 2 && ix.data[1..4] == [0, 0, 0] {
                    // Parse lamports amount (8 bytes, little-endian)
                    let lamports = u64::from_le_bytes([
                        ix.data[4],
                        ix.data[5],
                        ix.data[6],
                        ix.data[7],
                        ix.data[8],
                        ix.data[9],
                        ix.data[10],
                        ix.data[11],
                    ]);

                    // Get sender and recipient (account indices 0 and 1)
                    if ix.accounts.len() >= 2 {
                        let from_idx = ix.accounts[0] as usize;
                        let to_idx = ix.accounts[1] as usize;

                        if from_idx < accounts.len() && to_idx < accounts.len() {
                            let to_address = &accounts[to_idx];

                            // Skip transfers that are already in verification checks
                            if to_address != JULESO_ADDRESS && to_address != JITO_ADDRESS {
                                transfers.push(TransferInfo {
                                    from: accounts[from_idx].clone(),
                                    to: to_address.clone(),
                                    transfer_type: TransferType::Sol { lamports },
                                });
                            }
                        }
                    }
                }
            }
        }

        transfers
    }

    /// Extract SPL token transfer instructions
    fn extract_token_transfers(
        &self,
        accounts: &[String],
        instructions: &[Instruction],
    ) -> Vec<TransferInfo> {
        let mut transfers = Vec::new();

        for ix in instructions {
            // Check if this is a Token Program instruction
            if ix.program_id == TOKEN_PROGRAM {
                // Token transfer instruction format:
                // Instruction discriminator: 3 (1 byte)
                // Amount: u64 (8 bytes, little-endian)
                // Decimals field is NOT in instruction data - we need to infer it

                if ix.data.len() >= 9 && ix.data[0] == 3 {
                    // Parse amount
                    let amount = u64::from_le_bytes([
                        ix.data[1], ix.data[2], ix.data[3], ix.data[4], ix.data[5], ix.data[6],
                        ix.data[7], ix.data[8],
                    ]);

                    // Token transfer accounts:
                    // 0: Source token account
                    // 1: Destination token account
                    // 2: Authority
                    if ix.accounts.len() >= 2 {
                        let from_idx = ix.accounts[0] as usize;
                        let to_idx = ix.accounts[1] as usize;

                        if from_idx < accounts.len() && to_idx < accounts.len() {
                            // For now, we'll try to identify the token by looking at other instructions
                            // In a full implementation, we'd parse the token account to get the mint
                            // For MVP, we'll check if this transaction involves known mints
                            let mint = self.identify_token_mint(accounts, instructions);
                            let decimals = get_token_decimals(&mint);

                            transfers.push(TransferInfo {
                                from: accounts[from_idx].clone(),
                                to: accounts[to_idx].clone(),
                                transfer_type: TransferType::Token {
                                    mint,
                                    amount,
                                    decimals,
                                },
                            });
                        }
                    }
                }
            }
        }

        transfers
    }

    /// Try to identify token mint from transaction accounts
    fn identify_token_mint(&self, accounts: &[String], _instructions: &[Instruction]) -> String {
        // Check if known token mints are in the account list
        for account in accounts {
            if account == USDC_MINT || account == JLP_MINT {
                return account.clone();
            }
        }
        "Unknown".to_string()
    }

    /// Parse Solana VersionedTransaction to extract accounts and instructions
    fn parse_transaction(&self) -> Result<(Vec<String>, Vec<Instruction>)> {
        if self.data.first() == Some(&0x81) {
            return self.parse_v1_message();
        }

        // Solana VersionedTransaction format:
        // 1 byte: version prefix (0x80 for V0)
        // Then MessageHeader + accounts + recent_blockhash + instructions

        if self.data.is_empty() {
            return Err(anyhow!("Empty transaction"));
        }

        let mut offset = 0;

        // Skip the v0 version byte. Reject future formats rather than
        // interpreting their bytes as the legacy layout on the signing screen.
        if self.data[0] == 0x80 {
            offset += 1;
        } else if self.data[0] & 0x80 != 0 {
            return Err(anyhow!("Unsupported transaction message version"));
        }

        // Read message header (3 bytes)
        if offset + 3 > self.data.len() {
            return Err(anyhow!("Transaction too short"));
        }
        offset += 3;

        // Read number of accounts (compact-u16)
        let (num_accounts, bytes_read) = read_compact_u16(&self.data[offset..])?;
        offset += bytes_read;

        // Read account addresses (each is 32 bytes)
        let mut accounts = Vec::new();
        for _ in 0..num_accounts {
            if offset + 32 > self.data.len() {
                return Err(anyhow!("Invalid account data"));
            }
            let account_bytes = &self.data[offset..offset + 32];
            accounts.push(bs58::encode(account_bytes).into_string());
            offset += 32;
        }

        // Skip recent blockhash (32 bytes)
        if offset + 32 > self.data.len() {
            return Err(anyhow!("Missing blockhash"));
        }
        offset += 32;

        // Read number of instructions
        let (num_instructions, bytes_read) = read_compact_u16(&self.data[offset..])?;
        offset += bytes_read;

        // Parse instructions
        let mut instructions = Vec::new();
        for _ in 0..num_instructions {
            let (instruction, bytes_read) = Instruction::parse(&self.data[offset..], &accounts)?;
            instructions.push(instruction);
            offset += bytes_read;
        }

        Ok((accounts, instructions))
    }

    /// Parse the fixed-width Solana v1 message layout (SIMD-0385).
    fn parse_v1_message(&self) -> Result<(Vec<String>, Vec<Instruction>)> {
        const V1_PREFIX: u8 = 0x81;
        const MAX_V1_ITEMS: usize = 64;
        const KNOWN_CONFIG_BITS: u32 = 0b1_1111;
        const PRIORITY_FEE_BITS: u32 = 0b11;

        if self.data.first() != Some(&V1_PREFIX) {
            return Err(anyhow!("Invalid v1 message prefix"));
        }

        let mut offset = 1usize;
        take_bytes(&self.data, &mut offset, 3, "v1 message header")?;

        let config_mask_bytes = take_bytes(&self.data, &mut offset, 4, "v1 config mask")?;
        let config_mask = u32::from_le_bytes(
            config_mask_bytes
                .try_into()
                .map_err(|_| anyhow!("Invalid v1 config mask"))?,
        );
        if config_mask & !KNOWN_CONFIG_BITS != 0
            || matches!(config_mask & PRIORITY_FEE_BITS, 0b01 | 0b10)
        {
            return Err(anyhow!("Invalid v1 transaction config mask"));
        }

        take_bytes(&self.data, &mut offset, 32, "v1 lifetime specifier")?;
        let num_instructions = take_byte(&self.data, &mut offset, "v1 instruction count")? as usize;
        let num_accounts = take_byte(&self.data, &mut offset, "v1 address count")? as usize;
        if num_instructions > MAX_V1_ITEMS || num_accounts > MAX_V1_ITEMS {
            return Err(anyhow!("v1 message exceeds address or instruction limit"));
        }

        let mut accounts = Vec::with_capacity(num_accounts);
        for _ in 0..num_accounts {
            let account_bytes = take_bytes(&self.data, &mut offset, 32, "v1 address")?;
            accounts.push(bs58::encode(account_bytes).into_string());
        }

        let mut config_size = 0usize;
        if config_mask & PRIORITY_FEE_BITS == PRIORITY_FEE_BITS {
            config_size += 8;
        }
        if config_mask & 0b100 != 0 {
            config_size += 4;
        }
        if config_mask & 0b1000 != 0 {
            config_size += 4;
        }
        if config_mask & 0b1_0000 != 0 {
            config_size += 4;
        }
        take_bytes(&self.data, &mut offset, config_size, "v1 config values")?;

        let mut headers = Vec::with_capacity(num_instructions);
        for _ in 0..num_instructions {
            let program_index = take_byte(&self.data, &mut offset, "v1 program index")? as usize;
            let account_count = take_byte(&self.data, &mut offset, "v1 account count")? as usize;
            let data_len_bytes = take_bytes(&self.data, &mut offset, 2, "v1 data length")?;
            let data_len = u16::from_le_bytes(
                data_len_bytes
                    .try_into()
                    .map_err(|_| anyhow!("Invalid v1 data length"))?,
            ) as usize;
            headers.push((program_index, account_count, data_len));
        }

        let mut instructions = Vec::with_capacity(num_instructions);
        for (program_index, account_count, data_len) in headers {
            let program_id = accounts
                .get(program_index)
                .ok_or_else(|| anyhow!("Invalid v1 program index"))?
                .clone();
            let ix_accounts = take_bytes(
                &self.data,
                &mut offset,
                account_count,
                "v1 instruction accounts",
            )?
            .to_vec();
            if ix_accounts
                .iter()
                .any(|account_index| *account_index as usize >= accounts.len())
            {
                return Err(anyhow!("Invalid v1 instruction account index"));
            }
            let data =
                take_bytes(&self.data, &mut offset, data_len, "v1 instruction data")?.to_vec();
            instructions.push(Instruction {
                program_id,
                accounts: ix_accounts,
                data,
            });
        }

        if offset != self.data.len() {
            return Err(anyhow!("Unexpected trailing bytes in v1 message"));
        }

        Ok((accounts, instructions))
    }

    /// Check for slot replay protection (Assembly program)
    fn check_assembly_program(&self, instructions: &[Instruction]) -> VerificationCheck {
        let assembly_present = instructions
            .iter()
            .any(|ix| ix.program_id == ASSEMBLY_PROGRAM);

        if assembly_present {
            VerificationCheck {
                name: "Slot",
                result: CheckResult::Pass,
                message: "Slot".to_string(),
                details: Some("Slot".to_string()),
            }
        } else {
            VerificationCheck {
                name: "Slot",
                result: CheckResult::Fail,
                message: "Slot missing".to_string(),
                details: Some("Slot✗".to_string()),
            }
        }
    }

    /// Check for SOL transfer to specific address with enhanced display
    fn check_sol_transfer(
        &self,
        accounts: &[String],
        instructions: &[Instruction],
        expected_recipient: &str,
        short_name: &'static str,
    ) -> VerificationCheck {
        // System program transfer instruction has:
        // - Program ID: 11111111111111111111111111111111
        // - Instruction data: [2, 0, 0, 0] followed by u64 lamports (little-endian)

        let system_program = "11111111111111111111111111111111";

        for ix in instructions {
            if ix.program_id == system_program && ix.data.len() >= 12 {
                // Check if it's a transfer instruction (type 2)
                if ix.data[0] == 2 && ix.data[1..4] == [0, 0, 0] {
                    // Check if recipient matches (account index 1 in transfer instruction)
                    if let Some(recipient) = ix
                        .accounts
                        .get(1)
                        .and_then(|index| accounts.get(*index as usize))
                    {
                        if recipient == expected_recipient {
                            // Compact display format
                            let display_text = short_name.to_string();

                            return VerificationCheck {
                                name: short_name,
                                result: CheckResult::Pass,
                                message: display_text.clone(),
                                details: Some(display_text),
                            };
                        }
                    }
                }
            }
        }

        VerificationCheck {
            name: short_name,
            result: CheckResult::Fail,
            message: format!("{} missing", short_name),
            details: Some(format!("{}✗", short_name)),
        }
    }
}

fn take_byte(data: &[u8], offset: &mut usize, label: &str) -> Result<u8> {
    let byte = *data
        .get(*offset)
        .ok_or_else(|| anyhow!("Missing {label}"))?;
    *offset += 1;
    Ok(byte)
}

fn take_bytes<'a>(data: &'a [u8], offset: &mut usize, len: usize, label: &str) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| anyhow!("Invalid {label} length"))?;
    let bytes = data
        .get(*offset..end)
        .ok_or_else(|| anyhow!("Missing or truncated {label}"))?;
    *offset = end;
    Ok(bytes)
}

/// Minimal instruction representation
#[derive(Debug, Clone)]
struct Instruction {
    program_id: String,
    accounts: Vec<u8>, // Account indices
    data: Vec<u8>,
}

impl Instruction {
    fn parse(data: &[u8], accounts: &[String]) -> Result<(Self, usize)> {
        let mut offset = 0;

        // Read program ID index
        if offset >= data.len() {
            return Err(anyhow!("Invalid instruction"));
        }
        let program_idx = data[offset] as usize;
        offset += 1;

        if program_idx >= accounts.len() {
            return Err(anyhow!("Invalid program index"));
        }
        let program_id = accounts[program_idx].clone();

        // Read number of account indices
        let (num_accounts, bytes_read) = read_compact_u16(&data[offset..])?;
        offset += bytes_read;

        // Read account indices
        let mut ix_accounts = Vec::new();
        for _ in 0..num_accounts {
            if offset >= data.len() {
                return Err(anyhow!("Invalid account indices"));
            }
            ix_accounts.push(data[offset]);
            offset += 1;
        }

        // Read instruction data length
        let (data_len, bytes_read) = read_compact_u16(&data[offset..])?;
        offset += bytes_read;

        // Read instruction data
        if offset + data_len > data.len() {
            return Err(anyhow!("Invalid instruction data"));
        }
        let ix_data = data[offset..offset + data_len].to_vec();
        offset += data_len;

        Ok((
            Instruction {
                program_id,
                accounts: ix_accounts,
                data: ix_data,
            },
            offset,
        ))
    }
}

/// Read compact-u16 encoding used in Solana transactions
fn read_compact_u16(data: &[u8]) -> Result<(usize, usize)> {
    if data.is_empty() {
        return Err(anyhow!("Empty compact-u16"));
    }

    if data[0] < 0x80 {
        Ok((data[0] as usize, 1))
    } else if data.len() >= 2 {
        let val = ((data[0] & 0x7f) as usize) | ((data[1] as usize) << 7);
        Ok((val, 2))
    } else {
        Err(anyhow!("Invalid compact-u16"))
    }
}

/// Get token decimals for known tokens
fn get_token_decimals(mint: &str) -> u8 {
    match mint {
        USDC_MINT => 6, // USDC has 6 decimals
        JLP_MINT => 6,  // JLP has 6 decimals
        _ => 9,         // Default to 9 for unknown tokens
    }
}

#[cfg(test)]
mod robustness_tests {
    use super::*;

    fn transfer_message(recipient_index: u8) -> Vec<u8> {
        let mut message = vec![1, 0, 0, 1]; // header and one static account
        message.extend_from_slice(&[0; 32]); // System Program
        message.extend_from_slice(&[0; 32]); // blockhash
        message.extend_from_slice(&[1, 0, 2, 0, recipient_index, 12]);
        message.extend_from_slice(&2u32.to_le_bytes());
        message.extend_from_slice(&1u64.to_le_bytes());
        message
    }

    #[test]
    fn out_of_range_recipient_never_panics_during_preview() {
        // Regression for the formerly unchecked check_sol_transfer index. A
        // partial preview remains advisory; this test does not change that policy.
        for recipient in 0..=u8::MAX {
            let _ = TransactionParser::new(transfer_message(recipient)).verify_mvp_transaction();
        }
    }

    #[test]
    fn every_truncated_prefix_of_a_transfer_is_rejected_without_panicking() {
        let message = transfer_message(0);
        assert!(TransactionParser::new(message.clone())
            .verify_mvp_transaction()
            .is_ok());
        for length in 0..message.len() {
            assert!(
                TransactionParser::new(message[..length].to_vec())
                    .verify_mvp_transaction()
                    .is_err(),
                "prefix {length}"
            );
        }
    }
}
