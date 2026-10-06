use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey};
use reqwest::blocking::Client as HttpClient;
use serde::Deserialize;
use serialport::{ClearBuffer, SerialPort};
use sha2::{Digest, Sha256};
use solana_address_lookup_table_interface::state::AddressLookupTable;
use solana_commitment_config::CommitmentConfig;
use solana_falcon512::{
    Falcon512Pubkey, Falcon512Signature, FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN,
};
use solana_instruction::{AccountMeta, BorrowedAccountMeta, BorrowedInstruction, Instruction};
use solana_instructions_sysvar::construct_instructions_data;
use solana_keypair::{read_keypair_file, Keypair};
use solana_message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_rpc_client::api::config::RpcSendTransactionConfig;
use solana_rpc_client::rpc_client::{RpcClient, SerializableTransaction};
use solana_signature::Signature;
use solana_signer::Signer;
use solana_stake_interface::{
    instruction::{delegate_stake as stake_delegate_stake, initialize as stake_initialize},
    program::id as stake_program_id,
    stake_flags::StakeFlags,
    state::{
        Authorized as StakeAuthorized, Delegation, Lockup as StakeLockup, Meta, Stake, StakeStateV2,
    },
};
use solana_system_interface::instruction::create_account as system_create_account;
use solana_transaction::{versioned::VersionedTransaction, Transaction};
use spl_associated_token_account_interface::{
    address::get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};
use spl_token_interface::{
    id as spl_token_program_id,
    instruction::{
        close_account as spl_close_account, initialize_mint2 as spl_initialize_mint2,
        mint_to as spl_mint_to, transfer as spl_transfer,
        transfer_checked as spl_transfer_checked,
    },
    state::{Account as SplTokenAccount, Mint as SplTokenMint},
};
use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    str::FromStr,
    thread::sleep,
    time::{Duration, Instant},
};

const BAUD: u32 = 115_200;
const VECTOR_PDA_SEED: &[u8] = b"vector";
const DEFAULT_VECTOR_FALCON_PROGRAM_ID: &str = "FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU";
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
const INSTRUCTIONS_SYSVAR_ID: &str = "Sysvar1nstructions1111111111111111111111111";
const COMPUTE_BUDGET_PROGRAM_ID: &str = "ComputeBudget111111111111111111111111111111";
const INITIALIZE_DISCRIMINATOR: u8 = 0;
const ADVANCE_DISCRIMINATOR: u8 = 1;
const WITHDRAW_DISCRIMINATOR: u8 = 3;
const PASSTHROUGH_DISCRIMINATOR: u8 = 4;
const BINDING_TAG: &[u8] = b"unruggable-bind-v2";
const ED25519_SIG_BYTES: usize = 64;
const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_COMPUTE_UNITS: u32 = 300_000;
const DEFAULT_WITHDRAW_LAMPORTS: u64 = 1_000_000;
const DEFAULT_SPL_TRANSFER_AMOUNT: u64 = 10_000;
const DEFAULT_SPL_DECIMALS: u8 = 6;
const DEFAULT_STAKE_LAMPORT_BUFFER: u64 = 1_000_000;
const DEFAULT_RAYDIUM_API_URL: &str = "https://api-v3.raydium.io";
const DEFAULT_JUPITER_API_URL: &str = "https://api.jup.ag";
const DEFAULT_SWAP_INPUT_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const DEFAULT_SWAP_OUTPUT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const WRAPPED_SOL_MINT: &str = "So11111111111111111111111111111111111111112";
const DEFAULT_SWAP_AMOUNT_IN: u64 = 10_000;
const DEFAULT_SWAP_SLIPPAGE_BPS: u16 = 50;
const RAYDIUM_CPMM_PROGRAM_ID: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
const SYSTEM_TRANSFER_DISCRIMINATOR: u32 = 2;
const JUPITER_API_KEY_ENV: &str = "UNRUGGABLE_JUPITER_API_KEY";
const VECTOR_JUPITER_API_KEY_ENV: &str = "VECTOR_JUPITER_API_KEY";
const DEFAULT_PARTNER_SECRETS_FILE: &str = "partner_secrets.local";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Status,
    ProbeDevicePayer,
    Initialize,
    AdvanceEmpty,
    FullEmpty,
    WithdrawLamports,
    FullWithdraw,
    FullSplTransfer,
    FullStakeDelegate,
    FullRaydiumCpmmSwap,
    FullJupiterRouterSwap,
}

impl Mode {
    fn from_str(raw: &str) -> Result<Self> {
        match raw {
            "status" => Ok(Self::Status),
            "probe-device-payer" => Ok(Self::ProbeDevicePayer),
            "initialize" => Ok(Self::Initialize),
            "advance-empty" => Ok(Self::AdvanceEmpty),
            "full-empty" => Ok(Self::FullEmpty),
            "withdraw-lamports" => Ok(Self::WithdrawLamports),
            "full-withdraw" => Ok(Self::FullWithdraw),
            "full-spl-transfer" => Ok(Self::FullSplTransfer),
            "full-stake-delegate" => Ok(Self::FullStakeDelegate),
            "full-raydium-cpmm-swap" => Ok(Self::FullRaydiumCpmmSwap),
            "full-jupiter-router-swap" => Ok(Self::FullJupiterRouterSwap),
            _ => bail!("unsupported --mode value: {raw}"),
        }
    }
}

#[derive(Debug)]
enum FeePayer {
    Device { pubkey: Pubkey },
    Local { keypair: Keypair, path: PathBuf },
}

impl FeePayer {
    fn pubkey(&self) -> Pubkey {
        match self {
            Self::Device { pubkey } => *pubkey,
            Self::Local { keypair, .. } => keypair.pubkey(),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Device { pubkey } => format!("device:{pubkey}"),
            Self::Local { keypair, path } => {
                format!("file:{} ({})", path.display(), keypair.pubkey())
            }
        }
    }
}

#[derive(Clone)]
struct Config {
    port: String,
    pin: String,
    rpc_url: Option<String>,
    mode: Mode,
    vector_program_id: Pubkey,
    read_timeout: Duration,
    action_timeout: Duration,
    open_wait: Duration,
    auto_init: bool,
    commitment: CommitmentConfig,
    device_payer: bool,
    fee_payer_file: Option<PathBuf>,
    compute_units: Option<u32>,
    withdraw_lamports: Option<u64>,
    withdraw_receiver: Option<Pubkey>,
    spl_mint: Option<Pubkey>,
    spl_amount: Option<u64>,
    spl_decimals: u8,
    spl_mint_amount: Option<u64>,
    spl_receiver_owner: Option<Pubkey>,
    stake_lamports: Option<u64>,
    stake_vote_account: Option<Pubkey>,
    raydium_api_url: String,
    raydium_cpmm_pool_id: Option<Pubkey>,
    jupiter_api_url: String,
    jupiter_key_file: Option<PathBuf>,
    jupiter_dexes: Option<String>,
    jupiter_exclude_dexes: Option<String>,
    jupiter_mode: Option<String>,
    jupiter_direct_only: bool,
    swap_slippage_bps: u16,
    swap_max_accounts: Option<u32>,
    swap_input_mint: Option<Pubkey>,
    swap_output_mint: Option<Pubkey>,
    swap_amount_in: Option<u64>,
    swap_min_out: Option<u64>,
    swap_unwrap_sol: bool,
    swap_receiver_owner: Option<Pubkey>,
}

#[derive(Clone)]
struct PqcState {
    wire_pubkey: Vec<u8>,
    wire_pubkey_b64: String,
}

#[derive(Clone, Copy, Debug)]
struct VectorAccountState {
    nonce: [u8; 32],
    bump: u8,
    lamports: u64,
    data_len: usize,
}

#[derive(Debug, Clone)]
struct ResolvedRaydiumCpmmPool {
    program_id: Pubkey,
    pool_id: Pubkey,
    lookup_table_account: Option<Pubkey>,
    authority: Pubkey,
    amm_config: Pubkey,
    mint_a: Pubkey,
    mint_a_program: Pubkey,
    mint_a_decimals: u8,
    mint_a_symbol: Option<String>,
    mint_b: Pubkey,
    mint_b_program: Pubkey,
    mint_b_decimals: u8,
    mint_b_symbol: Option<String>,
    vault_a: Pubkey,
    vault_b: Pubkey,
    observation_state: Pubkey,
    tvl: Option<f64>,
    fee_rate: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RaydiumPoolSearchEnvelope {
    data: RaydiumPoolSearchPage,
}

#[derive(Debug, Deserialize)]
struct RaydiumPoolSearchPage {
    data: Vec<RaydiumPoolSearchItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct RaydiumPoolSearchItem {
    id: String,
    #[serde(rename = "programId")]
    program_id: String,
    #[serde(rename = "mintA")]
    mint_a: RaydiumMintRef,
    #[serde(rename = "mintB")]
    mint_b: RaydiumMintRef,
    tvl: Option<f64>,
    #[serde(rename = "feeRate")]
    fee_rate: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RaydiumPoolKeysEnvelope {
    data: Vec<RaydiumPoolKeysItem>,
}

#[derive(Debug, Deserialize)]
struct RaydiumPoolKeysItem {
    id: String,
    #[serde(rename = "programId")]
    program_id: String,
    #[serde(rename = "lookupTableAccount")]
    lookup_table_account: Option<String>,
    authority: String,
    #[serde(rename = "mintA")]
    mint_a: RaydiumMintRef,
    #[serde(rename = "mintB")]
    mint_b: RaydiumMintRef,
    config: RaydiumPoolConfigRef,
    vault: RaydiumVaultPair,
    #[serde(rename = "observationId")]
    observation_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RaydiumMintRef {
    address: String,
    #[serde(rename = "programId")]
    program_id: String,
    decimals: u8,
    symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RaydiumPoolConfigRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct RaydiumVaultPair {
    #[serde(rename = "A")]
    a: String,
    #[serde(rename = "B")]
    b: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JupiterBuildRoutePlan {
    #[serde(rename = "swapInfo")]
    swap_info: JupiterBuildSwapInfo,
    percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct JupiterBuildSwapInfo {
    #[serde(rename = "ammKey")]
    amm_key: String,
    label: String,
    #[serde(rename = "inputMint")]
    input_mint: String,
    #[serde(rename = "outputMint")]
    output_mint: String,
    #[serde(rename = "inAmount")]
    in_amount: String,
    #[serde(rename = "outAmount")]
    out_amount: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JupiterApiInstruction {
    #[serde(rename = "programId")]
    program_id: String,
    accounts: Vec<JupiterApiAccountMeta>,
    data: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JupiterApiAccountMeta {
    pubkey: String,
    #[serde(rename = "isWritable")]
    is_writable: bool,
    #[serde(rename = "isSigner")]
    is_signer: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct JupiterBuildResponse {
    #[serde(rename = "inputMint")]
    input_mint: String,
    #[serde(rename = "outputMint")]
    output_mint: String,
    #[serde(rename = "inAmount")]
    in_amount: String,
    #[serde(rename = "outAmount")]
    out_amount: String,
    #[serde(rename = "otherAmountThreshold")]
    other_amount_threshold: String,
    #[serde(rename = "swapMode")]
    swap_mode: String,
    #[serde(rename = "slippageBps")]
    slippage_bps: u16,
    #[serde(rename = "routePlan")]
    route_plan: Vec<JupiterBuildRoutePlan>,
    #[serde(rename = "computeBudgetInstructions")]
    compute_budget_instructions: Vec<JupiterApiInstruction>,
    #[serde(rename = "setupInstructions")]
    setup_instructions: Vec<JupiterApiInstruction>,
    #[serde(rename = "swapInstruction")]
    swap_instruction: JupiterApiInstruction,
    #[serde(rename = "cleanupInstruction")]
    cleanup_instruction: Option<JupiterApiInstruction>,
    #[serde(rename = "otherInstructions")]
    other_instructions: Vec<JupiterApiInstruction>,
    #[serde(rename = "tipInstruction")]
    tip_instruction: Option<JupiterApiInstruction>,
    #[serde(rename = "addressesByLookupTableAddress")]
    addresses_by_lookup_table_address: Option<BTreeMap<String, Vec<String>>>,
}

fn main() -> Result<()> {
    let config = Config::from_args()?;
    run(config)
}

impl Config {
    fn from_args() -> Result<Self> {
        let mut port: Option<String> = None;
        let mut pin = env::var("TEST_PIN").unwrap_or_else(|_| "246810".to_string());
        let mut rpc_url = env::var("SOLANA_RPC_URL")
            .ok()
            .or_else(|| env::var("RPC_URL").ok());
        let mut vector_program_id = env::var("VECTOR_PROGRAM_ID")
            .unwrap_or_else(|_| DEFAULT_VECTOR_FALCON_PROGRAM_ID.to_string());
        let mut mode = Mode::FullEmpty;
        let mut read_timeout = Duration::from_millis(4_000);
        let mut action_timeout = Duration::from_millis(35_000);
        let mut open_wait = Duration::from_millis(2_000);
        let mut auto_init = true;
        let mut commitment = CommitmentConfig::confirmed();
        let mut device_payer = false;
        let mut fee_payer_file = env::var("SOLANA_KEYPAIR")
            .ok()
            .map(PathBuf::from)
            .or_else(default_keypair_path);
        let mut compute_units = Some(DEFAULT_COMPUTE_UNITS);
        let mut withdraw_lamports = env::var("VECTOR_WITHDRAW_LAMPORTS")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_WITHDRAW_LAMPORTS")?;
        let mut withdraw_receiver: Option<Pubkey> = None;
        let mut spl_mint = env::var("VECTOR_SPL_MINT")
            .ok()
            .map(|raw| Pubkey::from_str(&raw).context("failed to parse VECTOR_SPL_MINT"))
            .transpose()?;
        let mut spl_amount = env::var("VECTOR_SPL_AMOUNT")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SPL_AMOUNT")?;
        let mut spl_decimals = env::var("VECTOR_SPL_DECIMALS")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SPL_DECIMALS")?
            .unwrap_or(DEFAULT_SPL_DECIMALS);
        let mut spl_mint_amount = env::var("VECTOR_SPL_MINT_AMOUNT")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SPL_MINT_AMOUNT")?;
        let mut spl_receiver_owner: Option<Pubkey> = None;
        let mut stake_lamports = env::var("VECTOR_STAKE_LAMPORTS")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_STAKE_LAMPORTS")?;
        let mut stake_vote_account = env::var("VECTOR_STAKE_VOTE_ACCOUNT")
            .ok()
            .map(|raw| Pubkey::from_str(&raw).context("failed to parse VECTOR_STAKE_VOTE_ACCOUNT"))
            .transpose()?;
        let mut raydium_api_url =
            env::var("RAYDIUM_API_URL").unwrap_or_else(|_| DEFAULT_RAYDIUM_API_URL.to_string());
        let mut raydium_cpmm_pool_id = env::var("RAYDIUM_CPMM_POOL_ID")
            .ok()
            .map(|raw| Pubkey::from_str(&raw).context("failed to parse RAYDIUM_CPMM_POOL_ID"))
            .transpose()?;
        let mut jupiter_api_url =
            env::var("JUPITER_API_URL").unwrap_or_else(|_| DEFAULT_JUPITER_API_URL.to_string());
        let mut jupiter_key_file = env::var("JUPITER_KEY_FILE").ok().map(PathBuf::from);
        let mut jupiter_dexes = env::var("JUPITER_DEXES").ok();
        let mut jupiter_exclude_dexes = env::var("JUPITER_EXCLUDE_DEXES").ok();
        let mut jupiter_mode = env::var("JUPITER_MODE").ok();
        let mut jupiter_direct_only = env::var("JUPITER_DIRECT_ONLY")
            .ok()
            .map(|raw| parse_bool_flag(&raw))
            .transpose()
            .context("failed to parse JUPITER_DIRECT_ONLY")?
            .unwrap_or(false);
        let mut swap_slippage_bps = env::var("VECTOR_SWAP_SLIPPAGE_BPS")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SWAP_SLIPPAGE_BPS")?
            .unwrap_or(DEFAULT_SWAP_SLIPPAGE_BPS);
        let mut swap_max_accounts = env::var("VECTOR_SWAP_MAX_ACCOUNTS")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SWAP_MAX_ACCOUNTS")?;
        let mut swap_input_mint = env::var("VECTOR_SWAP_INPUT_MINT")
            .ok()
            .map(|raw| Pubkey::from_str(&raw).context("failed to parse VECTOR_SWAP_INPUT_MINT"))
            .transpose()?;
        let mut swap_output_mint = env::var("VECTOR_SWAP_OUTPUT_MINT")
            .ok()
            .map(|raw| Pubkey::from_str(&raw).context("failed to parse VECTOR_SWAP_OUTPUT_MINT"))
            .transpose()?;
        let mut swap_amount_in = env::var("VECTOR_SWAP_AMOUNT_IN")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SWAP_AMOUNT_IN")?;
        let mut swap_min_out = env::var("VECTOR_SWAP_MIN_OUT")
            .ok()
            .map(|raw| raw.parse())
            .transpose()
            .context("failed to parse VECTOR_SWAP_MIN_OUT")?;
        let mut swap_unwrap_sol = env::var("VECTOR_SWAP_UNWRAP_SOL")
            .ok()
            .map(|raw| parse_bool_flag(&raw))
            .transpose()
            .context("failed to parse VECTOR_SWAP_UNWRAP_SOL")?
            .unwrap_or(false);
        let mut swap_receiver_owner = env::var("VECTOR_SWAP_RECEIVER_OWNER")
            .ok()
            .map(|raw| {
                Pubkey::from_str(&raw).context("failed to parse VECTOR_SWAP_RECEIVER_OWNER")
            })
            .transpose()?;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => port = Some(next_arg(&mut args, "--port")?),
                "--pin" => pin = next_arg(&mut args, "--pin")?,
                "--rpc-url" => rpc_url = Some(next_arg(&mut args, "--rpc-url")?),
                "--vector-program-id" => {
                    vector_program_id = next_arg(&mut args, "--vector-program-id")?;
                }
                "--mode" => mode = Mode::from_str(&next_arg(&mut args, "--mode")?)?,
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
                "--commitment" => {
                    commitment = parse_commitment(&next_arg(&mut args, "--commitment")?)?;
                }
                "--fee-payer-device" => device_payer = true,
                "--fee-payer-file" => {
                    fee_payer_file = Some(PathBuf::from(next_arg(&mut args, "--fee-payer-file")?));
                }
                "--no-compute-budget" => compute_units = None,
                "--compute-units" => {
                    compute_units = Some(next_arg(&mut args, "--compute-units")?.parse()?);
                }
                "--withdraw-lamports" => {
                    withdraw_lamports = Some(next_arg(&mut args, "--withdraw-lamports")?.parse()?);
                }
                "--withdraw-receiver" => {
                    let raw = next_arg(&mut args, "--withdraw-receiver")?;
                    withdraw_receiver =
                        Some(Pubkey::from_str(&raw).context("invalid --withdraw-receiver pubkey")?);
                }
                "--spl-mint" => {
                    let raw = next_arg(&mut args, "--spl-mint")?;
                    spl_mint = Some(Pubkey::from_str(&raw).context("invalid --spl-mint pubkey")?);
                }
                "--spl-amount" => {
                    spl_amount = Some(next_arg(&mut args, "--spl-amount")?.parse()?);
                }
                "--spl-decimals" => {
                    spl_decimals = next_arg(&mut args, "--spl-decimals")?.parse()?;
                }
                "--spl-mint-amount" => {
                    spl_mint_amount = Some(next_arg(&mut args, "--spl-mint-amount")?.parse()?);
                }
                "--spl-receiver-owner" => {
                    let raw = next_arg(&mut args, "--spl-receiver-owner")?;
                    spl_receiver_owner = Some(
                        Pubkey::from_str(&raw).context("invalid --spl-receiver-owner pubkey")?,
                    );
                }
                "--stake-lamports" => {
                    stake_lamports = Some(next_arg(&mut args, "--stake-lamports")?.parse()?);
                }
                "--stake-vote-account" => {
                    let raw = next_arg(&mut args, "--stake-vote-account")?;
                    stake_vote_account = Some(
                        Pubkey::from_str(&raw).context("invalid --stake-vote-account pubkey")?,
                    );
                }
                "--raydium-api-url" => {
                    raydium_api_url = next_arg(&mut args, "--raydium-api-url")?;
                }
                "--raydium-cpmm-pool-id" => {
                    let raw = next_arg(&mut args, "--raydium-cpmm-pool-id")?;
                    raydium_cpmm_pool_id = Some(
                        Pubkey::from_str(&raw)
                            .context("invalid --raydium-cpmm-pool-id pubkey")?,
                    );
                }
                "--jupiter-api-url" => {
                    jupiter_api_url = next_arg(&mut args, "--jupiter-api-url")?;
                }
                "--jupiter-key-file" => {
                    jupiter_key_file =
                        Some(PathBuf::from(next_arg(&mut args, "--jupiter-key-file")?));
                }
                "--jupiter-dexes" => {
                    jupiter_dexes = Some(next_arg(&mut args, "--jupiter-dexes")?);
                }
                "--jupiter-exclude-dexes" => {
                    jupiter_exclude_dexes =
                        Some(next_arg(&mut args, "--jupiter-exclude-dexes")?);
                }
                "--jupiter-mode" => {
                    jupiter_mode = Some(next_arg(&mut args, "--jupiter-mode")?);
                }
                "--jupiter-direct-only" => jupiter_direct_only = true,
                "--swap-input-mint" => {
                    let raw = next_arg(&mut args, "--swap-input-mint")?;
                    swap_input_mint = Some(
                        Pubkey::from_str(&raw).context("invalid --swap-input-mint pubkey")?,
                    );
                }
                "--swap-output-mint" => {
                    let raw = next_arg(&mut args, "--swap-output-mint")?;
                    swap_output_mint = Some(
                        Pubkey::from_str(&raw).context("invalid --swap-output-mint pubkey")?,
                    );
                }
                "--swap-amount-in" => {
                    swap_amount_in = Some(next_arg(&mut args, "--swap-amount-in")?.parse()?);
                }
                "--swap-slippage-bps" => {
                    swap_slippage_bps =
                        next_arg(&mut args, "--swap-slippage-bps")?.parse()?;
                }
                "--swap-max-accounts" => {
                    swap_max_accounts =
                        Some(next_arg(&mut args, "--swap-max-accounts")?.parse()?);
                }
                "--swap-min-out" => {
                    swap_min_out = Some(next_arg(&mut args, "--swap-min-out")?.parse()?);
                }
                "--swap-unwrap-sol" => swap_unwrap_sol = true,
                "--swap-receiver-owner" => {
                    let raw = next_arg(&mut args, "--swap-receiver-owner")?;
                    swap_receiver_owner = Some(
                        Pubkey::from_str(&raw).context("invalid --swap-receiver-owner pubkey")?,
                    );
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

        if device_payer && fee_payer_file.is_some() {
            fee_payer_file = None;
        }

        if !matches!(mode, Mode::Status | Mode::ProbeDevicePayer) && rpc_url.is_none() {
            rpc_url = Some(DEFAULT_RPC_URL.to_string());
        }

        if matches!(mode, Mode::WithdrawLamports | Mode::FullWithdraw)
            && withdraw_lamports.is_none()
        {
            withdraw_lamports = Some(DEFAULT_WITHDRAW_LAMPORTS);
        }

        if matches!(mode, Mode::FullSplTransfer) && spl_amount.is_none() {
            spl_amount = Some(DEFAULT_SPL_TRANSFER_AMOUNT);
        }
        if matches!(mode, Mode::FullRaydiumCpmmSwap | Mode::FullJupiterRouterSwap) {
            if swap_input_mint.is_none() {
                swap_input_mint = Some(
                    Pubkey::from_str(DEFAULT_SWAP_INPUT_MINT)
                        .context("invalid default Raydium swap input mint")?,
                );
            }
            if swap_output_mint.is_none() {
                swap_output_mint = Some(
                    Pubkey::from_str(DEFAULT_SWAP_OUTPUT_MINT)
                        .context("invalid default Raydium swap output mint")?,
                );
            }
            if swap_amount_in.is_none() {
                swap_amount_in = Some(DEFAULT_SWAP_AMOUNT_IN);
            }
        }
        if let (Some(_), Some(_)) = (&jupiter_dexes, &jupiter_exclude_dexes) {
            bail!("--jupiter-dexes and --jupiter-exclude-dexes are mutually exclusive");
        }

        Ok(Self {
            port,
            pin,
            rpc_url,
            mode,
            vector_program_id: Pubkey::from_str(&vector_program_id)
                .context("invalid Vector program id")?,
            read_timeout,
            action_timeout,
            open_wait,
            auto_init,
            commitment,
            device_payer,
            fee_payer_file,
            compute_units,
            withdraw_lamports,
            withdraw_receiver,
            spl_mint,
            spl_amount,
            spl_decimals,
            spl_mint_amount,
            spl_receiver_owner,
            stake_lamports,
            stake_vote_account,
            raydium_api_url,
            raydium_cpmm_pool_id,
            jupiter_api_url,
            jupiter_key_file,
            jupiter_dexes,
            jupiter_exclude_dexes,
            jupiter_mode,
            jupiter_direct_only,
            swap_slippage_bps,
            swap_max_accounts,
            swap_input_mint,
            swap_output_mint,
            swap_amount_in,
            swap_min_out,
            swap_unwrap_sol,
            swap_receiver_owner,
        })
    }
}

fn print_usage() {
    println!(
        "Usage:\n  cargo run --manifest-path tools/vector-onchain-harness/Cargo.toml -- --port /dev/cu.usbmodemXXXX [options]\n\
\nModes:\n  status                   Print device binding info and, if --rpc-url is set, current on-chain Vector account state\n  probe-device-payer      Ask the SE050 path to sign dummy Vector initialize/advance messages without using RPC\n  initialize              Submit Vector initialize if the account does not already exist\n  advance-empty           Submit an empty Vector advance against the current on-chain nonce\n  full-empty              Initialize if needed, then submit an empty advance\n  withdraw-lamports       Submit a Falcon-authorized Vector withdraw against the current nonce\n  full-withdraw           Initialize if needed, top up the Vector PDA if needed, then submit a withdraw\n  full-spl-transfer       Initialize if needed, then Falcon-authorize a transfer_checked; existing-mint mode auto-bootstraps from the device wallet when possible\n  full-stake-delegate     Initialize if needed, create a stake account owned by the Vector PDA, then Falcon-authorize delegate_stake\n  full-raydium-cpmm-swap  Initialize if needed, bootstrap the input mint into a Vector ATA, then Falcon-authorize a Raydium CPMM swap\n  full-jupiter-router-swap Initialize if needed, bootstrap the input mint into a Vector ATA, then Falcon-authorize a Jupiter router swap\n\
\nOptions:\n  --pin <digits>              6-digit PIN. Default: 246810\n  --rpc-url <url>             RPC endpoint. Required for non-status modes. Default: SOLANA_RPC_URL / RPC_URL / http://127.0.0.1:8899\n  --vector-program-id <id>    Vector Falcon program id. Default: VECTOR_PROGRAM_ID / FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU\n  --mode <name>               One of: status, initialize, advance-empty, full-empty, withdraw-lamports, full-withdraw, full-spl-transfer, full-stake-delegate, full-raydium-cpmm-swap, full-jupiter-router-swap\n  --commitment <level>        processed | confirmed | finalized. Default: confirmed\n  --fee-payer-device          Use the SE050 Ed25519 key as the outer Solana fee payer/signature\n  --fee-payer-file <path>     Use a local Solana keypair file for the outer Solana fee payer\n  --compute-units <n>         Prepend a compute-budget ix before advance. Default: 300000\n  --no-compute-budget         Do not prepend a compute-budget ix before advance\n  --withdraw-lamports <n>     Lamports to withdraw through Vector. Default for withdraw modes: 1000000\n  --withdraw-receiver <pk>    Receiver pubkey for the Vector withdraw. Default: device wallet\n  --spl-mint <pk>             Existing SPL mint to transfer under Vector control. Default: create a temporary test mint\n  --spl-amount <n>            Raw SPL units to Falcon-transfer. Default for full-spl-transfer: 10000\n  --spl-decimals <n>          Mint decimals for the temporary SPL mint. Default: 6\n  --spl-mint-amount <n>       Raw SPL units to mint into the Vector-owned source ATA. Default: 2x --spl-amount\n  --spl-receiver-owner <pk>   Owner pubkey for the destination ATA. Default: device wallet\n  --stake-lamports <n>        Lamports to put into the stake account. Default: rent + min delegation + 1000000 buffer\n  --stake-vote-account <pk>   Vote account to delegate to. Default: first vote account returned by RPC\n  --raydium-api-url <url>     Raydium API base URL for CPMM pool discovery. Default: RAYDIUM_API_URL / https://api-v3.raydium.io\n  --raydium-cpmm-pool-id <pk> Pin a specific Raydium CPMM pool id instead of auto-selecting by mints\n  --jupiter-api-url <url>     Jupiter Router API base URL. Default: JUPITER_API_URL / https://api.jup.ag\n  --jupiter-key-file <path>   File containing UNRUGGABLE_JUPITER_API_KEY entries. Default: env, ./partner_secrets.local, ../../unruggable-app/partner_secrets.local\n  --jupiter-dexes <csv>       Restrict Jupiter routing to these DEX labels\n  --jupiter-exclude-dexes <csv> Exclude these DEX labels from Jupiter routing\n  --jupiter-mode <name>       Jupiter routing mode, e.g. fast\n  --swap-input-mint <pk>      Input mint for swap modes. Default: USDC\n  --swap-output-mint <pk>     Output mint for swap modes. Default: USDT\n  --swap-amount-in <n>        Raw input token units for swap modes. Default: 10000\n  --swap-slippage-bps <n>     Slippage tolerance for Jupiter router swap mode. Default: 50\n  --swap-max-accounts <n>     Max route accounts for Jupiter router swap mode. Optional\n  --swap-min-out <n>          Raw minimum output token units for Raydium CPMM swap mode. Required for that mode\n  --swap-receiver-owner <pk>  Owner pubkey for the swap output ATA. Default: device wallet\n  --read-timeout-ms <n>       Default command wait time in ms. Default: 4000\n  --action-timeout-ms <n>     Long command wait time in ms. Default: 35000\n  --open-wait-ms <n>          Delay after opening serial port in ms. Default: 2000\n  --no-auto-init              Fail instead of calling PQC_INIT when PQC is blank\n  -h, --help                  Show this help text"
    );
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn parse_commitment(raw: &str) -> Result<CommitmentConfig> {
    match raw {
        "processed" => Ok(CommitmentConfig::processed()),
        "confirmed" => Ok(CommitmentConfig::confirmed()),
        "finalized" => Ok(CommitmentConfig::finalized()),
        _ => bail!("unsupported commitment level: {raw}"),
    }
}

fn parse_bool_flag(raw: &str) -> Result<bool> {
    match raw {
        "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON" => Ok(true),
        "0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF" => Ok(false),
        _ => bail!("unsupported boolean value: {raw}"),
    }
}

fn default_keypair_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/solana/id.json"))
}

fn parse_local_secret_file(path: &Path) -> BTreeMap<String, String> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }

            let (key, value) = trimmed.split_once('=')?;
            let mut value = value.trim().to_string();
            if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value = value[1..value.len() - 1].to_string();
            }
            Some((key.trim().to_string(), value))
        })
        .collect()
}

fn default_jupiter_key_file_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(DEFAULT_PARTNER_SECRETS_FILE));
        candidates.push(cwd.join("../../unruggable-app").join(DEFAULT_PARTNER_SECRETS_FILE));
    }
    candidates
}

fn resolve_jupiter_api_key(config: &Config) -> Result<String> {
    for env_name in [VECTOR_JUPITER_API_KEY_ENV, JUPITER_API_KEY_ENV] {
        if let Ok(value) = env::var(env_name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    let mut candidates = Vec::new();
    if let Some(path) = &config.jupiter_key_file {
        candidates.push(path.clone());
    }
    candidates.extend(default_jupiter_key_file_candidates());

    for path in candidates {
        let secrets = parse_local_secret_file(&path);
        if let Some(value) = secrets.get(JUPITER_API_KEY_ENV) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        if let Some(value) = secrets.get(VECTOR_JUPITER_API_KEY_ENV) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    bail!(
        "missing Jupiter API key; set {} or {} in the environment, or provide --jupiter-key-file / a partner_secrets.local file",
        JUPITER_API_KEY_ENV,
        VECTOR_JUPITER_API_KEY_ENV
    )
}

fn run(config: Config) -> Result<()> {
    println!("============================================================");
    println!("Vector On-Chain Harness");
    println!("============================================================");
    println!("Port: {}", config.port);
    println!("Mode: {:?}", config.mode);
    println!("Vector Program: {}", config.vector_program_id);
    if let Some(lamports) = config.withdraw_lamports {
        println!("Withdraw Lamports:    {}", lamports);
    }
    if let Some(receiver) = config.withdraw_receiver {
        println!("Withdraw Receiver:    {}", receiver);
    }
    if let Some(mint) = config.spl_mint {
        println!("SPL Mint:             {}", mint);
    }
    if let Some(amount) = config.spl_amount {
        println!("SPL Transfer Amount:  {}", amount);
    }
    if let Some(amount) = config.spl_mint_amount {
        println!("SPL Mint Amount:      {}", amount);
    }
    if let Some(receiver) = config.spl_receiver_owner {
        println!("SPL Receiver Owner:   {}", receiver);
    }
    if matches!(config.mode, Mode::FullSplTransfer) {
        println!("SPL Decimals:         {}", config.spl_decimals);
    }
    if let Some(lamports) = config.stake_lamports {
        println!("Stake Lamports:       {}", lamports);
    }
    if let Some(vote_account) = config.stake_vote_account {
        println!("Stake Vote Account:   {}", vote_account);
    }
    if let Some(input_mint) = config.swap_input_mint {
        println!("Swap Input Mint:      {}", input_mint);
    }
    if let Some(output_mint) = config.swap_output_mint {
        println!("Swap Output Mint:     {}", output_mint);
    }
    if let Some(amount_in) = config.swap_amount_in {
        println!("Swap Amount In:       {}", amount_in);
    }
    if let Some(min_out) = config.swap_min_out {
        println!("Swap Min Out:         {}", min_out);
    }
    if config.swap_unwrap_sol {
        println!("Swap Unwrap SOL:      true");
    }
    if let Some(receiver) = config.swap_receiver_owner {
        println!("Swap Receiver Owner:  {}", receiver);
    }
    if matches!(config.mode, Mode::FullJupiterRouterSwap) {
        println!("Jupiter API URL:      {}", config.jupiter_api_url);
        println!("Swap Slippage Bps:    {}", config.swap_slippage_bps);
        if let Some(max_accounts) = config.swap_max_accounts {
            println!("Swap Max Accounts:    {}", max_accounts);
        }
        if let Some(dexes) = &config.jupiter_dexes {
            println!("Jupiter Dexes:        {}", dexes);
        }
        if let Some(exclude_dexes) = &config.jupiter_exclude_dexes {
            println!("Jupiter Exclude Dexes: {}", exclude_dexes);
        }
        if let Some(mode) = &config.jupiter_mode {
            println!("Jupiter Mode:         {}", mode);
        }
        if config.jupiter_direct_only {
            println!("Jupiter Direct Only:  true");
        }
    }
    if matches!(config.mode, Mode::FullRaydiumCpmmSwap) {
        println!("Raydium API URL:      {}", config.raydium_api_url);
    }
    if let Some(pool_id) = config.raydium_cpmm_pool_id {
        println!("Raydium CPMM Pool:    {}", pool_id);
    }

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

    unlock_device(&mut *port, &config.pin, config.read_timeout)?;

    let se050_pubkey = get_se050_pubkey(&mut *port, config.read_timeout)?;
    let pqc = get_or_init_pqc(
        &mut *port,
        &se050_pubkey,
        config.read_timeout,
        config.action_timeout,
        config.auto_init,
    )?;

    let vector_identity = falcon512_identity(&pqc.wire_pubkey);
    let (vector_pda, expected_bump) = find_vector_pda(&config.vector_program_id, &vector_identity);

    println!();
    println!("Device binding");
    println!("SE050 pubkey:         {}", se050_pubkey);
    println!("Falcon pubkey b64:    {}", pqc.wire_pubkey_b64);
    println!("Vector identity hex:  {}", hex::encode(vector_identity));
    println!("Vector identity b64:  {}", STANDARD.encode(vector_identity));
    println!("Vector PDA:           {}", vector_pda);
    println!("Vector PDA bump:      {}", expected_bump);

    let rpc_client = match &config.rpc_url {
        Some(url) => {
            println!("RPC URL:              {url}");
            Some(RpcClient::new_with_commitment(
                url.clone(),
                config.commitment,
            ))
        }
        None => None,
    };

    let current_account = if let Some(client) = &rpc_client {
        let account = fetch_vector_account(client, &vector_pda, config.commitment)?;
        print_onchain_status(account.as_ref(), expected_bump);
        account
    } else {
        None
    };

    if config.mode == Mode::Status {
        return Ok(());
    }

    if config.mode == Mode::ProbeDevicePayer {
        probe_device_payer_messages(
            &mut *port,
            &config,
            se050_pubkey,
            &pqc.wire_pubkey,
            &vector_identity,
        )?;
        return Ok(());
    }

    let client = rpc_client
        .as_ref()
        .ok_or_else(|| anyhow!("--rpc-url is required for this mode"))?;
    let mut payer = resolve_fee_payer(&config, se050_pubkey)?;
    println!();
    println!("Outer fee payer");
    println!("Source:               {}", payer.label());
    match client.get_balance(&payer.pubkey()) {
        Ok(balance) => println!(
            "Balance:              {} SOL ({balance} lamports)",
            lamports_to_sol(balance)
        ),
        Err(err) => println!("Balance:              <rpc error: {err}>"),
    }

    let account_after_init = match config.mode {
        Mode::Initialize
        | Mode::FullEmpty
        | Mode::FullWithdraw
        | Mode::FullSplTransfer
        | Mode::FullStakeDelegate
        | Mode::FullRaydiumCpmmSwap
        | Mode::FullJupiterRouterSwap => initialize_if_needed(
            client,
            &mut *port,
            &config,
            &mut payer,
            &pqc.wire_pubkey,
            &vector_pda,
            current_account,
            expected_bump,
        )?,
        Mode::ProbeDevicePayer => unreachable!(),
        Mode::AdvanceEmpty | Mode::WithdrawLamports => current_account.ok_or_else(|| {
            anyhow!(
                "Vector account does not exist yet; run initialize, full-empty, or full-withdraw first"
            )
        })?,
        Mode::Status => unreachable!(),
    };

    if config.mode == Mode::Initialize {
        return Ok(());
    }

    match config.mode {
        Mode::AdvanceEmpty | Mode::FullEmpty => submit_empty_advance(
            client,
            &mut *port,
            &config,
            &mut payer,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::WithdrawLamports | Mode::FullWithdraw => submit_withdraw_advance(
            client,
            &mut *port,
            &config,
            &mut payer,
            se050_pubkey,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::FullSplTransfer => submit_full_spl_transfer(
            client,
            &mut *port,
            &config,
            &mut payer,
            se050_pubkey,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::FullStakeDelegate => submit_full_stake_delegate(
            client,
            &mut *port,
            &config,
            &mut payer,
            se050_pubkey,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::FullRaydiumCpmmSwap => submit_full_raydium_cpmm_swap(
            client,
            &mut *port,
            &config,
            &mut payer,
            se050_pubkey,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::FullJupiterRouterSwap => submit_full_jupiter_router_swap(
            client,
            &mut *port,
            &config,
            &mut payer,
            se050_pubkey,
            &pqc,
            &vector_identity,
            &vector_pda,
            expected_bump,
            account_after_init,
        )?,
        Mode::Status | Mode::ProbeDevicePayer | Mode::Initialize => unreachable!(),
    }

    Ok(())
}

fn print_onchain_status(account: Option<&VectorAccountState>, expected_bump: u8) {
    println!();
    println!("On-chain Vector account");
    match account {
        Some(account) => {
            println!("Status:               present");
            println!("Nonce hex:            {}", hex::encode(account.nonce));
            println!("Bump:                 {}", account.bump);
            println!("Expected bump:        {}", expected_bump);
            println!(
                "Lamports:             {} SOL ({})",
                lamports_to_sol(account.lamports),
                account.lamports
            );
            println!("Data length:          {} bytes", account.data_len);
        }
        None => {
            println!("Status:               absent");
        }
    }
}

fn resolve_fee_payer(config: &Config, se050_pubkey: Pubkey) -> Result<FeePayer> {
    if config.device_payer {
        return Ok(FeePayer::Device {
            pubkey: se050_pubkey,
        });
    }

    let path = config
        .fee_payer_file
        .clone()
        .ok_or_else(|| anyhow!("missing fee payer; pass --fee-payer-device or --fee-payer-file"))?;
    let keypair = read_keypair_file(&path).map_err(|err| {
        anyhow!(
            "failed to read fee payer keypair file {}: {err}",
            path.display()
        )
    })?;
    Ok(FeePayer::Local { keypair, path })
}

fn initialize_if_needed(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    wire_pubkey: &[u8],
    vector_pda: &Pubkey,
    current_account: Option<VectorAccountState>,
    expected_bump: u8,
) -> Result<VectorAccountState> {
    if let Some(account) = current_account {
        println!();
        println!("Initialize");
        println!("Skipped:              Vector account already exists");
        ensure!(
            account.bump == expected_bump,
            "existing Vector bump {} does not match derived bump {}",
            account.bump,
            expected_bump
        );
        return Ok(account);
    }

    println!();
    println!("Initialize");

    let init_ix =
        create_initialize_falcon512(config.vector_program_id, payer.pubkey(), wire_pubkey)?;
    let tx_sig = submit_transaction(client, port, config, payer, &[init_ix])?;
    println!("Initialize tx:        {}", tx_sig);

    let account = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account still missing after initialize"))?;
    ensure!(
        account.bump == expected_bump,
        "initialized Vector bump {} does not match derived bump {}",
        account.bump,
        expected_bump
    );
    println!("Nonce hex:            {}", hex::encode(account.nonce));
    println!(
        "Lamports:             {} SOL ({})",
        lamports_to_sol(account.lamports),
        account.lamports
    );
    Ok(account)
}

fn probe_device_payer_messages(
    port: &mut dyn SerialPort,
    config: &Config,
    se050_pubkey: Pubkey,
    wire_pubkey: &[u8],
    vector_identity: &[u8; 32],
) -> Result<()> {
    println!();
    println!("Device payer probe");
    println!("Outer payer:          {}", se050_pubkey);

    let init_ix = create_initialize_falcon512(config.vector_program_id, se050_pubkey, wire_pubkey)?;
    let init_message = Message::new(&[init_ix], Some(&se050_pubkey));
    let init_result =
        probe_outer_sign_message(port, config, se050_pubkey, &init_message.serialize());
    print_probe_result("Initialize probe", &init_result);

    let mut advance_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        advance_instructions.push(compute_budget_set_compute_unit_limit(units)?);
    }
    advance_instructions.push(create_advance_instruction(
        config.vector_program_id,
        vector_identity,
        &vec![0u8; FALCON_512_SIGNATURE_LEN],
    )?);
    let advance_message = Message::new(&advance_instructions, Some(&se050_pubkey));
    let advance_result =
        probe_outer_sign_message(port, config, se050_pubkey, &advance_message.serialize());
    print_probe_result("Advance probe", &advance_result);

    if let Err(err) = init_result {
        bail!("Vector initialize outer-sign probe failed: {err}");
    }
    if let Err(err) = advance_result {
        bail!("Vector advance outer-sign probe failed: {err}");
    }

    Ok(())
}

fn probe_outer_sign_message(
    port: &mut dyn SerialPort,
    config: &Config,
    se050_pubkey: Pubkey,
    message_bytes: &[u8],
) -> Result<()> {
    unlock_device(port, &config.pin, config.read_timeout)?;
    let signature = sign_message_with_se050(port, message_bytes, config.action_timeout)?;
    ensure!(
        signature.verify(se050_pubkey.as_ref(), message_bytes),
        "SE050 returned a signature that does not verify"
    );
    Ok(())
}

fn print_probe_result(label: &str, result: &Result<()>) {
    match result {
        Ok(()) => println!("{label}:     signed"),
        Err(err) => println!("{label}:     failed ({err})"),
    }
}

fn submit_empty_advance(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    println!();
    println!("Advance");

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        &[],
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);

    let tx_sig = submit_transaction(client, port, config, payer, &instructions)?;
    println!("Advance tx:           {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after advance"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after advance: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );

    println!("Next nonce hex:       {}", hex::encode(updated.nonce));
    println!(
        "Lamports:             {} SOL ({})",
        lamports_to_sol(updated.lamports),
        updated.lamports
    );
    Ok(())
}

fn submit_withdraw_advance(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    device_wallet: Pubkey,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    let withdraw_amount = config
        .withdraw_lamports
        .ok_or_else(|| anyhow!("missing --withdraw-lamports"))?;
    ensure!(
        withdraw_amount > 0,
        "--withdraw-lamports must be greater than zero"
    );

    let receiver = config.withdraw_receiver.unwrap_or(device_wallet);

    println!();
    println!("Withdraw");
    println!("Device wallet:        {}", device_wallet);
    println!("Relayer / payer:      {}", payer.pubkey());
    println!("Receiver:             {}", receiver);
    println!(
        "Requested:            {} SOL ({withdraw_amount} lamports)",
        lamports_to_sol(withdraw_amount)
    );

    let rent_floor = client
        .get_minimum_balance_for_rent_exemption(account.data_len)
        .context("failed to fetch rent floor for Vector account")?;
    ensure!(
        account.lamports >= rent_floor,
        "Vector account balance {} is already below rent floor {}",
        account.lamports,
        rent_floor
    );

    let withdrawable_before = account.lamports - rent_floor;
    println!(
        "Vector withdrawable:  {} SOL ({} lamports)",
        lamports_to_sol(withdrawable_before),
        withdrawable_before
    );
    println!(
        "Rent floor:           {} SOL ({rent_floor} lamports)",
        lamports_to_sol(rent_floor)
    );

    let account = if withdrawable_before < withdraw_amount {
        let top_up_amount = withdraw_amount - withdrawable_before;
        println!(
            "Top-up needed:        {} SOL ({top_up_amount} lamports)",
            lamports_to_sol(top_up_amount)
        );
        let device_balance = client.get_balance(&device_wallet).ok();
        if let Some(balance) = device_balance {
            println!(
                "Device SOL balance:   {} SOL ({balance} lamports)",
                lamports_to_sol(balance)
            );
            ensure!(
                balance >= top_up_amount,
                "device wallet {} holds {} lamports but needs at least {} lamports to top up the Vector PDA",
                device_wallet,
                balance,
                top_up_amount
            );
        }

        let top_up_ix =
            create_system_transfer_instruction(device_wallet, *vector_pda, top_up_amount)?;
        let tx_sig = if payer.pubkey() == device_wallet {
            submit_transaction(client, port, config, payer, &[top_up_ix])?
        } else {
            submit_transaction_with_device_signer(
                client,
                port,
                config,
                payer,
                &[top_up_ix],
                &device_wallet,
            )?
        };
        println!("Top-up tx:            {}", tx_sig);

        let updated = fetch_vector_account(client, vector_pda, config.commitment)?
            .ok_or_else(|| anyhow!("Vector account disappeared after top-up"))?;
        ensure!(
            updated.lamports == account.lamports + top_up_amount,
            "Vector top-up mismatch: expected {} got {}",
            account.lamports + top_up_amount,
            updated.lamports
        );
        println!(
            "Lamports after top-up: {} SOL ({})",
            lamports_to_sol(updated.lamports),
            updated.lamports
        );
        updated
    } else {
        account
    };

    let receiver_balance_before = client.get_balance(&receiver).ok();

    let withdraw_sub = create_withdraw_subinstruction(
        config.vector_program_id,
        vector_identity,
        receiver,
        withdraw_amount,
    )?;
    let passthrough_ix =
        create_passthrough_instruction(config.vector_program_id, vector_identity, &[withdraw_sub])?;

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        std::slice::from_ref(&passthrough_ix),
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);
    instructions.push(passthrough_ix);

    let tx_sig = submit_transaction(client, port, config, payer, &instructions)?;
    println!("Withdraw tx:          {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after withdraw"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after withdraw: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );

    let expected_lamports = account
        .lamports
        .checked_sub(withdraw_amount)
        .ok_or_else(|| anyhow!("Vector lamports underflow after withdraw"))?;
    ensure!(
        updated.lamports == expected_lamports,
        "Vector lamports mismatch after withdraw: expected {} got {}",
        expected_lamports,
        updated.lamports
    );

    println!("Next nonce hex:       {}", hex::encode(updated.nonce));
    println!(
        "Vector lamports:      {} SOL ({}) -> {} SOL ({})",
        lamports_to_sol(account.lamports),
        account.lamports,
        lamports_to_sol(updated.lamports),
        updated.lamports
    );

    match client.get_balance(&receiver) {
        Ok(after) => {
            println!(
                "Receiver balance:     {} SOL ({})",
                lamports_to_sol(after),
                after
            );
            if let Some(before) = receiver_balance_before {
                if receiver != payer.pubkey() {
                    ensure!(
                        after == before + withdraw_amount,
                        "receiver balance mismatch after withdraw: expected {} got {}",
                        before + withdraw_amount,
                        after
                    );
                } else {
                    println!(
                        "Receiver note:        receiver is the outer fee payer, so its balance delta also includes tx fees"
                    );
                }
            }
        }
        Err(err) => {
            println!("Receiver balance:     <rpc error: {err}>");
        }
    }

    Ok(())
}

fn submit_full_spl_transfer(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    device_wallet: Pubkey,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    let transfer_amount = config
        .spl_amount
        .ok_or_else(|| anyhow!("missing --spl-amount"))?;
    ensure!(
        transfer_amount > 0,
        "--spl-amount must be greater than zero"
    );

    let receiver_owner = config.spl_receiver_owner.unwrap_or(device_wallet);
    let token_program_id = spl_token_program_id();

    println!();
    println!("SPL transfer");
    let (mint_pubkey, source_ata, destination_ata, decimals, source_before, destination_before) =
        match config.spl_mint {
            Some(existing_mint) => {
                let source_ata = get_associated_token_address_with_program_id(
                    vector_pda,
                    &existing_mint,
                    &token_program_id,
                );
                let native_source_ata = get_associated_token_address_with_program_id(
                    &device_wallet,
                    &existing_mint,
                    &token_program_id,
                );
                let destination_ata = get_associated_token_address_with_program_id(
                    &receiver_owner,
                    &existing_mint,
                    &token_program_id,
                );
                ensure!(
                    config.spl_mint_amount.is_none(),
                    "--spl-mint-amount is only valid when creating a temporary test mint"
                );
                let mint_state = fetch_spl_mint_state(client, &existing_mint, config.commitment)?;
                let decimals = mint_state.decimals;

                println!("Mint mode:            existing");
                println!("Mint:                 {}", existing_mint);
                println!("Token program:        {}", token_program_id);
                println!("Device wallet:        {}", device_wallet);
                println!("Device wallet ATA:    {}", native_source_ata);
                println!("Relayer / payer:      {}", payer.pubkey());
                println!("Source ATA:           {}", source_ata);
                println!("Destination ATA:      {}", destination_ata);
                println!("Destination owner:    {}", receiver_owner);
                println!(
                    "Transfer amount:      {} raw units ({})",
                    transfer_amount,
                    format_token_amount(transfer_amount, decimals)
                );

                let mut setup_instructions = Vec::new();
                if !account_exists(client, &source_ata, config.commitment)? {
                    setup_instructions.push(create_associated_token_account_idempotent(
                        &payer.pubkey(),
                        vector_pda,
                        &existing_mint,
                        &token_program_id,
                    ));
                }
                if destination_ata != source_ata
                    && !account_exists(client, &destination_ata, config.commitment)?
                {
                    setup_instructions.push(create_associated_token_account_idempotent(
                        &payer.pubkey(),
                        &receiver_owner,
                        &existing_mint,
                        &token_program_id,
                    ));
                }
                if native_source_ata != source_ata
                    && native_source_ata != destination_ata
                    && !account_exists(client, &native_source_ata, config.commitment)?
                {
                    setup_instructions.push(create_associated_token_account_idempotent(
                        &payer.pubkey(),
                        &device_wallet,
                        &existing_mint,
                        &token_program_id,
                    ));
                }
                if setup_instructions.is_empty() {
                    println!("ATA setup:            already ready");
                } else {
                    let setup_tx =
                        submit_transaction(client, port, config, payer, &setup_instructions)?;
                    println!("ATA setup tx:         {}", setup_tx);
                }

                let mut source_before =
                    fetch_spl_token_account_state(client, &source_ata, config.commitment)?;
                let mut destination_before =
                    fetch_spl_token_account_state(client, &destination_ata, config.commitment)?;
                ensure!(
                    source_before.owner == *vector_pda,
                    "source ATA owner mismatch: expected {} got {}",
                    vector_pda,
                    source_before.owner
                );
                ensure!(
                    destination_before.owner == receiver_owner,
                    "destination ATA owner mismatch: expected {} got {}",
                    receiver_owner,
                    destination_before.owner
                );
                ensure!(
                    source_before.mint == existing_mint,
                    "source ATA mint mismatch: expected {} got {}",
                    existing_mint,
                    source_before.mint
                );
                ensure!(
                    destination_before.mint == existing_mint,
                    "destination ATA mint mismatch: expected {} got {}",
                    existing_mint,
                    destination_before.mint
                );

                if source_before.amount < transfer_amount {
                    let native_before = fetch_spl_token_account_state(
                        client,
                        &native_source_ata,
                        config.commitment,
                    )?;
                    ensure!(
                        native_before.owner == device_wallet,
                        "device ATA owner mismatch: expected {} got {}",
                        device_wallet,
                        native_before.owner
                    );
                    ensure!(
                        native_before.mint == existing_mint,
                        "device ATA mint mismatch: expected {} got {}",
                        existing_mint,
                        native_before.mint
                    );

                    let top_up_amount = transfer_amount - source_before.amount;
                    println!(
                        "Bootstrap shortfall:   {} raw units ({})",
                        top_up_amount,
                        format_token_amount(top_up_amount, decimals)
                    );
                    println!(
                        "Native balance:        {} raw units ({})",
                        native_before.amount,
                        format_token_amount(native_before.amount, decimals)
                    );

                    if native_before.amount >= top_up_amount {
                        let bootstrap_ix = spl_transfer(
                            &token_program_id,
                            &native_source_ata,
                            &source_ata,
                            &device_wallet,
                            &[],
                            top_up_amount,
                        )
                        .map_err(|err| anyhow!("failed to build bootstrap transfer: {err}"))?;
                        let bootstrap_tx = submit_transaction_with_device_payer(
                            client,
                            port,
                            config,
                            &[bootstrap_ix],
                            &device_wallet,
                        )?;
                        println!("Bootstrap tx:         {}", bootstrap_tx);

                        source_before =
                            fetch_spl_token_account_state(client, &source_ata, config.commitment)?;
                        destination_before = fetch_spl_token_account_state(
                            client,
                            &destination_ata,
                            config.commitment,
                        )?;
                        println!(
                            "Source after bootstrap: {} raw units ({})",
                            source_before.amount,
                            format_token_amount(source_before.amount, decimals)
                        );
                    } else {
                        let device_sol_balance = client.get_balance(&device_wallet).ok();
                        let relayer_sol_balance = client.get_balance(&payer.pubkey()).ok();
                        bail!(
                            "Vector source ATA {} holds {} raw units ({}) of mint {} and needs {} more raw units ({}). Device wallet {} ATA {} holds {} raw units ({}). Fund the device wallet with this mint.{}{}",
                            source_ata,
                            source_before.amount,
                            format_token_amount(source_before.amount, decimals),
                            existing_mint,
                            top_up_amount,
                            format_token_amount(top_up_amount, decimals),
                            device_wallet,
                            native_source_ata,
                            native_before.amount,
                            format_token_amount(native_before.amount, decimals),
                            match device_sol_balance {
                                Some(balance) => format!(
                                    " Device SOL balance: {} SOL / {} lamports.",
                                    lamports_to_sol(balance),
                                    balance
                                ),
                                None => String::new(),
                            }
                            ,
                            match relayer_sol_balance {
                                Some(balance) => format!(
                                    " Relayer {} SOL balance: {} SOL / {} lamports.",
                                    payer.pubkey(),
                                    lamports_to_sol(balance),
                                    balance
                                ),
                                None => String::new(),
                            }
                        );
                    }
                }

                ensure!(
                    source_before.amount >= transfer_amount,
                    "Vector source ATA {} still holds only {} raw units ({}) of mint {} after bootstrap",
                    source_ata,
                    source_before.amount,
                    format_token_amount(source_before.amount, decimals),
                    existing_mint
                );

                (
                    existing_mint,
                    source_ata,
                    destination_ata,
                    decimals,
                    source_before,
                    destination_before,
                )
            }
            None => {
                let mint = Keypair::new();
                let mint_pubkey = mint.pubkey();
                let mint_amount = match config.spl_mint_amount {
                    Some(amount) => amount,
                    None => transfer_amount
                        .checked_mul(2)
                        .ok_or_else(|| anyhow!("default SPL mint amount overflowed"))?,
                };
                ensure!(
                    mint_amount >= transfer_amount,
                    "--spl-mint-amount must be at least --spl-amount"
                );

                let decimals = config.spl_decimals;
                let source_ata = get_associated_token_address_with_program_id(
                    vector_pda,
                    &mint_pubkey,
                    &token_program_id,
                );
                let destination_ata = get_associated_token_address_with_program_id(
                    &receiver_owner,
                    &mint_pubkey,
                    &token_program_id,
                );

                println!("Mint mode:            temporary");
                println!("Mint:                 {}", mint_pubkey);
                println!("Token program:        {}", token_program_id);
                println!("Source ATA:           {}", source_ata);
                println!("Destination ATA:      {}", destination_ata);
                println!("Destination owner:    {}", receiver_owner);
                println!(
                    "Transfer amount:      {} raw units ({})",
                    transfer_amount,
                    format_token_amount(transfer_amount, decimals)
                );
                println!(
                    "Mint amount:          {} raw units ({})",
                    mint_amount,
                    format_token_amount(mint_amount, decimals)
                );

                let mint_rent = client
                    .get_minimum_balance_for_rent_exemption(SplTokenMint::LEN)
                    .context("failed to fetch mint rent exemption")?;
                let create_mint_ix = system_create_account(
                    &payer.pubkey(),
                    &mint_pubkey,
                    mint_rent,
                    SplTokenMint::LEN as u64,
                    &token_program_id,
                );
                let init_mint_ix = spl_initialize_mint2(
                    &token_program_id,
                    &mint_pubkey,
                    &payer.pubkey(),
                    None,
                    decimals,
                )
                .map_err(|err| anyhow!("failed to build initialize_mint2: {err}"))?;
                let create_source_ata_ix = create_associated_token_account_idempotent(
                    &payer.pubkey(),
                    vector_pda,
                    &mint_pubkey,
                    &token_program_id,
                );
                let create_destination_ata_ix = create_associated_token_account_idempotent(
                    &payer.pubkey(),
                    &receiver_owner,
                    &mint_pubkey,
                    &token_program_id,
                );

                let setup_tx = submit_transaction_with_extra_signers(
                    client,
                    port,
                    config,
                    payer,
                    &[
                        create_mint_ix,
                        init_mint_ix,
                        create_source_ata_ix,
                        create_destination_ata_ix,
                    ],
                    &[&mint],
                )?;
                println!("Mint setup tx:        {}", setup_tx);

                let mint_state = fetch_spl_mint_state(client, &mint_pubkey, config.commitment)?;
                ensure!(
                    mint_state.decimals == decimals,
                    "mint decimals mismatch: expected {} got {}",
                    decimals,
                    mint_state.decimals
                );

                let mint_to_ix = spl_mint_to(
                    &token_program_id,
                    &mint_pubkey,
                    &source_ata,
                    &payer.pubkey(),
                    &[],
                    mint_amount,
                )
                .map_err(|err| anyhow!("failed to build mint_to instruction: {err}"))?;
                let mint_to_tx = submit_transaction(client, port, config, payer, &[mint_to_ix])?;
                println!("Mint-to tx:           {}", mint_to_tx);

                let source_before =
                    fetch_spl_token_account_state(client, &source_ata, config.commitment)?;
                let destination_before =
                    fetch_spl_token_account_state(client, &destination_ata, config.commitment)?;
                ensure!(
                    source_before.owner == *vector_pda,
                    "source ATA owner mismatch: expected {} got {}",
                    vector_pda,
                    source_before.owner
                );
                ensure!(
                    destination_before.owner == receiver_owner,
                    "destination ATA owner mismatch: expected {} got {}",
                    receiver_owner,
                    destination_before.owner
                );
                ensure!(
                    source_before.mint == mint_pubkey,
                    "source ATA mint mismatch: expected {} got {}",
                    mint_pubkey,
                    source_before.mint
                );
                ensure!(
                    destination_before.mint == mint_pubkey,
                    "destination ATA mint mismatch: expected {} got {}",
                    mint_pubkey,
                    destination_before.mint
                );
                ensure!(
                    source_before.amount == mint_amount,
                    "source ATA amount mismatch after mint: expected {} got {}",
                    mint_amount,
                    source_before.amount
                );
                ensure!(
                    destination_before.amount == 0,
                    "destination ATA should start empty but holds {}",
                    destination_before.amount
                );

                (
                    mint_pubkey,
                    source_ata,
                    destination_ata,
                    decimals,
                    source_before,
                    destination_before,
                )
            }
        };

    println!(
        "Source balance:       {} raw units ({})",
        source_before.amount,
        format_token_amount(source_before.amount, decimals)
    );
    println!(
        "Destination balance:  {} raw units ({})",
        destination_before.amount,
        format_token_amount(destination_before.amount, decimals)
    );

    let transfer_sub = create_spl_transfer_checked_subinstruction(
        mint_pubkey,
        source_ata,
        destination_ata,
        *vector_pda,
        transfer_amount,
        decimals,
    )?;
    let passthrough_ix =
        create_passthrough_instruction(config.vector_program_id, vector_identity, &[transfer_sub])?;

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        std::slice::from_ref(&passthrough_ix),
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);
    instructions.push(passthrough_ix);

    let tx_sig = submit_transaction(client, port, config, payer, &instructions)?;
    println!("SPL transfer tx:      {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after SPL transfer"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after SPL transfer: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );
    ensure!(
        updated.lamports == account.lamports,
        "Vector lamports changed during SPL transfer: expected {} got {}",
        account.lamports,
        updated.lamports
    );

    let source_after = fetch_spl_token_account_state(client, &source_ata, config.commitment)?;
    let destination_after =
        fetch_spl_token_account_state(client, &destination_ata, config.commitment)?;
    ensure!(
        source_after.amount == source_before.amount - transfer_amount,
        "source ATA amount mismatch after transfer: expected {} got {}",
        source_before.amount - transfer_amount,
        source_after.amount
    );
    ensure!(
        destination_after.amount == destination_before.amount + transfer_amount,
        "destination ATA amount mismatch after transfer: expected {} got {}",
        destination_before.amount + transfer_amount,
        destination_after.amount
    );

    println!("Next nonce hex:       {}", hex::encode(updated.nonce));
    println!(
        "Vector lamports:      {} SOL ({})",
        lamports_to_sol(updated.lamports),
        updated.lamports
    );
    println!(
        "Source after:         {} raw units ({})",
        source_after.amount,
        format_token_amount(source_after.amount, decimals)
    );
    println!(
        "Destination after:    {} raw units ({})",
        destination_after.amount,
        format_token_amount(destination_after.amount, decimals)
    );

    Ok(())
}

fn submit_full_stake_delegate(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    device_wallet: Pubkey,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    let vote_account = match config.stake_vote_account {
        Some(vote_account) => validate_vote_account(client, vote_account)?,
        None => choose_vote_account(client)?,
    };
    let stake_rent = client
        .get_minimum_balance_for_rent_exemption(StakeStateV2::size_of())
        .context("failed to fetch stake-account rent exemption")?;
    let minimum_delegation = client
        .get_stake_minimum_delegation()
        .context("failed to fetch stake minimum delegation")?;
    let minimum_required = stake_rent
        .checked_add(minimum_delegation)
        .ok_or_else(|| anyhow!("stake rent + minimum delegation overflowed"))?;
    let stake_lamports = match config.stake_lamports {
        Some(lamports) => lamports,
        None => minimum_required
            .checked_add(DEFAULT_STAKE_LAMPORT_BUFFER)
            .ok_or_else(|| anyhow!("default stake funding amount overflowed"))?,
    };
    ensure!(
        stake_lamports >= minimum_required,
        "--stake-lamports must be at least rent + minimum delegation ({} + {} = {})",
        stake_rent,
        minimum_delegation,
        minimum_required
    );
    let device_top_up_amount = stake_lamports
        .checked_sub(stake_rent)
        .ok_or_else(|| anyhow!("stake lamports unexpectedly below rent floor"))?;

    let stake_account = Keypair::new();
    let stake_pubkey = stake_account.pubkey();
    let authorized = StakeAuthorized::auto(vector_pda);
    let lockup = StakeLockup::default();
    let device_wallet_rent_floor = client
        .get_minimum_balance_for_rent_exemption(0)
        .context("failed to fetch device-wallet rent floor")?;
    let (fee_blockhash, _slot) = client
        .get_latest_blockhash_with_commitment(config.commitment)
        .context("failed to fetch latest blockhash for stake setup fee estimate")?;
    let device_top_up_ix =
        create_system_transfer_instruction(device_wallet, stake_pubkey, device_top_up_amount)?;
    let top_up_fee = client
        .get_fee_for_message(&Message::new_with_blockhash(
            std::slice::from_ref(&device_top_up_ix),
            Some(&device_wallet),
            &fee_blockhash,
        ))
        .context("failed to estimate stake top-up fee")?;
    let required_device_balance = device_top_up_amount
        .checked_add(device_wallet_rent_floor)
        .and_then(|value| value.checked_add(top_up_fee))
        .ok_or_else(|| anyhow!("required device balance overflowed"))?;

    println!();
    println!("Stake delegate");
    println!("Device wallet:        {}", device_wallet);
    println!("Relayer / payer:      {}", payer.pubkey());
    println!("Stake account:        {}", stake_pubkey);
    println!("Vote account:         {}", vote_account);
    println!(
        "Stake lamports:       {} SOL ({})",
        lamports_to_sol(stake_lamports),
        stake_lamports
    );
    println!(
        "Stake rent floor:     {} SOL ({})",
        lamports_to_sol(stake_rent),
        stake_rent
    );
    println!(
        "Min delegation:       {} SOL ({})",
        lamports_to_sol(minimum_delegation),
        minimum_delegation
    );
    println!(
        "Relayer shell funding:{} SOL ({})",
        lamports_to_sol(stake_rent),
        stake_rent
    );
    println!(
        "Device top-up amount: {} SOL ({})",
        lamports_to_sol(device_top_up_amount),
        device_top_up_amount
    );
    println!(
        "Wallet rent floor:    {} SOL ({})",
        lamports_to_sol(device_wallet_rent_floor),
        device_wallet_rent_floor
    );
    println!(
        "Top-up fee estimate:  {} SOL ({})",
        lamports_to_sol(top_up_fee),
        top_up_fee
    );
    match client.get_balance(&device_wallet) {
        Ok(balance) => {
            println!(
                "Device SOL balance:   {} SOL ({balance} lamports)",
                lamports_to_sol(balance)
            );
            ensure!(
                balance >= required_device_balance,
                "device wallet {} holds {} lamports but needs at least {} lamports to fund the stake account, cover setup fees, and remain rent-safe (shortfall: {} lamports)",
                device_wallet,
                balance,
                required_device_balance,
                required_device_balance.saturating_sub(balance)
            );
        }
        Err(err) => {
            println!("Device SOL balance:   <rpc error: {err}>");
        }
    }
    let create_stake_ix = system_create_account(
        &payer.pubkey(),
        &stake_pubkey,
        stake_rent,
        StakeStateV2::size_of() as u64,
        &stake_program_id(),
    );
    let init_stake_ix = stake_initialize(&stake_pubkey, &authorized, &lockup);
    let setup_tx = submit_transaction_with_extra_signers(
        client,
        port,
        config,
        payer,
        &[create_stake_ix, init_stake_ix],
        &[&stake_account],
    )?;
    println!("Stake create/init tx: {}", setup_tx);
    wait_for_signature_status(
        client,
        &setup_tx,
        CommitmentConfig::finalized(),
        config.action_timeout,
    )
    .context("stake create/init transaction did not reach finalized commitment")?;

    let mut stake_before = wait_for_stake_account_state(
        client,
        &stake_pubkey,
        CommitmentConfig::finalized(),
        config.action_timeout,
        "Initialized with Vector PDA authorities",
        |state| {
            matches!(
                state,
                StakeStateV2::Initialized(meta)
                    if meta.authorized.staker == *vector_pda
                        && meta.authorized.withdrawer == *vector_pda
            )
        },
    )?;
    if device_top_up_amount > 0 {
        let top_up_tx = if payer.pubkey() == device_wallet {
            submit_transaction(
                client,
                port,
                config,
                payer,
                std::slice::from_ref(&device_top_up_ix),
            )?
        } else {
            submit_transaction_with_device_signer(
                client,
                port,
                config,
                payer,
                std::slice::from_ref(&device_top_up_ix),
                &device_wallet,
            )?
        };
        println!("Stake top-up tx:      {}", top_up_tx);
        wait_for_signature_status(
            client,
            &top_up_tx,
            CommitmentConfig::finalized(),
            config.action_timeout,
        )
        .context("stake top-up transaction did not reach finalized commitment")?;
        stake_before = wait_for_stake_account_state(
            client,
            &stake_pubkey,
            CommitmentConfig::finalized(),
            config.action_timeout,
            "Initialized with Vector PDA authorities",
            |state| {
                matches!(
                    state,
                    StakeStateV2::Initialized(meta)
                        if meta.authorized.staker == *vector_pda
                            && meta.authorized.withdrawer == *vector_pda
                )
            },
        )?;
    }
    match &stake_before {
        StakeStateV2::Initialized(meta) => {
            ensure!(
                meta.authorized.staker == *vector_pda,
                "stake staker mismatch after setup: expected {} got {}",
                vector_pda,
                meta.authorized.staker
            );
            ensure!(
                meta.authorized.withdrawer == *vector_pda,
                "stake withdrawer mismatch after setup: expected {} got {}",
                vector_pda,
                meta.authorized.withdrawer
            );
        }
        other => bail!(
            "unexpected stake state after setup: expected Initialized, got {}",
            describe_stake_state(other)
        ),
    }

    let delegate_sub = stake_delegate_stake(&stake_pubkey, vector_pda, &vote_account);
    let passthrough_ix =
        create_passthrough_instruction(config.vector_program_id, vector_identity, &[delegate_sub])?;

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        std::slice::from_ref(&passthrough_ix),
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);
    instructions.push(passthrough_ix);

    let tx_sig = submit_transaction(client, port, config, payer, &instructions)?;
    println!("Stake delegate tx:    {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after stake delegation"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after stake delegation: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );
    ensure!(
        updated.lamports == account.lamports,
        "Vector lamports changed during stake delegation: expected {} got {}",
        account.lamports,
        updated.lamports
    );

    let stake_after = wait_for_stake_account_state(
        client,
        &stake_pubkey,
        config.commitment,
        config.action_timeout,
        "Stake",
        |state| matches!(state, StakeStateV2::Stake(_, _, _)),
    )?;
    match &stake_after {
        StakeStateV2::Stake(meta, stake, _flags) => {
            ensure!(
                meta.authorized.staker == *vector_pda,
                "delegated stake staker mismatch: expected {} got {}",
                vector_pda,
                meta.authorized.staker
            );
            ensure!(
                meta.authorized.withdrawer == *vector_pda,
                "delegated stake withdrawer mismatch: expected {} got {}",
                vector_pda,
                meta.authorized.withdrawer
            );
            ensure!(
                stake.delegation.voter_pubkey == vote_account,
                "delegated vote account mismatch: expected {} got {}",
                vote_account,
                stake.delegation.voter_pubkey
            );
            ensure!(
                stake.delegation.stake >= minimum_delegation,
                "delegated stake {} is below RPC minimum delegation {}",
                stake.delegation.stake,
                minimum_delegation
            );
            println!("Next nonce hex:       {}", hex::encode(updated.nonce));
            println!(
                "Vector lamports:      {} SOL ({})",
                lamports_to_sol(updated.lamports),
                updated.lamports
            );
            println!(
                "Delegated stake:      {} SOL ({})",
                lamports_to_sol(stake.delegation.stake),
                stake.delegation.stake
            );
        }
        other => bail!(
            "unexpected stake state after delegation: expected Stake, got {}",
            describe_stake_state(other)
        ),
    }

    Ok(())
}

fn submit_full_raydium_cpmm_swap(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    device_wallet: Pubkey,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    let input_mint = config
        .swap_input_mint
        .ok_or_else(|| anyhow!("missing --swap-input-mint"))?;
    let output_mint = config
        .swap_output_mint
        .ok_or_else(|| anyhow!("missing --swap-output-mint"))?;
    let amount_in = config
        .swap_amount_in
        .ok_or_else(|| anyhow!("missing --swap-amount-in"))?;
    let minimum_amount_out = config
        .swap_min_out
        .ok_or_else(|| anyhow!("missing --swap-min-out"))?;
    let wrapped_sol_mint =
        Pubkey::from_str(WRAPPED_SOL_MINT).context("failed to parse wrapped SOL mint id")?;
    let unwrap_native_sol = config.swap_unwrap_sol && output_mint == wrapped_sol_mint;
    ensure!(input_mint != output_mint, "swap input/output mint must differ");
    ensure!(amount_in > 0, "--swap-amount-in must be greater than zero");
    ensure!(
        !config.swap_unwrap_sol || output_mint == wrapped_sol_mint,
        "--swap-unwrap-sol is currently only supported when --swap-output-mint is WSOL ({})",
        wrapped_sol_mint
    );

    let receiver_owner = config.swap_receiver_owner.unwrap_or(device_wallet);
    let pool = resolve_raydium_cpmm_pool(config, input_mint, output_mint)?;
    let pool_direction = describe_raydium_pool_direction(&pool, input_mint, output_mint)?;
    let input_token_program = pool_direction.input_token_program;
    let output_token_program = pool_direction.output_token_program;
    let input_decimals = pool_direction.input_decimals;
    let output_decimals = pool_direction.output_decimals;

    let vector_input_ata =
        get_associated_token_address_with_program_id(vector_pda, &input_mint, &input_token_program);
    let device_input_ata = get_associated_token_address_with_program_id(
        &device_wallet,
        &input_mint,
        &input_token_program,
    );
    let receiver_output_ata = if unwrap_native_sol {
        get_associated_token_address_with_program_id(vector_pda, &output_mint, &output_token_program)
    } else {
        get_associated_token_address_with_program_id(
            &receiver_owner,
            &output_mint,
            &output_token_program,
        )
    };

    println!();
    println!("Raydium CPMM swap");
    println!(
        "Pool:                 {}",
        pool.pool_id
    );
    println!("Pool program:         {}", pool.program_id);
    if let Some(lookup_table_account) = pool.lookup_table_account {
        println!("Pool LUT:             {}", lookup_table_account);
    }
    if let Some(tvl) = pool.tvl {
        println!("Pool TVL:             ${:.2}", tvl);
    }
    if let Some(fee_rate) = pool.fee_rate {
        println!("Pool fee rate:        {:.6}", fee_rate);
    }
    println!("Device wallet:        {}", device_wallet);
    println!("Relayer / payer:      {}", payer.pubkey());
    println!("Input mint:           {}", input_mint);
    println!("Output mint:          {}", output_mint);
    if let Some(symbol) = pool_direction.input_symbol {
        println!("Input symbol:         {}", symbol);
    }
    if let Some(symbol) = pool_direction.output_symbol {
        println!("Output symbol:        {}", symbol);
    }
    println!("Vector input ATA:     {}", vector_input_ata);
    println!("Device input ATA:     {}", device_input_ata);
    println!("Receiver output ATA:  {}", receiver_output_ata);
    if unwrap_native_sol {
        println!("Output owner:         {}", vector_pda);
        println!("Native SOL receiver:  {}", receiver_owner);
    } else {
        println!("Output owner:         {}", receiver_owner);
    }
    println!(
        "Amount in:            {} raw units ({})",
        amount_in,
        format_token_amount(amount_in, input_decimals)
    );
    println!(
        "Minimum out:          {} raw units ({})",
        minimum_amount_out,
        format_token_amount(minimum_amount_out, output_decimals)
    );

    let mut setup_instructions = Vec::new();
    if !account_exists(client, &vector_input_ata, config.commitment)? {
        setup_instructions.push(create_associated_token_account_idempotent(
            &payer.pubkey(),
            vector_pda,
            &input_mint,
            &input_token_program,
        ));
    }
    if !account_exists(client, &receiver_output_ata, config.commitment)? {
        setup_instructions.push(create_associated_token_account_idempotent(
            &payer.pubkey(),
            if unwrap_native_sol { vector_pda } else { &receiver_owner },
            &output_mint,
            &output_token_program,
        ));
    }
    if !account_exists(client, &device_input_ata, config.commitment)? {
        setup_instructions.push(create_associated_token_account_idempotent(
            &payer.pubkey(),
            &device_wallet,
            &input_mint,
            &input_token_program,
        ));
    }
    if setup_instructions.is_empty() {
        println!("ATA setup:            already ready");
    } else {
        let setup_tx = submit_transaction(client, port, config, payer, &setup_instructions)?;
        println!("ATA setup tx:         {}", setup_tx);
    }

    let mut vector_input_before = fetch_token_account_state_for_program(
        client,
        &vector_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let device_input_before = fetch_token_account_state_for_program(
        client,
        &device_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let mut receiver_output_before = fetch_token_account_state_for_program(
        client,
        &receiver_output_ata,
        output_token_program,
        config.commitment,
    )?;
    let receiver_output_before_lamports =
        fetch_account_lamports(client, &receiver_output_ata, config.commitment)?;
    let receiver_native_before = if unwrap_native_sol {
        Some(fetch_account_lamports(
            client,
            &receiver_owner,
            config.commitment,
        )?)
    } else {
        None
    };

    ensure!(
        vector_input_before.owner == *vector_pda,
        "Vector input ATA owner mismatch: expected {} got {}",
        vector_pda,
        vector_input_before.owner
    );
    ensure!(
        vector_input_before.mint == input_mint,
        "Vector input ATA mint mismatch: expected {} got {}",
        input_mint,
        vector_input_before.mint
    );
    ensure!(
        device_input_before.owner == device_wallet,
        "device input ATA owner mismatch: expected {} got {}",
        device_wallet,
        device_input_before.owner
    );
    ensure!(
        device_input_before.mint == input_mint,
        "device input ATA mint mismatch: expected {} got {}",
        input_mint,
        device_input_before.mint
    );
    let expected_output_owner = if unwrap_native_sol {
        *vector_pda
    } else {
        receiver_owner
    };
    ensure!(
        receiver_output_before.owner == expected_output_owner,
        "receiver output ATA owner mismatch: expected {} got {}",
        expected_output_owner,
        receiver_output_before.owner
    );
    ensure!(
        receiver_output_before.mint == output_mint,
        "receiver output ATA mint mismatch: expected {} got {}",
        output_mint,
        receiver_output_before.mint
    );
    if unwrap_native_sol {
        ensure!(
            receiver_output_before.amount == 0,
            "refusing to unwrap from {} because it already holds {} raw units ({}); drain or use a fresh WSOL account first",
            receiver_output_ata,
            receiver_output_before.amount,
            format_token_amount(receiver_output_before.amount, output_decimals)
        );
    }

    if vector_input_before.amount < amount_in {
        let top_up_amount = amount_in - vector_input_before.amount;
        println!(
            "Bootstrap shortfall:   {} raw units ({})",
            top_up_amount,
            format_token_amount(top_up_amount, input_decimals)
        );
        println!(
            "Device input balance:  {} raw units ({})",
            device_input_before.amount,
            format_token_amount(device_input_before.amount, input_decimals)
        );

        if device_input_before.amount < top_up_amount {
            let device_sol_balance = client.get_balance(&device_wallet).ok();
            let relayer_sol_balance = client.get_balance(&payer.pubkey()).ok();
            bail!(
                "Vector input ATA {} holds {} raw units ({}) of mint {} and needs {} more raw units ({}). Device wallet {} ATA {} holds {} raw units ({}). Fund the hardware wallet with this mint.{}{}",
                vector_input_ata,
                vector_input_before.amount,
                format_token_amount(vector_input_before.amount, input_decimals),
                input_mint,
                top_up_amount,
                format_token_amount(top_up_amount, input_decimals),
                device_wallet,
                device_input_ata,
                device_input_before.amount,
                format_token_amount(device_input_before.amount, input_decimals),
                match device_sol_balance {
                    Some(balance) => format!(
                        " Device SOL balance: {} SOL / {} lamports.",
                        lamports_to_sol(balance),
                        balance
                    ),
                    None => String::new(),
                },
                match relayer_sol_balance {
                    Some(balance) => format!(
                        " Relayer {} SOL balance: {} SOL / {} lamports.",
                        payer.pubkey(),
                        lamports_to_sol(balance),
                        balance
                    ),
                    None => String::new(),
                }
            );
        }

        let bootstrap_ix = spl_transfer(
            &input_token_program,
            &device_input_ata,
            &vector_input_ata,
            &device_wallet,
            &[],
            top_up_amount,
        )
        .map_err(|err| anyhow!("failed to build Raydium bootstrap transfer: {err}"))?;
        let bootstrap_tx = if payer.pubkey() == device_wallet {
            submit_transaction_with_device_payer(
                client,
                port,
                config,
                &[bootstrap_ix],
                &device_wallet,
            )?
        } else {
            submit_transaction_with_device_signer(
                client,
                port,
                config,
                payer,
                &[bootstrap_ix],
                &device_wallet,
            )?
        };
        println!("Bootstrap tx:         {}", bootstrap_tx);

        vector_input_before = fetch_token_account_state_for_program(
            client,
            &vector_input_ata,
            input_token_program,
            config.commitment,
        )?;
        receiver_output_before = fetch_token_account_state_for_program(
            client,
            &receiver_output_ata,
            output_token_program,
            config.commitment,
        )?;
        println!(
            "Vector input after bootstrap: {} raw units ({})",
            vector_input_before.amount,
            format_token_amount(vector_input_before.amount, input_decimals)
        );
    }

    ensure!(
        vector_input_before.amount >= amount_in,
        "Vector input ATA {} still holds only {} raw units ({}) of mint {} after bootstrap",
        vector_input_ata,
        vector_input_before.amount,
        format_token_amount(vector_input_before.amount, input_decimals),
        input_mint
    );

    println!(
        "Vector input before:  {} raw units ({})",
        vector_input_before.amount,
        format_token_amount(vector_input_before.amount, input_decimals)
    );
    println!(
        "Receiver output before: {} raw units ({})",
        receiver_output_before.amount,
        format_token_amount(receiver_output_before.amount, output_decimals)
    );
    if let Some(receiver_native_before) = receiver_native_before {
        println!(
            "Receiver SOL before:  {} SOL ({})",
            lamports_to_sol(receiver_native_before),
            receiver_native_before
        );
    }

    let swap_sub = create_raydium_cpmm_swap_base_input_instruction(
        &pool,
        *vector_pda,
        vector_input_ata,
        receiver_output_ata,
        input_mint,
        output_mint,
        amount_in,
        minimum_amount_out,
    )?;
    let mut passthrough_subinstructions = vec![swap_sub];
    if unwrap_native_sol {
        passthrough_subinstructions.push(create_spl_close_account_subinstruction(
            output_token_program,
            receiver_output_ata,
            receiver_owner,
            *vector_pda,
        )?);
    }
    let passthrough_ix = create_passthrough_instruction(
        config.vector_program_id,
        vector_identity,
        &passthrough_subinstructions,
    )?;

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        std::slice::from_ref(&passthrough_ix),
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);
    instructions.push(passthrough_ix);

    let tx_sig = if let Some(lookup_table_pubkey) = pool.lookup_table_account {
        let lookup_table_account = fetch_address_lookup_table_account(
            client,
            &lookup_table_pubkey,
            config.commitment,
        )?;
        submit_v0_transaction_with_lookup_tables(
            client,
            config,
            payer,
            &instructions,
            &[lookup_table_account],
            "failed to send and confirm Raydium CPMM swap transaction",
        )?
    } else {
        submit_transaction(client, port, config, payer, &instructions)?
    };
    println!("Swap tx:              {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after Raydium swap"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after Raydium swap: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );
    ensure!(
        updated.lamports == account.lamports,
        "Vector lamports changed during Raydium swap: expected {} got {}",
        account.lamports,
        updated.lamports
    );

    let vector_input_after = fetch_token_account_state_for_program(
        client,
        &vector_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let receiver_output_after = if unwrap_native_sol {
        None
    } else {
        Some(fetch_token_account_state_for_program(
            client,
            &receiver_output_ata,
            output_token_program,
            config.commitment,
        )?)
    };
    let receiver_native_after = if unwrap_native_sol {
        Some(fetch_account_lamports(
            client,
            &receiver_owner,
            config.commitment,
        )?)
    } else {
        None
    };

    ensure!(
        vector_input_after.amount == vector_input_before.amount - amount_in,
        "Vector input ATA amount mismatch after swap: expected {} got {}",
        vector_input_before.amount - amount_in,
        vector_input_after.amount
    );
    let actual_amount_out = if let Some(receiver_output_after) = &receiver_output_after {
        ensure!(
            receiver_output_after.amount >= receiver_output_before.amount + minimum_amount_out,
            "receiver output ATA amount mismatch after swap: expected at least {} got {}",
            receiver_output_before.amount + minimum_amount_out,
            receiver_output_after.amount
        );
        receiver_output_after
            .amount
            .checked_sub(receiver_output_before.amount)
            .ok_or_else(|| anyhow!("receiver output underflow after Raydium swap"))?
    } else {
        ensure!(
            !account_exists(client, &receiver_output_ata, config.commitment)?,
            "expected WSOL output account {} to be closed after unwrap",
            receiver_output_ata
        );
        let receiver_native_before = receiver_native_before
            .ok_or_else(|| anyhow!("missing receiver native balance before Raydium unwrap"))?;
        let receiver_native_after = receiver_native_after
            .ok_or_else(|| anyhow!("missing receiver native balance after Raydium unwrap"))?;
        let receiver_native_delta = receiver_native_after
            .checked_sub(receiver_native_before)
            .ok_or_else(|| anyhow!("receiver SOL underflow after Raydium unwrap"))?;
        ensure!(
            receiver_native_delta >= receiver_output_before_lamports + minimum_amount_out,
            "receiver SOL delta mismatch after unwrap: expected at least {} got {}",
            receiver_output_before_lamports + minimum_amount_out,
            receiver_native_delta
        );
        receiver_native_delta
            .checked_sub(receiver_output_before_lamports)
            .ok_or_else(|| anyhow!("receiver SOL delta smaller than reclaimed WSOL rent"))?
    };

    println!("Next nonce hex:       {}", hex::encode(updated.nonce));
    println!(
        "Vector lamports:      {} SOL ({})",
        lamports_to_sol(updated.lamports),
        updated.lamports
    );
    println!(
        "Vector input after:   {} raw units ({})",
        vector_input_after.amount,
        format_token_amount(vector_input_after.amount, input_decimals)
    );
    if let Some(receiver_output_after) = &receiver_output_after {
        println!(
            "Receiver output after: {} raw units ({})",
            receiver_output_after.amount,
            format_token_amount(receiver_output_after.amount, output_decimals)
        );
    }
    if let Some(receiver_native_after) = receiver_native_after {
        println!(
            "Receiver SOL after:   {} SOL ({})",
            lamports_to_sol(receiver_native_after),
            receiver_native_after
        );
        println!(
            "Recovered WSOL rent:  {} SOL ({})",
            lamports_to_sol(receiver_output_before_lamports),
            receiver_output_before_lamports
        );
    }
    println!(
        "Actual amount out:    {} raw units ({})",
        actual_amount_out,
        format_token_amount(actual_amount_out, output_decimals)
    );

    Ok(())
}

fn submit_full_jupiter_router_swap(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    device_wallet: Pubkey,
    pqc: &PqcState,
    vector_identity: &[u8; 32],
    vector_pda: &Pubkey,
    expected_bump: u8,
    account: VectorAccountState,
) -> Result<()> {
    let api_key = resolve_jupiter_api_key(config)?;
    let input_mint = config
        .swap_input_mint
        .ok_or_else(|| anyhow!("missing --swap-input-mint"))?;
    let output_mint = config
        .swap_output_mint
        .ok_or_else(|| anyhow!("missing --swap-output-mint"))?;
    let amount_in = config
        .swap_amount_in
        .ok_or_else(|| anyhow!("missing --swap-amount-in"))?;
    let wrapped_sol_mint =
        Pubkey::from_str(WRAPPED_SOL_MINT).context("failed to parse wrapped SOL mint id")?;
    let native_sol_output = output_mint == wrapped_sol_mint;
    ensure!(input_mint != output_mint, "swap input/output mint must differ");
    ensure!(amount_in > 0, "--swap-amount-in must be greater than zero");
    ensure!(
        config.swap_min_out.is_none(),
        "--swap-min-out is not used for full-jupiter-router-swap; use --swap-slippage-bps"
    );

    let receiver_owner = config.swap_receiver_owner.unwrap_or(device_wallet);
    let (input_token_program, input_mint_state) =
        fetch_mint_state_with_program(client, &input_mint, config.commitment)?;
    let (output_token_program, output_mint_state) =
        fetch_mint_state_with_program(client, &output_mint, config.commitment)?;
    let input_decimals = input_mint_state.decimals;
    let output_decimals = output_mint_state.decimals;

    let vector_input_ata =
        get_associated_token_address_with_program_id(vector_pda, &input_mint, &input_token_program);
    let device_input_ata = get_associated_token_address_with_program_id(
        &device_wallet,
        &input_mint,
        &input_token_program,
    );
    let receiver_output_ata = (!native_sol_output).then(|| {
        get_associated_token_address_with_program_id(
            &receiver_owner,
            &output_mint,
            &output_token_program,
        )
    });

    println!();
    println!("Jupiter router swap");
    println!("Device wallet:        {}", device_wallet);
    println!("Relayer / payer:      {}", payer.pubkey());
    println!("Input mint:           {}", input_mint);
    println!("Output mint:          {}", output_mint);
    println!("Vector input ATA:     {}", vector_input_ata);
    println!("Device input ATA:     {}", device_input_ata);
    if let Some(receiver_output_ata) = receiver_output_ata {
        println!("Receiver output ATA:  {}", receiver_output_ata);
    } else {
        println!("Receiver native SOL:  {}", receiver_owner);
    }
    println!("Output owner:         {}", receiver_owner);
    println!(
        "Amount in:            {} raw units ({})",
        amount_in,
        format_token_amount(amount_in, input_decimals)
    );
    println!("Slippage bps:         {}", config.swap_slippage_bps);

    let mut setup_instructions = Vec::new();
    if !account_exists(client, &vector_input_ata, config.commitment)? {
        setup_instructions.push(create_associated_token_account_idempotent(
            &payer.pubkey(),
            vector_pda,
            &input_mint,
            &input_token_program,
        ));
    }
    if let Some(receiver_output_ata) = receiver_output_ata {
        if !account_exists(client, &receiver_output_ata, config.commitment)? {
            setup_instructions.push(create_associated_token_account_idempotent(
                &payer.pubkey(),
                &receiver_owner,
                &output_mint,
                &output_token_program,
            ));
        }
    }
    if !account_exists(client, &device_input_ata, config.commitment)? {
        setup_instructions.push(create_associated_token_account_idempotent(
            &payer.pubkey(),
            &device_wallet,
            &input_mint,
            &input_token_program,
        ));
    }
    if setup_instructions.is_empty() {
        println!("ATA setup:            already ready");
    } else {
        let setup_tx = submit_transaction(client, port, config, payer, &setup_instructions)?;
        println!("ATA setup tx:         {}", setup_tx);
    }

    let mut vector_input_before = fetch_token_account_state_for_program(
        client,
        &vector_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let device_input_before = fetch_token_account_state_for_program(
        client,
        &device_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let receiver_output_before = receiver_output_ata
        .map(|receiver_output_ata| {
            fetch_token_account_state_for_program(
                client,
                &receiver_output_ata,
                output_token_program,
                config.commitment,
            )
        })
        .transpose()?;
    let receiver_native_before = if native_sol_output {
        Some(fetch_account_lamports(
            client,
            &receiver_owner,
            config.commitment,
        )?)
    } else {
        None
    };

    ensure!(
        vector_input_before.owner == *vector_pda,
        "Vector input ATA owner mismatch: expected {} got {}",
        vector_pda,
        vector_input_before.owner
    );
    ensure!(
        vector_input_before.mint == input_mint,
        "Vector input ATA mint mismatch: expected {} got {}",
        input_mint,
        vector_input_before.mint
    );
    ensure!(
        device_input_before.owner == device_wallet,
        "device input ATA owner mismatch: expected {} got {}",
        device_wallet,
        device_input_before.owner
    );
    ensure!(
        device_input_before.mint == input_mint,
        "device input ATA mint mismatch: expected {} got {}",
        input_mint,
        device_input_before.mint
    );
    if let Some(receiver_output_before) = &receiver_output_before {
        ensure!(
            receiver_output_before.owner == receiver_owner,
            "receiver output ATA owner mismatch: expected {} got {}",
            receiver_owner,
            receiver_output_before.owner
        );
        ensure!(
            receiver_output_before.mint == output_mint,
            "receiver output ATA mint mismatch: expected {} got {}",
            output_mint,
            receiver_output_before.mint
        );
    }

    if vector_input_before.amount < amount_in {
        let top_up_amount = amount_in - vector_input_before.amount;
        println!(
            "Bootstrap shortfall:   {} raw units ({})",
            top_up_amount,
            format_token_amount(top_up_amount, input_decimals)
        );
        println!(
            "Device input balance:  {} raw units ({})",
            device_input_before.amount,
            format_token_amount(device_input_before.amount, input_decimals)
        );

        if device_input_before.amount < top_up_amount {
            bail!(
                "Vector input ATA {} holds {} raw units ({}) of mint {} and needs {} more raw units ({}). Device wallet {} ATA {} holds only {} raw units ({}). Fund the hardware wallet with this mint and rerun.",
                vector_input_ata,
                vector_input_before.amount,
                format_token_amount(vector_input_before.amount, input_decimals),
                input_mint,
                top_up_amount,
                format_token_amount(top_up_amount, input_decimals),
                device_wallet,
                device_input_ata,
                device_input_before.amount,
                format_token_amount(device_input_before.amount, input_decimals)
            );
        }

        let bootstrap_ix = spl_transfer(
            &input_token_program,
            &device_input_ata,
            &vector_input_ata,
            &device_wallet,
            &[],
            top_up_amount,
        )
        .map_err(|err| anyhow!("failed to build Jupiter bootstrap transfer: {err}"))?;
        let bootstrap_tx = if payer.pubkey() == device_wallet {
            submit_transaction_with_device_payer(
                client,
                port,
                config,
                &[bootstrap_ix],
                &device_wallet,
            )?
        } else {
            submit_transaction_with_device_signer(
                client,
                port,
                config,
                payer,
                &[bootstrap_ix],
                &device_wallet,
            )?
        };
        println!("Bootstrap tx:         {}", bootstrap_tx);

        vector_input_before = fetch_token_account_state_for_program(
            client,
            &vector_input_ata,
            input_token_program,
            config.commitment,
        )?;
        println!(
            "Vector input after bootstrap: {} raw units ({})",
            vector_input_before.amount,
            format_token_amount(vector_input_before.amount, input_decimals)
        );
    }

    ensure!(
        vector_input_before.amount >= amount_in,
        "Vector input ATA {} still holds only {} raw units ({}) of mint {} after bootstrap",
        vector_input_ata,
        vector_input_before.amount,
        format_token_amount(vector_input_before.amount, input_decimals),
        input_mint
    );

    println!(
        "Vector input before:  {} raw units ({})",
        vector_input_before.amount,
        format_token_amount(vector_input_before.amount, input_decimals)
    );
    if let Some(receiver_output_before) = &receiver_output_before {
        println!(
            "Receiver output before: {} raw units ({})",
            receiver_output_before.amount,
            format_token_amount(receiver_output_before.amount, output_decimals)
        );
    }
    if let Some(receiver_native_before) = receiver_native_before {
        println!(
            "Receiver SOL before:  {} SOL ({})",
            lamports_to_sol(receiver_native_before),
            receiver_native_before
        );
    }

    let build = fetch_jupiter_router_build(
        config,
        &api_key,
        *vector_pda,
        payer.pubkey(),
        input_mint,
        output_mint,
        amount_in,
        receiver_output_ata,
        native_sol_output.then_some(receiver_owner),
    )?;

    ensure!(
        build.input_mint == input_mint.to_string(),
        "Jupiter build returned input mint {} instead of {}",
        build.input_mint,
        input_mint
    );
    ensure!(
        build.output_mint == output_mint.to_string(),
        "Jupiter build returned output mint {} instead of {}",
        build.output_mint,
        output_mint
    );
    let quoted_in_amount: u64 = build
        .in_amount
        .parse()
        .context("failed to parse Jupiter inAmount")?;
    let quoted_out_amount: u64 = build
        .out_amount
        .parse()
        .context("failed to parse Jupiter outAmount")?;
    let minimum_amount_out: u64 = build
        .other_amount_threshold
        .parse()
        .context("failed to parse Jupiter otherAmountThreshold")?;
    ensure!(
        quoted_in_amount == amount_in,
        "Jupiter build returned inAmount {} instead of {}",
        quoted_in_amount,
        amount_in
    );

    let route_summary = if build.route_plan.is_empty() {
        "<empty route>".to_string()
    } else {
        build.route_plan
            .iter()
            .map(|leg| {
                format!(
                    "{}:{:.2}% {} {}->{} {}->{}",
                    leg.swap_info.label,
                    leg.percent,
                    leg.swap_info.amm_key,
                    leg.swap_info.input_mint,
                    leg.swap_info.output_mint,
                    leg.swap_info.in_amount,
                    leg.swap_info.out_amount
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    };

    if config.jupiter_direct_only {
        ensure!(
            build.route_plan.len() == 1,
            "Jupiter returned {} route legs despite --jupiter-direct-only",
            build.route_plan.len()
        );
        let direct_leg = &build.route_plan[0];
        ensure!(
            (direct_leg.percent - 100.0).abs() < f64::EPSILON,
            "Jupiter returned a split route despite --jupiter-direct-only: percent={}",
            direct_leg.percent
        );
        ensure!(
            direct_leg.swap_info.input_mint == input_mint.to_string()
                && direct_leg.swap_info.output_mint == output_mint.to_string(),
            "Jupiter returned a multi-hop route despite --jupiter-direct-only: {} -> {}",
            direct_leg.swap_info.input_mint,
            direct_leg.swap_info.output_mint
        );
    }

    println!("Route plan:           {}", route_summary);
    println!("Swap mode:            {}", build.swap_mode);
    println!("Build slippage bps:   {}", build.slippage_bps);
    println!(
        "Quoted out:           {} raw units ({})",
        quoted_out_amount,
        format_token_amount(quoted_out_amount, output_decimals)
    );
    println!(
        "Minimum out:          {} raw units ({})",
        minimum_amount_out,
        format_token_amount(minimum_amount_out, output_decimals)
    );

    let mut pre_instructions = Vec::new();
    if let Some(units) = config.compute_units {
        pre_instructions.push(compute_budget_set_compute_unit_limit(units)?);
        println!("Compute units:        {}", units);
    } else {
        println!("Compute units:        default");
    }
    pre_instructions.extend(
        build
            .compute_budget_instructions
            .iter()
            .map(jupiter_api_instruction_to_instruction)
            .collect::<Result<Vec<_>>>()?,
    );
    pre_instructions.extend(
        build
            .other_instructions
            .iter()
            .map(jupiter_api_instruction_to_instruction)
            .collect::<Result<Vec<_>>>()?,
    );
    pre_instructions.extend(
        build
            .setup_instructions
            .iter()
            .map(jupiter_api_instruction_to_instruction)
            .collect::<Result<Vec<_>>>()?,
    );

    let mut passthrough_subinstructions =
        vec![jupiter_api_instruction_to_instruction(&build.swap_instruction)?];
    if let Some(cleanup_ix) = &build.cleanup_instruction {
        passthrough_subinstructions.push(jupiter_api_instruction_to_instruction(cleanup_ix)?);
    }
    let passthrough_ix = create_passthrough_instruction(
        config.vector_program_id,
        vector_identity,
        &passthrough_subinstructions,
    )?;

    let mut post_instructions = vec![passthrough_ix];
    if let Some(tip_ix) = &build.tip_instruction {
        post_instructions.push(jupiter_api_instruction_to_instruction(tip_ix)?);
    }

    unlock_device(port, &config.pin, config.read_timeout)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        vector_identity,
        &account.nonce,
        &pre_instructions,
        &post_instructions,
        Some(payer.pubkey()),
    )?;

    let response = send_command(
        port,
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
        returned_identity == *vector_identity,
        "device returned a different Vector identity"
    );
    verify_vector_signature(&pqc.wire_pubkey, &digest, &signature)?;

    let advance_ix =
        create_advance_instruction(config.vector_program_id, vector_identity, &signature)?;
    let mut instructions = pre_instructions.clone();
    instructions.push(advance_ix);
    instructions.extend(post_instructions);

    let lookup_table_accounts =
        build_lookup_table_accounts_from_response(build.addresses_by_lookup_table_address)?;
    let tx_sig = submit_v0_transaction_with_lookup_tables(
        client,
        config,
        payer,
        &instructions,
        &lookup_table_accounts,
        "failed to send and confirm Jupiter router swap transaction",
    )?;
    println!("Swap tx:              {}", tx_sig);
    println!("Advance digest hex:   {}", hex::encode(digest));
    println!("Advance digest b64:   {}", STANDARD.encode(digest));

    let updated = fetch_vector_account(client, vector_pda, config.commitment)?
        .ok_or_else(|| anyhow!("Vector account disappeared after Jupiter router swap"))?;
    ensure!(
        updated.bump == expected_bump,
        "advanced Vector bump {} does not match derived bump {}",
        updated.bump,
        expected_bump
    );
    ensure!(
        updated.nonce == digest,
        "Vector nonce mismatch after Jupiter router swap: expected {} got {}",
        hex::encode(digest),
        hex::encode(updated.nonce)
    );
    ensure!(
        updated.lamports == account.lamports,
        "Vector lamports changed during Jupiter router swap: expected {} got {}",
        account.lamports,
        updated.lamports
    );

    let vector_input_after = fetch_token_account_state_for_program(
        client,
        &vector_input_ata,
        input_token_program,
        config.commitment,
    )?;
    let receiver_output_after = receiver_output_ata
        .map(|receiver_output_ata| {
            fetch_token_account_state_for_program(
                client,
                &receiver_output_ata,
                output_token_program,
                config.commitment,
            )
        })
        .transpose()?;
    let receiver_native_after = if native_sol_output {
        Some(fetch_account_lamports(
            client,
            &receiver_owner,
            config.commitment,
        )?)
    } else {
        None
    };

    ensure!(
        vector_input_after.amount == vector_input_before.amount - amount_in,
        "Vector input ATA amount mismatch after swap: expected {} got {}",
        vector_input_before.amount - amount_in,
        vector_input_after.amount
    );
    let actual_amount_out = if let (Some(receiver_output_before), Some(receiver_output_after)) =
        (&receiver_output_before, &receiver_output_after)
    {
        ensure!(
            receiver_output_after.amount >= receiver_output_before.amount + minimum_amount_out,
            "receiver output ATA amount mismatch after swap: expected at least {} got {}",
            receiver_output_before.amount + minimum_amount_out,
            receiver_output_after.amount
        );
        receiver_output_after
            .amount
            .checked_sub(receiver_output_before.amount)
            .ok_or_else(|| anyhow!("receiver output underflow after Jupiter router swap"))?
    } else if let (Some(receiver_native_before), Some(receiver_native_after)) =
        (receiver_native_before, receiver_native_after)
    {
        ensure!(
            receiver_native_after >= receiver_native_before + minimum_amount_out,
            "receiver native SOL balance mismatch after swap: expected at least {} got {}",
            receiver_native_before + minimum_amount_out,
            receiver_native_after
        );
        receiver_native_after
            .checked_sub(receiver_native_before)
            .ok_or_else(|| anyhow!("receiver SOL underflow after Jupiter router swap"))?
    } else {
        bail!("missing Jupiter output account state after swap")
    };

    println!("Next nonce hex:       {}", hex::encode(updated.nonce));
    println!(
        "Vector lamports:      {} SOL ({})",
        lamports_to_sol(updated.lamports),
        updated.lamports
    );
    println!(
        "Vector input after:   {} raw units ({})",
        vector_input_after.amount,
        format_token_amount(vector_input_after.amount, input_decimals)
    );
    if let Some(receiver_output_after) = &receiver_output_after {
        println!(
            "Receiver output after: {} raw units ({})",
            receiver_output_after.amount,
            format_token_amount(receiver_output_after.amount, output_decimals)
        );
    }
    if let Some(receiver_native_after) = receiver_native_after {
        println!(
            "Receiver SOL after:   {} SOL ({})",
            lamports_to_sol(receiver_native_after),
            receiver_native_after
        );
    }
    println!(
        "Actual amount out:    {} raw units ({})",
        actual_amount_out,
        format_token_amount(actual_amount_out, output_decimals)
    );

    Ok(())
}

fn fetch_jupiter_router_build(
    config: &Config,
    api_key: &str,
    taker: Pubkey,
    payer: Pubkey,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount_in: u64,
    destination_token_account: Option<Pubkey>,
    native_destination_account: Option<Pubkey>,
) -> Result<JupiterBuildResponse> {
    let http = HttpClient::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("failed to build Jupiter API client")?;
    let url = format!("{}/swap/v2/build", config.jupiter_api_url.trim_end_matches('/'));

    let mut query = vec![
        ("inputMint".to_string(), input_mint.to_string()),
        ("outputMint".to_string(), output_mint.to_string()),
        ("amount".to_string(), amount_in.to_string()),
        ("taker".to_string(), taker.to_string()),
        ("payer".to_string(), payer.to_string()),
        (
            "slippageBps".to_string(),
            config.swap_slippage_bps.to_string(),
        ),
    ];
    if let Some(destination_token_account) = destination_token_account {
        query.push((
            "destinationTokenAccount".to_string(),
            destination_token_account.to_string(),
        ));
    }
    if let Some(native_destination_account) = native_destination_account {
        query.push((
            "nativeDestinationAccount".to_string(),
            native_destination_account.to_string(),
        ));
        query.push(("wrapAndUnwrapSol".to_string(), "true".to_string()));
    }
    if let Some(max_accounts) = config.swap_max_accounts {
        query.push(("maxAccounts".to_string(), max_accounts.to_string()));
    }
    if let Some(dexes) = &config.jupiter_dexes {
        query.push(("dexes".to_string(), dexes.clone()));
    }
    if let Some(exclude_dexes) = &config.jupiter_exclude_dexes {
        query.push(("excludeDexes".to_string(), exclude_dexes.clone()));
    }
    if let Some(mode) = &config.jupiter_mode {
        query.push(("mode".to_string(), mode.clone()));
    }
    if config.jupiter_direct_only {
        query.push(("onlyDirectRoutes".to_string(), "true".to_string()));
    }

    let response = http
        .get(url)
        .header("x-api-key", api_key)
        .query(&query)
        .send()
        .context("failed to query Jupiter /swap/v2/build")?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read Jupiter /swap/v2/build response body")?;
    if !status.is_success() {
        bail!("Jupiter /swap/v2/build returned {}: {}", status, body);
    }
    serde_json::from_str(&body).context("failed to decode Jupiter /swap/v2/build response")
}

fn jupiter_api_instruction_to_instruction(api_ix: &JupiterApiInstruction) -> Result<Instruction> {
    Ok(Instruction {
        program_id: Pubkey::from_str(&api_ix.program_id)
            .with_context(|| format!("invalid Jupiter instruction program id {}", api_ix.program_id))?,
        accounts: api_ix
            .accounts
            .iter()
            .map(|account| {
                Ok(AccountMeta {
                    pubkey: Pubkey::from_str(&account.pubkey).with_context(|| {
                        format!("invalid Jupiter instruction account {}", account.pubkey)
                    })?,
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        data: STANDARD
            .decode(api_ix.data.as_bytes())
            .context("failed to decode Jupiter instruction data")?,
    })
}

fn build_lookup_table_accounts_from_response(
    addresses_by_lookup_table_address: Option<BTreeMap<String, Vec<String>>>,
) -> Result<Vec<AddressLookupTableAccount>> {
    addresses_by_lookup_table_address
        .unwrap_or_default()
        .into_iter()
        .map(|(lookup_table_key, addresses)| {
            let key = Pubkey::from_str(&lookup_table_key)
                .with_context(|| format!("invalid Jupiter LUT address {}", lookup_table_key))?;
            let addresses = addresses
                .into_iter()
                .map(|address| {
                    Pubkey::from_str(&address)
                        .with_context(|| format!("invalid Jupiter LUT entry {}", address))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(AddressLookupTableAccount {
                key: key.to_bytes().into(),
                addresses: addresses
                    .into_iter()
                    .map(|address| address.to_bytes().into())
                    .collect(),
            })
        })
        .collect()
}

fn submit_transaction(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    instructions: &[Instruction],
) -> Result<Signature> {
    submit_transaction_with_extra_signers(client, port, config, payer, instructions, &[])
}

fn submit_transaction_with_device_payer(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    instructions: &[Instruction],
    device_payer: &Pubkey,
) -> Result<Signature> {
    submit_transaction_with_device_payer_and_extra_signers(
        client,
        port,
        config,
        instructions,
        device_payer,
        &[],
    )
}

fn submit_transaction_with_device_payer_and_extra_signers(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    instructions: &[Instruction],
    device_payer: &Pubkey,
    extra_signers: &[&Keypair],
) -> Result<Signature> {
    let (recent_blockhash, _slot) = client
        .get_latest_blockhash_with_commitment(config.commitment)
        .context("failed to fetch latest blockhash")?;

    let message = Message::new(instructions, Some(device_payer));
    let mut tx = Transaction::new_unsigned(message);
    tx.message.recent_blockhash = recent_blockhash;

    if !extra_signers.is_empty() {
        tx.try_partial_sign(extra_signers, recent_blockhash)
            .context("failed to partially sign transaction with local keypairs")?;
    }
    unlock_device(port, &config.pin, config.read_timeout)?;
    let signature = sign_message_with_se050(port, &tx.message.serialize(), config.action_timeout)?;
    ensure!(
        signature.verify(device_payer.as_ref(), &tx.message.serialize()),
        "device returned an invalid Solana signature for device-payer transaction"
    );
    set_transaction_signature(&mut tx, device_payer, signature)?;

    client
        .send_and_confirm_transaction(&tx)
        .context("failed to send and confirm transaction")
}

fn submit_transaction_with_device_signer(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    instructions: &[Instruction],
    device_signer: &Pubkey,
) -> Result<Signature> {
    let (recent_blockhash, _slot) = client
        .get_latest_blockhash_with_commitment(config.commitment)
        .context("failed to fetch latest blockhash")?;

    let payer_pubkey = payer.pubkey();
    let message = Message::new(instructions, Some(&payer_pubkey));
    let mut tx = Transaction::new_unsigned(message);
    tx.message.recent_blockhash = recent_blockhash;

    match payer {
        FeePayer::Local { keypair, .. } => {
            tx.try_partial_sign(&[&*keypair], recent_blockhash)
                .context("failed to sign transaction with local fee payer")?;
            unlock_device(port, &config.pin, config.read_timeout)?;
            let signature =
                sign_message_with_se050(port, &tx.message.serialize(), config.action_timeout)?;
            ensure!(
                signature.verify(device_signer.as_ref(), &tx.message.serialize()),
                "device returned an invalid Solana signature for bootstrap signer"
            );
            set_transaction_signature(&mut tx, device_signer, signature)?;
        }
        FeePayer::Device { pubkey } => {
            ensure!(
                pubkey == device_signer,
                "device signer {} does not match device fee payer {}",
                device_signer,
                pubkey
            );
            unlock_device(port, &config.pin, config.read_timeout)?;
            let signature =
                sign_message_with_se050(port, &tx.message.serialize(), config.action_timeout)?;
            ensure!(
                signature.verify(pubkey.as_ref(), &tx.message.serialize()),
                "device returned an invalid outer Solana signature"
            );
            set_transaction_signature(&mut tx, pubkey, signature)?;
        }
    }

    client
        .send_and_confirm_transaction(&tx)
        .context("failed to send and confirm transaction")
}

fn submit_transaction_with_extra_signers(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    instructions: &[Instruction],
    extra_signers: &[&Keypair],
) -> Result<Signature> {
    submit_transaction_with_extra_signers_and_config(
        client,
        port,
        config,
        payer,
        instructions,
        extra_signers,
        None,
        "failed to send and confirm transaction",
    )
}

fn submit_transaction_with_extra_signers_and_config(
    client: &RpcClient,
    port: &mut dyn SerialPort,
    config: &Config,
    payer: &mut FeePayer,
    instructions: &[Instruction],
    extra_signers: &[&Keypair],
    send_config: Option<RpcSendTransactionConfig>,
    error_context: &str,
) -> Result<Signature> {
    let (recent_blockhash, _slot) = client
        .get_latest_blockhash_with_commitment(config.commitment)
        .context("failed to fetch latest blockhash")?;

    let payer_pubkey = payer.pubkey();
    let message = Message::new(instructions, Some(&payer_pubkey));
    let mut tx = Transaction::new_unsigned(message);
    tx.message.recent_blockhash = recent_blockhash;

    match payer {
        FeePayer::Local { keypair, .. } => {
            let mut signers: Vec<&Keypair> = Vec::with_capacity(1 + extra_signers.len());
            signers.push(&*keypair);
            signers.extend_from_slice(extra_signers);
            tx.try_sign(&signers, recent_blockhash)
                .context("failed to sign transaction with local keypairs")?;
        }
        FeePayer::Device { pubkey } => {
            if !extra_signers.is_empty() {
                tx.try_partial_sign(extra_signers, recent_blockhash)
                    .context("failed to partially sign transaction with local keypairs")?;
            }
            unlock_device(port, &config.pin, config.read_timeout)?;
            let signature =
                sign_message_with_se050(port, &tx.message.serialize(), config.action_timeout)?;
            ensure!(
                signature.verify(pubkey.as_ref(), &tx.message.serialize()),
                "device returned an invalid outer Solana signature"
            );
            set_transaction_signature(&mut tx, pubkey, signature)?;
        }
    }

    if let Some(send_config) = send_config {
        send_and_confirm_transaction_with_config(client, config, &tx, send_config, error_context)
    } else {
        client
            .send_and_confirm_transaction(&tx)
            .with_context(|| error_context.to_string())
    }
}

fn submit_v0_transaction_with_lookup_tables(
    client: &RpcClient,
    config: &Config,
    payer: &mut FeePayer,
    instructions: &[Instruction],
    lookup_table_accounts: &[AddressLookupTableAccount],
    error_context: &str,
) -> Result<Signature> {
    let (recent_blockhash, _slot) = client
        .get_latest_blockhash_with_commitment(config.commitment)
        .context("failed to fetch latest blockhash")?;

    let payer_pubkey = payer.pubkey();
    let message = v0::Message::try_compile(
        &payer_pubkey,
        instructions,
        lookup_table_accounts,
        recent_blockhash,
    )
    .context("failed to compile V0 transaction message")?;
    let versioned_message = VersionedMessage::V0(message);

    match payer {
        FeePayer::Local { keypair, .. } => {
            let signers = [&*keypair];
            let tx = VersionedTransaction::try_new(versioned_message, &signers)
                .context("failed to sign V0 transaction with local fee payer")?;
            client
                .send_and_confirm_transaction(&tx)
                .with_context(|| error_context.to_string())
        }
        FeePayer::Device { .. } => bail!(
            "V0 Raydium swap submission currently requires a local fee payer; device fee payer versioned-message signing is not wired yet"
        ),
    }
}

fn send_and_confirm_transaction_with_config(
    client: &RpcClient,
    config: &Config,
    tx: &impl SerializableTransaction,
    send_config: RpcSendTransactionConfig,
    error_context: &str,
) -> Result<Signature> {
    let signature = client
        .send_transaction_with_config(tx, send_config)
        .with_context(|| error_context.to_string())?;
    wait_for_signature_status(client, &signature, config.commitment, config.action_timeout)
        .with_context(|| format!("{error_context}: {}", signature))?;
    Ok(signature)
}

fn wait_for_signature_status(
    client: &RpcClient,
    signature: &Signature,
    commitment: CommitmentConfig,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last_pending_detail: Option<String> = None;

    loop {
        let statuses = client
            .get_signature_statuses(&[*signature])
            .with_context(|| format!("failed to fetch signature status for {signature}"))?;
        if let Some(status) = statuses.value.into_iter().next().flatten() {
            if let Some(err) = status.err {
                bail!("transaction {} failed on chain: {:?}", signature, err);
            }
            if client
                .confirm_transaction_with_commitment(signature, commitment)
                .with_context(|| format!("failed to confirm transaction {signature}"))?
                .value
            {
                return Ok(());
            }

            last_pending_detail = Some(format!(
                "transaction {} is processed but not yet {:?} confirmed",
                signature, commitment.commitment
            ));
        }

        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(500));
    }

    Err(anyhow!(
        "{}",
        last_pending_detail.unwrap_or_else(|| {
            format!(
                "timed out waiting for transaction {} to reach {:?}",
                signature, commitment.commitment
            )
        })
    ))
}

fn set_transaction_signature(
    tx: &mut Transaction,
    signer: &Pubkey,
    signature: Signature,
) -> Result<()> {
    let signer_count = tx.message.header.num_required_signatures as usize;
    ensure!(
        tx.message.account_keys.len() >= signer_count,
        "message has fewer account keys than required signers"
    );
    let position = tx
        .message
        .account_keys
        .iter()
        .take(signer_count)
        .position(|key| key == signer)
        .ok_or_else(|| anyhow!("transaction does not require a signature from {signer}"))?;
    ensure!(
        tx.signatures.len() > position,
        "transaction unexpectedly has only {} signature slots for signer index {}",
        tx.signatures.len(),
        position
    );
    tx.signatures[position] = signature;
    Ok(())
}

fn sign_message_with_se050(
    port: &mut dyn SerialPort,
    message_bytes: &[u8],
    timeout: Duration,
) -> Result<Signature> {
    let response = send_command(
        port,
        &format!("SIGN:{}", STANDARD.encode(message_bytes)),
        timeout,
    )?;
    let signature_b64 = prefixed_value(&response, "SIGNATURE:")?;
    let signature_bytes = STANDARD
        .decode(signature_b64)
        .context("failed to decode SIGNATURE")?;
    Signature::try_from(signature_bytes.as_slice()).context("failed to parse Ed25519 signature")
}

fn fetch_vector_account(
    client: &RpcClient,
    vector_pda: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<Option<VectorAccountState>> {
    let response = client
        .get_account_with_commitment(vector_pda, commitment)
        .with_context(|| format!("failed to fetch Vector account {vector_pda}"))?;
    let Some(account) = response.value else {
        return Ok(None);
    };

    ensure!(
        account.data.len() >= 33,
        "Vector account data too short: {} bytes",
        account.data.len()
    );

    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&account.data[..32]);
    Ok(Some(VectorAccountState {
        nonce,
        bump: account.data[32],
        lamports: account.lamports,
        data_len: account.data.len(),
    }))
}

fn account_exists(
    client: &RpcClient,
    account: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<bool> {
    let response = client
        .get_account_with_commitment(account, commitment)
        .with_context(|| format!("failed to fetch account {account}"))?;
    Ok(response.value.is_some())
}

fn fetch_account_lamports(
    client: &RpcClient,
    account: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<u64> {
    let response = client
        .get_account_with_commitment(account, commitment)
        .with_context(|| format!("failed to fetch account {account}"))?;
    Ok(response.value.map(|entry| entry.lamports).unwrap_or(0))
}

fn fetch_spl_mint_state(
    client: &RpcClient,
    mint: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<SplTokenMint> {
    let response = client
        .get_account_with_commitment(mint, commitment)
        .with_context(|| format!("failed to fetch mint account {mint}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("mint account {mint} is missing"))?;
    ensure!(
        account.owner == spl_token_program_id(),
        "mint account {} is owned by {} instead of {}",
        mint,
        account.owner,
        spl_token_program_id()
    );
    SplTokenMint::unpack(&account.data).context("failed to unpack mint account")
}

fn fetch_mint_state_with_program(
    client: &RpcClient,
    mint: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<(Pubkey, SplTokenMint)> {
    let response = client
        .get_account_with_commitment(mint, commitment)
        .with_context(|| format!("failed to fetch mint account {mint}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("mint account {mint} is missing"))?;
    let mint_state = SplTokenMint::unpack(&account.data).context("failed to unpack mint account")?;
    Ok((account.owner, mint_state))
}

fn fetch_spl_token_account_state(
    client: &RpcClient,
    account_pubkey: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<SplTokenAccount> {
    let response = client
        .get_account_with_commitment(account_pubkey, commitment)
        .with_context(|| format!("failed to fetch token account {account_pubkey}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("token account {account_pubkey} is missing"))?;
    ensure!(
        account.owner == spl_token_program_id(),
        "token account {} is owned by {} instead of {}",
        account_pubkey,
        account.owner,
        spl_token_program_id()
    );
    SplTokenAccount::unpack(&account.data).context("failed to unpack token account")
}

fn fetch_token_account_state_for_program(
    client: &RpcClient,
    account_pubkey: &Pubkey,
    token_program_id: Pubkey,
    commitment: CommitmentConfig,
) -> Result<SplTokenAccount> {
    let response = client
        .get_account_with_commitment(account_pubkey, commitment)
        .with_context(|| format!("failed to fetch token account {account_pubkey}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("token account {account_pubkey} is missing"))?;
    ensure!(
        account.owner == token_program_id,
        "token account {} is owned by {} instead of {}",
        account_pubkey,
        account.owner,
        token_program_id
    );
    SplTokenAccount::unpack(&account.data).context("failed to unpack token account")
}

fn fetch_address_lookup_table_account(
    client: &RpcClient,
    lookup_table_pubkey: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<AddressLookupTableAccount> {
    let response = client
        .get_account_with_commitment(lookup_table_pubkey, commitment)
        .with_context(|| format!("failed to fetch lookup table account {lookup_table_pubkey}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("lookup table account {lookup_table_pubkey} is missing"))?;
    let lookup_table = AddressLookupTable::deserialize(&account.data)
        .map_err(|err| anyhow!("failed to decode lookup table {lookup_table_pubkey}: {err:?}"))?;

    Ok(AddressLookupTableAccount {
        key: lookup_table_pubkey.to_bytes().into(),
        addresses: lookup_table
            .addresses
            .iter()
            .map(|address| address.to_bytes().into())
            .collect(),
    })
}

fn resolve_raydium_cpmm_pool(
    config: &Config,
    input_mint: Pubkey,
    output_mint: Pubkey,
) -> Result<ResolvedRaydiumCpmmPool> {
    let http = HttpClient::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build Raydium API client")?;
    let cpmm_program_id =
        Pubkey::from_str(RAYDIUM_CPMM_PROGRAM_ID).context("invalid Raydium CPMM program id")?;

    let selected = if let Some(pool_id) = config.raydium_cpmm_pool_id {
        let keys = fetch_raydium_pool_keys(&http, &config.raydium_api_url, &pool_id)?;
        let program_id = Pubkey::from_str(&keys.program_id)
            .context("invalid Raydium pool program id in key response")?;
        ensure!(
            program_id == cpmm_program_id,
            "Raydium pool {} is not a CPMM pool; expected program {} got {}",
            pool_id,
            cpmm_program_id,
            program_id
        );
        ensure!(
            raydium_mints_match(&keys.mint_a, &keys.mint_b, input_mint, output_mint),
            "Raydium pool {} does not match requested mints {} -> {}",
            pool_id,
            input_mint,
            output_mint
        );
        build_resolved_raydium_cpmm_pool(keys, None, None)
    } else {
        let search = search_raydium_cpmm_pools(&http, &config.raydium_api_url, input_mint, output_mint)?;
        let chosen = search
            .iter()
            .filter(|pool| pool.program_id == RAYDIUM_CPMM_PROGRAM_ID)
            .max_by(|left, right| {
                let left_tvl = left.tvl.unwrap_or(0.0);
                let right_tvl = right.tvl.unwrap_or(0.0);
                left_tvl
                    .partial_cmp(&right_tvl)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "Raydium API returned no CPMM pools for mints {} and {}",
                    input_mint,
                    output_mint
                )
            })?;
        let keys = fetch_raydium_pool_keys(
            &http,
            &config.raydium_api_url,
            &Pubkey::from_str(&chosen.id).context("invalid Raydium pool id from search response")?,
        )?;
        build_resolved_raydium_cpmm_pool(keys, chosen.tvl, chosen.fee_rate)
    }?;

    Ok(selected)
}

fn search_raydium_cpmm_pools(
    http: &HttpClient,
    api_base_url: &str,
    input_mint: Pubkey,
    output_mint: Pubkey,
) -> Result<Vec<RaydiumPoolSearchItem>> {
    let url = format!(
        "{}/pools/info/mint?mint1={}&mint2={}&poolType=standard&poolSortField=liquidity&sortType=desc&pageSize=20&page=1",
        api_base_url.trim_end_matches('/'),
        input_mint,
        output_mint
    );
    let response = http
        .get(url)
        .send()
        .context("failed to query Raydium pool search endpoint")?
        .error_for_status()
        .context("Raydium pool search returned an error status")?;
    let envelope: RaydiumPoolSearchEnvelope = response
        .json()
        .context("failed to decode Raydium pool search response")?;
    Ok(envelope
        .data
        .data
        .into_iter()
        .filter(|pool| raydium_mints_match(&pool.mint_a, &pool.mint_b, input_mint, output_mint))
        .collect())
}

fn fetch_raydium_pool_keys(
    http: &HttpClient,
    api_base_url: &str,
    pool_id: &Pubkey,
) -> Result<RaydiumPoolKeysItem> {
    let url = format!(
        "{}/pools/key/ids?ids={}",
        api_base_url.trim_end_matches('/'),
        pool_id
    );
    let response = http
        .get(url)
        .send()
        .context("failed to query Raydium pool key endpoint")?
        .error_for_status()
        .context("Raydium pool key endpoint returned an error status")?;
    let envelope: RaydiumPoolKeysEnvelope = response
        .json()
        .context("failed to decode Raydium pool key response")?;
    envelope
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Raydium key endpoint returned no pool data for {}", pool_id))
}

fn build_resolved_raydium_cpmm_pool(
    keys: RaydiumPoolKeysItem,
    tvl: Option<f64>,
    fee_rate: Option<f64>,
) -> Result<ResolvedRaydiumCpmmPool> {
    let lookup_table_account = keys
        .lookup_table_account
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map(|raw| Pubkey::from_str(raw).context("invalid Raydium lookup table account"))
        .transpose()?;

    Ok(ResolvedRaydiumCpmmPool {
        program_id: Pubkey::from_str(&keys.program_id)
            .context("invalid Raydium pool program id")?,
        pool_id: Pubkey::from_str(&keys.id).context("invalid Raydium pool id")?,
        lookup_table_account,
        authority: Pubkey::from_str(&keys.authority)
            .context("invalid Raydium pool authority")?,
        amm_config: Pubkey::from_str(&keys.config.id)
            .context("invalid Raydium amm config id")?,
        mint_a: Pubkey::from_str(&keys.mint_a.address).context("invalid Raydium mintA")?,
        mint_a_program: Pubkey::from_str(&keys.mint_a.program_id)
            .context("invalid Raydium mintA token program")?,
        mint_a_decimals: keys.mint_a.decimals,
        mint_a_symbol: keys.mint_a.symbol,
        mint_b: Pubkey::from_str(&keys.mint_b.address).context("invalid Raydium mintB")?,
        mint_b_program: Pubkey::from_str(&keys.mint_b.program_id)
            .context("invalid Raydium mintB token program")?,
        mint_b_decimals: keys.mint_b.decimals,
        mint_b_symbol: keys.mint_b.symbol,
        vault_a: Pubkey::from_str(&keys.vault.a).context("invalid Raydium vault A")?,
        vault_b: Pubkey::from_str(&keys.vault.b).context("invalid Raydium vault B")?,
        observation_state: Pubkey::from_str(&keys.observation_id)
            .context("invalid Raydium observation id")?,
        tvl,
        fee_rate,
    })
}

fn raydium_mints_match(
    mint_a: &RaydiumMintRef,
    mint_b: &RaydiumMintRef,
    input_mint: Pubkey,
    output_mint: Pubkey,
) -> bool {
    let Ok(mint_a_pubkey) = Pubkey::from_str(&mint_a.address) else {
        return false;
    };
    let Ok(mint_b_pubkey) = Pubkey::from_str(&mint_b.address) else {
        return false;
    };
    (mint_a_pubkey == input_mint && mint_b_pubkey == output_mint)
        || (mint_a_pubkey == output_mint && mint_b_pubkey == input_mint)
}

struct RaydiumPoolDirection<'a> {
    input_token_program: Pubkey,
    output_token_program: Pubkey,
    input_decimals: u8,
    output_decimals: u8,
    input_symbol: Option<&'a str>,
    output_symbol: Option<&'a str>,
}

fn describe_raydium_pool_direction<'a>(
    pool: &'a ResolvedRaydiumCpmmPool,
    input_mint: Pubkey,
    output_mint: Pubkey,
) -> Result<RaydiumPoolDirection<'a>> {
    if input_mint == pool.mint_a && output_mint == pool.mint_b {
        return Ok(RaydiumPoolDirection {
            input_token_program: pool.mint_a_program,
            output_token_program: pool.mint_b_program,
            input_decimals: pool.mint_a_decimals,
            output_decimals: pool.mint_b_decimals,
            input_symbol: pool.mint_a_symbol.as_deref(),
            output_symbol: pool.mint_b_symbol.as_deref(),
        });
    }
    if input_mint == pool.mint_b && output_mint == pool.mint_a {
        return Ok(RaydiumPoolDirection {
            input_token_program: pool.mint_b_program,
            output_token_program: pool.mint_a_program,
            input_decimals: pool.mint_b_decimals,
            output_decimals: pool.mint_a_decimals,
            input_symbol: pool.mint_b_symbol.as_deref(),
            output_symbol: pool.mint_a_symbol.as_deref(),
        });
    }
    bail!(
        "Raydium pool {} does not support requested direction {} -> {}",
        pool.pool_id,
        input_mint,
        output_mint
    )
}

fn create_raydium_cpmm_swap_base_input_instruction(
    pool: &ResolvedRaydiumCpmmPool,
    payer: Pubkey,
    input_token_account: Pubkey,
    output_token_account: Pubkey,
    input_mint: Pubkey,
    output_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Result<Instruction> {
    let (input_vault, output_vault, input_token_program, output_token_program) =
        if input_mint == pool.mint_a && output_mint == pool.mint_b {
            (
                pool.vault_a,
                pool.vault_b,
                pool.mint_a_program,
                pool.mint_b_program,
            )
        } else if input_mint == pool.mint_b && output_mint == pool.mint_a {
            (
                pool.vault_b,
                pool.vault_a,
                pool.mint_b_program,
                pool.mint_a_program,
            )
        } else {
            bail!(
                "Raydium pool {} does not match requested swap {} -> {}",
                pool.pool_id,
                input_mint,
                output_mint
            );
        };

    let mut data = Vec::with_capacity(8 + 8 + 8);
    data.extend_from_slice(&anchor_instruction_discriminator("swap_base_input"));
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    Ok(Instruction {
        program_id: pool.program_id,
        accounts: vec![
            AccountMeta::new_readonly(payer, true),
            AccountMeta::new_readonly(pool.authority, false),
            AccountMeta::new_readonly(pool.amm_config, false),
            AccountMeta::new(pool.pool_id, false),
            AccountMeta::new(input_token_account, false),
            AccountMeta::new(output_token_account, false),
            AccountMeta::new(input_vault, false),
            AccountMeta::new(output_vault, false),
            AccountMeta::new_readonly(input_token_program, false),
            AccountMeta::new_readonly(output_token_program, false),
            AccountMeta::new_readonly(input_mint, false),
            AccountMeta::new_readonly(output_mint, false),
            AccountMeta::new(pool.observation_state, false),
        ],
        data,
    })
}

fn anchor_instruction_discriminator(name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"global:");
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    out
}

fn fetch_stake_account_state(
    client: &RpcClient,
    account_pubkey: &Pubkey,
    commitment: CommitmentConfig,
) -> Result<StakeStateV2> {
    let response = client
        .get_account_with_commitment(account_pubkey, commitment)
        .with_context(|| format!("failed to fetch stake account {account_pubkey}"))?;
    let account = response
        .value
        .ok_or_else(|| anyhow!("stake account {account_pubkey} is missing"))?;
    ensure!(
        account.owner == stake_program_id(),
        "stake account {} is owned by {} instead of {}",
        account_pubkey,
        account.owner,
        stake_program_id()
    );
    parse_stake_state(&account.data).with_context(|| {
        format!(
            "failed to decode stake account ({} bytes)",
            account.data.len()
        )
    })
}

fn parse_stake_state(data: &[u8]) -> Result<StakeStateV2> {
    let mut offset = 0usize;
    let variant = take_u32(data, &mut offset)?;
    match variant {
        0 => Ok(StakeStateV2::Uninitialized),
        1 => {
            let meta = parse_stake_meta(data, &mut offset)?;
            Ok(StakeStateV2::Initialized(meta))
        }
        2 => {
            let meta = parse_stake_meta(data, &mut offset)?;
            let delegation = Delegation {
                voter_pubkey: take_pubkey(data, &mut offset)?,
                stake: take_u64(data, &mut offset)?,
                activation_epoch: take_u64(data, &mut offset)?,
                deactivation_epoch: take_u64(data, &mut offset)?,
                #[allow(deprecated)]
                warmup_cooldown_rate: take_f64(data, &mut offset)?,
            };
            let stake = Stake {
                delegation,
                credits_observed: take_u64(data, &mut offset)?,
            };
            let _flags_bits = take_u8(data, &mut offset)?;
            Ok(StakeStateV2::Stake(meta, stake, StakeFlags::empty()))
        }
        3 => Ok(StakeStateV2::RewardsPool),
        other => bail!("unsupported stake state discriminator {other}"),
    }
}

fn parse_stake_meta(data: &[u8], offset: &mut usize) -> Result<Meta> {
    Ok(Meta {
        rent_exempt_reserve: take_u64(data, offset)?,
        authorized: StakeAuthorized {
            staker: take_pubkey(data, offset)?,
            withdrawer: take_pubkey(data, offset)?,
        },
        lockup: StakeLockup {
            unix_timestamp: take_i64(data, offset)?,
            epoch: take_u64(data, offset)?,
            custodian: take_pubkey(data, offset)?,
        },
    })
}

fn take_pubkey(data: &[u8], offset: &mut usize) -> Result<Pubkey> {
    let bytes = take_bytes(data, offset, 32)?;
    let mut array = [0u8; 32];
    array.copy_from_slice(bytes);
    Ok(Pubkey::from(array))
}

fn take_u8(data: &[u8], offset: &mut usize) -> Result<u8> {
    Ok(take_bytes(data, offset, 1)?[0])
}

fn take_u32(data: &[u8], offset: &mut usize) -> Result<u32> {
    let bytes = take_bytes(data, offset, 4)?;
    let mut array = [0u8; 4];
    array.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(array))
}

fn take_u64(data: &[u8], offset: &mut usize) -> Result<u64> {
    let bytes = take_bytes(data, offset, 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(array))
}

fn take_i64(data: &[u8], offset: &mut usize) -> Result<i64> {
    let bytes = take_bytes(data, offset, 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Ok(i64::from_le_bytes(array))
}

fn take_f64(data: &[u8], offset: &mut usize) -> Result<f64> {
    let bytes = take_bytes(data, offset, 8)?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Ok(f64::from_le_bytes(array))
}

fn take_bytes<'a>(data: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| anyhow!("stake account offset overflowed"))?;
    ensure!(
        end <= data.len(),
        "stake account too short: need {} bytes at offset {}, have {} total bytes",
        len,
        offset,
        data.len()
    );
    let bytes = &data[*offset..end];
    *offset = end;
    Ok(bytes)
}

fn wait_for_stake_account_state(
    client: &RpcClient,
    account_pubkey: &Pubkey,
    commitment: CommitmentConfig,
    timeout: Duration,
    expected_label: &str,
    predicate: impl Fn(&StakeStateV2) -> bool,
) -> Result<StakeStateV2> {
    let deadline = Instant::now() + timeout;

    loop {
        let observation = match fetch_stake_account_state(client, account_pubkey, commitment) {
            Ok(state) => {
                if predicate(&state) {
                    return Ok(state);
                }
                format!(
                    "stake account {} is {} but expected {}",
                    account_pubkey,
                    describe_stake_state(&state),
                    expected_label
                )
            }
            Err(err) => err.to_string(),
        };

        if Instant::now() >= deadline {
            return Err(anyhow!(observation));
        }
        sleep(Duration::from_millis(500));
    }
}

fn choose_vote_account(client: &RpcClient) -> Result<Pubkey> {
    let vote_accounts = client
        .get_vote_accounts()
        .context("failed to fetch vote accounts")?;
    let entry = vote_accounts
        .current
        .first()
        .or_else(|| vote_accounts.delinquent.first())
        .ok_or_else(|| anyhow!("RPC returned no vote accounts"))?;
    Pubkey::from_str(&entry.vote_pubkey).context("invalid vote account pubkey from RPC")
}

fn validate_vote_account(client: &RpcClient, vote_account: Pubkey) -> Result<Pubkey> {
    let vote_accounts = client
        .get_vote_accounts()
        .context("failed to fetch vote accounts")?;
    let found = vote_accounts
        .current
        .iter()
        .chain(vote_accounts.delinquent.iter())
        .any(|entry| entry.vote_pubkey == vote_account.to_string());
    ensure!(
        found,
        "{} is not a current or delinquent vote account on this cluster; pass the validator's vote account pubkey, not a wallet or stake address",
        vote_account
    );
    Ok(vote_account)
}

fn describe_stake_state(state: &StakeStateV2) -> &'static str {
    match state {
        StakeStateV2::Uninitialized => "Uninitialized",
        StakeStateV2::Initialized(_) => "Initialized",
        StakeStateV2::Stake(_, _, _) => "Stake",
        StakeStateV2::RewardsPool => "RewardsPool",
    }
}

fn unlock_device(port: &mut dyn SerialPort, pin: &str, timeout: Duration) -> Result<()> {
    let response = send_command(port, &format!("UNLOCK:PIN:{pin}"), timeout)?;
    expect_line_prefix("UNLOCK:PIN", &response, "UNLOCKED_UNTIL:")
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
) -> Result<Instruction> {
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

    Ok(Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(Pubkey::from_str(SYSTEM_PROGRAM_ID).unwrap(), false),
        ],
        data,
    })
}

fn create_advance_instruction(
    program_id: Pubkey,
    identity: &[u8],
    signature: &[u8],
) -> Result<Instruction> {
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );
    let (vector_pda, _) = find_vector_pda(&program_id, identity);

    let mut data = Vec::with_capacity(1 + signature.len());
    data.push(ADVANCE_DISCRIMINATOR);
    data.extend_from_slice(signature);

    Ok(Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID).unwrap(), false),
        ],
        data,
    })
}

fn create_passthrough_instruction(
    program_id: Pubkey,
    identity: &[u8],
    instructions: &[Instruction],
) -> Result<Instruction> {
    ensure!(
        instructions.len() <= u8::MAX as usize,
        "too many passthrough sub-instructions: {}",
        instructions.len()
    );

    let (vector_pda, _) = find_vector_pda(&program_id, identity);
    let mut accounts = Vec::with_capacity(
        2 + instructions
            .iter()
            .map(|ix| 1 + ix.accounts.len())
            .sum::<usize>(),
    );
    accounts.push(AccountMeta::new(vector_pda, false));
    accounts.push(AccountMeta::new_readonly(
        Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID).unwrap(),
        false,
    ));
    for ix in instructions {
        accounts.push(AccountMeta::new_readonly(ix.program_id, false));
        accounts.extend(ix.accounts.iter().map(|meta| AccountMeta {
            pubkey: meta.pubkey,
            is_signer: false,
            is_writable: meta.is_writable,
        }));
    }

    let mut data = Vec::with_capacity(
        2 + instructions
            .iter()
            .map(|ix| 3 + ix.data.len())
            .sum::<usize>(),
    );
    data.push(PASSTHROUGH_DISCRIMINATOR);
    data.push(instructions.len() as u8);
    for ix in instructions {
        ensure!(
            ix.accounts.len() <= u8::MAX as usize,
            "passthrough sub-instruction has too many accounts: {}",
            ix.accounts.len()
        );
        ensure!(
            ix.data.len() <= u16::MAX as usize,
            "passthrough sub-instruction data too long: {}",
            ix.data.len()
        );
        data.push(ix.accounts.len() as u8);
        data.extend_from_slice(&(ix.data.len() as u16).to_le_bytes());
        data.extend_from_slice(&ix.data);
    }

    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

fn create_withdraw_subinstruction(
    program_id: Pubkey,
    identity: &[u8],
    receiver: Pubkey,
    lamports: u64,
) -> Result<Instruction> {
    let (vector_pda, _) = find_vector_pda(&program_id, identity);
    let mut data = Vec::with_capacity(1 + 8);
    data.push(WITHDRAW_DISCRIMINATOR);
    data.extend_from_slice(&lamports.to_le_bytes());

    Ok(Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(vector_pda, false),
            AccountMeta::new(receiver, false),
        ],
        data,
    })
}

fn create_system_transfer_instruction(
    from: Pubkey,
    to: Pubkey,
    lamports: u64,
) -> Result<Instruction> {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&SYSTEM_TRANSFER_DISCRIMINATOR.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());

    Ok(Instruction {
        program_id: Pubkey::from_str(SYSTEM_PROGRAM_ID)
            .context("failed to parse system program id")?,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data,
    })
}

fn create_spl_transfer_checked_subinstruction(
    mint: Pubkey,
    source: Pubkey,
    destination: Pubkey,
    authority: Pubkey,
    amount: u64,
    decimals: u8,
) -> Result<Instruction> {
    spl_transfer_checked(
        &spl_token_program_id(),
        &source,
        &mint,
        &destination,
        &authority,
        &[],
        amount,
        decimals,
    )
    .map_err(|err| anyhow!("failed to build transfer_checked instruction: {err}"))
}

fn create_spl_close_account_subinstruction(
    token_program: Pubkey,
    account: Pubkey,
    destination: Pubkey,
    authority: Pubkey,
) -> Result<Instruction> {
    spl_close_account(
        &token_program,
        &account,
        &destination,
        &authority,
        &[],
    )
    .map_err(|err| anyhow!("failed to build close_account instruction: {err}"))
}

fn compute_budget_set_compute_unit_limit(units: u32) -> Result<Instruction> {
    let mut data = Vec::with_capacity(5);
    data.push(2);
    data.extend_from_slice(&units.to_le_bytes());
    Ok(Instruction {
        program_id: Pubkey::from_str(COMPUTE_BUDGET_PROGRAM_ID)
            .context("failed to parse compute budget program id")?,
        accounts: vec![],
        data,
    })
}

fn advance_vector_digest(
    program_id: Pubkey,
    identity: &[u8],
    nonce: &[u8; 32],
    pre_instructions: &[Instruction],
    post_instructions: &[Instruction],
    fee_payer: Option<Pubkey>,
) -> Result<[u8; 32]> {
    let placeholder = vec![0u8; FALCON_512_SIGNATURE_LEN];
    let advance_ix = create_advance_instruction(program_id, identity, &placeholder)?;
    let target_index = pre_instructions.len();

    let mut all = Vec::with_capacity(pre_instructions.len() + 1 + post_instructions.len());
    all.extend(pre_instructions.iter().cloned());
    all.push(advance_ix);
    all.extend(post_instructions.iter().cloned());

    let promoted = promote_to_message_flags(&all, fee_payer);
    let borrowed: Vec<_> = promoted
        .iter()
        .map(|ix| BorrowedInstruction {
            program_id: &ix.program_id,
            accounts: ix
                .accounts
                .iter()
                .map(|meta| BorrowedAccountMeta {
                    pubkey: &meta.pubkey,
                    is_signer: meta.is_signer,
                    is_writable: meta.is_writable,
                })
                .collect(),
            data: &ix.data,
        })
        .collect();

    let mut buffer = construct_instructions_data(&borrowed);
    let footer_offset = buffer
        .len()
        .checked_sub(2)
        .ok_or_else(|| anyhow!("instructions sysvar buffer too short"))?;
    buffer[footer_offset..].copy_from_slice(&(target_index as u16).to_le_bytes());

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
    instructions: &[Instruction],
    fee_payer: Option<Pubkey>,
) -> Vec<Instruction> {
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
        .map(|ix| Instruction {
            program_id: ix.program_id,
            accounts: ix
                .accounts
                .iter()
                .map(|meta| {
                    let promoted = flags
                        .get(&meta.pubkey.to_bytes())
                        .copied()
                        .unwrap_or_default();
                    AccountMeta {
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

fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

fn format_token_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }

    let decimals = decimals as usize;
    let mut out = format!("{amount:0width$}", width = decimals + 1);
    let split = out.len() - decimals;
    out.insert(split, '.');
    while out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
    out
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

fn lamports_to_sol(lamports: u64) -> String {
    format!("{:.9}", lamports as f64 / 1_000_000_000.0)
}

#[allow(dead_code)]
fn print_instruction(ix: &Instruction) {
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

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}
