use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{
    Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey,
};
use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, PublicKey as _, SecretKey as _};
use serialport::{ClearBuffer, SerialPort};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use solana_commitment_config::CommitmentConfig;
use solana_falcon512::{
    Falcon512Pubkey, Falcon512Signature, FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN,
};
use solana_instruction::{AccountMeta, BorrowedAccountMeta, BorrowedInstruction, Instruction};
use solana_instructions_sysvar::construct_instructions_data;
use solana_keypair::{read_keypair_file, Keypair};
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_rpc_client::rpc_client::RpcClient;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::address::get_associated_token_address_with_program_id;
use spl_token_interface::id as spl_token_program_id;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    thread::sleep,
    time::{Duration, Instant},
};

const BAUD: u32 = 115_200;
const VECTOR_PDA_SEED: &[u8] = b"vector";
const VAULT_PDA_SEED: &[u8] = b"pq_vault";
const PROPOSAL_PDA_SEED: &[u8] = b"proposal";
const PROPOSAL_TX_PDA_SEED: &[u8] = b"proposal_tx";
const ACTION_HASH_DOMAIN: &[u8] = b"UNRUGGABLE_PQ_MULTISIG_PROPOSAL_V1";
const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_VECTOR_PROGRAM_ID: &str = "FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU";
const DEFAULT_VAULT_PROGRAM_ID: &str = "9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52";
const DEFAULT_ALIASES_FILE: &str = ".pq-multisig-ops.aliases.json";
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
const INSTRUCTIONS_SYSVAR_ID: &str = "Sysvar1nstructions1111111111111111111111111";
const INITIALIZE_VECTOR_DISCRIMINATOR: u8 = 0;
const ADVANCE_VECTOR_DISCRIMINATOR: u8 = 1;
const PASSTHROUGH_VECTOR_DISCRIMINATOR: u8 = 4;
const INITIALIZE_VAULT_DISCRIMINATOR: u8 = 0;
const CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 1;
const APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 2;
const EXECUTE_PROPOSAL_DISCRIMINATOR: u8 = 3;
const CANCEL_PROPOSAL_DISCRIMINATOR: u8 = 4;
const CLOSE_PROPOSAL_DISCRIMINATOR: u8 = 5;
const ACCEPT_PENDING_MEMBER_DISCRIMINATOR: u8 = 6;
const REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 7;
const ACTION_KIND_CPI_BUNDLE: u8 = 0;
const ACTION_KIND_REPLACE_MEMBER: u8 = 1;
const ACTION_KIND_SOL_WITHDRAW: u8 = 4;
const ACTION_KIND_SPL_TRANSFER: u8 = 5;
const ACTION_KIND_UPDATE_CONFIG: u8 = 6;
const BINDING_TAG: &[u8] = b"unruggable-bind-v2";
const ED25519_SIG_BYTES: usize = 64;
const VAULT_ACCOUNT_LEN: usize = 224;
const PROPOSAL_ACCOUNT_LEN: usize = 160;
const PROPOSAL_TX_HEADER_LEN: usize = 40;
const VECTOR_FALCON_ACCOUNT_LEN: usize = 1090;
const MAX_MEMBERS: usize = 3;
const MEMBER_RECORD_LEN: usize = 40;
const DEFAULT_PROPOSAL_SLOT_MARGIN: u64 = 4;
const DEFAULT_DEVICE_PROPOSAL_SLOT_MARGIN: u64 = 16;
const PROPOSAL_STATE_ACTIVE: u8 = 0;
const PROPOSAL_STATE_EXECUTED: u8 = 1;
const PROPOSAL_STATE_CANCELLED: u8 = 2;
const PROPOSAL_STATE_EXPIRED: u8 = 3;
const NO_PENDING_MEMBER_INDEX: u8 = u8::MAX;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    GenerateMemberFixture,
    InitializeVector,
    InitVault,
    ShowVault,
    ListProposals,
    ShowProposal,
    CreateSolWithdraw,
    CreateSplTransfer,
    CreateConfigUpdate,
    CreateReplaceMember,
    Approve,
    Revoke,
    Cancel,
    AcceptPendingMember,
    Execute,
    Close,
}

impl Mode {
    fn from_str(raw: &str) -> Result<Self> {
        match raw {
            "generate-member-fixture" => Ok(Self::GenerateMemberFixture),
            "initialize-vector" => Ok(Self::InitializeVector),
            "init-vault" => Ok(Self::InitVault),
            "show-vault" => Ok(Self::ShowVault),
            "list-proposals" => Ok(Self::ListProposals),
            "show-proposal" => Ok(Self::ShowProposal),
            "create-sol-withdraw" => Ok(Self::CreateSolWithdraw),
            "create-spl-transfer" => Ok(Self::CreateSplTransfer),
            "create-config-update" => Ok(Self::CreateConfigUpdate),
            "create-replace-member" => Ok(Self::CreateReplaceMember),
            "approve" => Ok(Self::Approve),
            "revoke" => Ok(Self::Revoke),
            "cancel" => Ok(Self::Cancel),
            "accept-pending-member" => Ok(Self::AcceptPendingMember),
            "execute" => Ok(Self::Execute),
            "close" => Ok(Self::Close),
            _ => bail!("unsupported --mode value: {raw}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::GenerateMemberFixture => "generate-member-fixture",
            Self::InitializeVector => "initialize-vector",
            Self::InitVault => "init-vault",
            Self::ShowVault => "show-vault",
            Self::ListProposals => "list-proposals",
            Self::ShowProposal => "show-proposal",
            Self::CreateSolWithdraw => "create-sol-withdraw",
            Self::CreateSplTransfer => "create-spl-transfer",
            Self::CreateConfigUpdate => "create-config-update",
            Self::CreateReplaceMember => "create-replace-member",
            Self::Approve => "approve",
            Self::Revoke => "revoke",
            Self::Cancel => "cancel",
            Self::AcceptPendingMember => "accept-pending-member",
            Self::Execute => "execute",
            Self::Close => "close",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposalStateFilter {
    Active,
    Executed,
    Cancelled,
    Expired,
    All,
}

impl ProposalStateFilter {
    fn from_str(raw: &str) -> Result<Self> {
        match raw {
            "active" => Ok(Self::Active),
            "executed" => Ok(Self::Executed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "all" => Ok(Self::All),
            _ => bail!("unsupported --state value: {raw}"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Executed => "executed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposalQueueFilter {
    All,
    NeedsApproval,
    Executable,
    Closable,
}

impl ProposalQueueFilter {
    fn from_str(raw: &str) -> Result<Self> {
        match raw {
            "all" => Ok(Self::All),
            "needs-approval" => Ok(Self::NeedsApproval),
            "executable" => Ok(Self::Executable),
            "closable" => Ok(Self::Closable),
            _ => bail!("unsupported --queue value: {raw}"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::NeedsApproval => "needs-approval",
            Self::Executable => "executable",
            Self::Closable => "closable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectiveProposalState {
    Active,
    Executed,
    Cancelled,
    Expired,
}

impl EffectiveProposalState {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Executed => "Executed",
            Self::Cancelled => "Cancelled",
            Self::Expired => "Expired",
        }
    }
}

#[derive(Clone)]
struct Config {
    mode: Mode,
    rpc_url: String,
    fee_payer_file: Option<PathBuf>,
    fee_payer_device: bool,
    vector_program_id: Pubkey,
    vault_program_id: Pubkey,
    commitment: CommitmentConfig,
    json: bool,
    manifest_file: Option<PathBuf>,
    manifest_only: bool,
    aliases_file: Option<PathBuf>,
    vault_seed: Option<Vec<u8>>,
    vault: Option<Pubkey>,
    vault_alias: Option<String>,
    member: Option<Pubkey>,
    member_alias: Option<String>,
    proposal_id: Option<u64>,
    proposal_state_filter: ProposalStateFilter,
    proposal_queue_filter: ProposalQueueFilter,
    action_kind_filter: Option<u8>,
    threshold: Option<u8>,
    proposal_ttl_slots: Option<u64>,
    timelock_slots: Option<u64>,
    vault_members: Vec<Pubkey>,
    receiver: Option<Pubkey>,
    lamports: Option<u64>,
    spl_mint: Option<Pubkey>,
    spl_destination: Option<Pubkey>,
    spl_amount: Option<u64>,
    spl_decimals: Option<u8>,
    replace_member_index: Option<u8>,
    new_member_pda: Option<Pubkey>,
    new_member_alias: Option<String>,
    proposal_offset_slots: Option<u64>,
    not_before_slot: Option<u64>,
    expires_at_slot: Option<u64>,
    member_fixture_file: Option<PathBuf>,
    output_file: Option<PathBuf>,
    overwrite: bool,
    port: Option<String>,
    pin: String,
    read_timeout: Duration,
    action_timeout: Duration,
    open_wait: Duration,
    auto_init: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct MemberFixtureFile {
    wire_pubkey_b64: String,
    secret_key_b64: String,
}

enum MemberSession {
    Device(DeviceMemberSession),
    Fixture(FixtureMemberSession),
}

struct DeviceMemberSession {
    port: Box<dyn SerialPort>,
    pin: String,
    read_timeout: Duration,
    action_timeout: Duration,
    se050_pubkey: Pubkey,
    wire_pubkey: [u8; FALCON_512_PUBKEY_LEN],
    identity: [u8; 32],
    vector_pda: Pubkey,
}

struct FixtureMemberSession {
    fixture_path: PathBuf,
    wire_pubkey: [u8; FALCON_512_PUBKEY_LEN],
    secret_key: falcon512::SecretKey,
    identity: [u8; 32],
    vector_pda: Pubkey,
}

#[derive(Clone, Copy)]
struct VaultSnapshot {
    threshold: u8,
    member_count: u8,
    pending_member_index: u8,
    pending_member_pda: Pubkey,
    config_version: u64,
    next_proposal_id: u64,
    timelock_slots: u64,
    proposal_ttl_slots: u64,
    members: [VaultMemberSnapshot; MAX_MEMBERS],
}

#[derive(Clone, Copy, Default)]
struct VaultMemberSnapshot {
    member_pda: Pubkey,
    enabled: bool,
    weight: u8,
}

#[derive(Clone, Copy)]
struct ProposalSnapshot {
    state: u8,
    proposer_member_index: u8,
    approval_bitmap: u8,
    approval_count: u8,
    action_kind: u8,
    proposal_id: u64,
    config_version: u64,
    created_slot: u64,
    not_before_slot: u64,
    expires_at_slot: u64,
    action_hash: [u8; 32],
    tx_account: Pubkey,
    executed_slot: u64,
}

struct ProposalTxSnapshot {
    action_hash: [u8; 32],
    payload: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct AliasesFile {
    #[serde(default)]
    vaults: BTreeMap<String, VaultAliasEntry>,
    #[serde(default)]
    members: BTreeMap<String, MemberAliasEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct VaultAliasEntry {
    #[serde(default)]
    pubkey: Option<String>,
    #[serde(default)]
    seed: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MemberAliasEntry {
    vector_pda: String,
    #[serde(default)]
    fixture_file: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct OpsManifest {
    version: &'static str,
    mode: String,
    description: String,
    script: String,
    args: Vec<String>,
    rpc_url: String,
    vector_program_id: String,
    vault_program_id: String,
    aliases_file: Option<String>,
    vault: Option<String>,
    vault_alias: Option<String>,
    member: Option<String>,
    member_alias: Option<String>,
    proposal_id: Option<u64>,
    summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposalQueueStatus {
    NeedsApproval,
    Executable,
    Closable,
    WaitingThreshold,
    WaitingTimelock,
    StaleConfig,
}

impl ProposalQueueStatus {
    fn label(self) -> &'static str {
        match self {
            Self::NeedsApproval => "needs-approval",
            Self::Executable => "executable",
            Self::Closable => "closable",
            Self::WaitingThreshold => "waiting-threshold",
            Self::WaitingTimelock => "waiting-timelock",
            Self::StaleConfig => "stale-config",
        }
    }
}

#[derive(Clone, Copy)]
struct ListMemberStatus {
    member_pda: Pubkey,
    is_active: bool,
    has_approved: bool,
    can_approve: bool,
}

#[derive(Clone, Copy)]
struct ProposalListView {
    display_state: EffectiveProposalState,
    queue_status: ProposalQueueStatus,
    member_status: Option<ListMemberStatus>,
}

struct MemberSubmitResult {
    tx_signature: Signature,
    vector_digest: [u8; 32],
}

enum FeePayer {
    Local(Keypair),
    Device { pubkey: Pubkey },
}

impl FeePayer {
    fn pubkey(&self) -> Pubkey {
        match self {
            Self::Local(keypair) => keypair.pubkey(),
            Self::Device { pubkey } => *pubkey,
        }
    }

    fn device_pubkey(&self) -> Option<Pubkey> {
        match self {
            Self::Local(_) => None,
            Self::Device { pubkey } => Some(*pubkey),
        }
    }
}

impl VaultSnapshot {
    fn member_index(&self, member_pda: Pubkey) -> Option<usize> {
        self.members
            .iter()
            .take(self.member_count as usize)
            .enumerate()
            .find_map(|(index, member)| {
                (member.enabled && member.member_pda == member_pda).then_some(index)
            })
    }
}

impl ProposalSnapshot {
    fn has_member_approval(&self, member_index: usize) -> bool {
        (self.approval_bitmap & (1u8 << member_index)) != 0
    }
}

fn main() -> Result<()> {
    let config = parse_args()?;
    run(config)
}

fn run(config: Config) -> Result<()> {
    maybe_emit_manifest(&config)?;
    if config.manifest_only {
        return Ok(());
    }

    match config.mode {
        Mode::GenerateMemberFixture => run_generate_member_fixture(&config),
        mode => {
            let client = RpcClient::new_with_commitment(config.rpc_url.clone(), config.commitment);
            match mode {
                Mode::InitializeVector => run_initialize_vector(&client, &config),
                Mode::InitVault => run_init_vault(&client, &config),
                Mode::ShowVault => run_show_vault(&client, &config),
                Mode::ListProposals => run_list_proposals(&client, &config),
                Mode::ShowProposal => run_show_proposal(&client, &config),
                Mode::CreateSolWithdraw => {
                    run_create_sol_withdraw(&client, &config)
                }
                Mode::CreateSplTransfer => {
                    run_create_spl_transfer(&client, &config)
                }
                Mode::CreateConfigUpdate => {
                    run_create_config_update(&client, &config)
                }
                Mode::CreateReplaceMember => {
                    run_create_replace_member(&client, &config)
                }
                Mode::Approve => run_member_approval_action(
                    &client,
                    &config,
                    "Approve",
                    |vault_program_id, member, vault, proposal_id, action_hash| {
                        create_approve_proposal_from_member_instruction(
                            vault_program_id,
                            member,
                            vault,
                            proposal_id,
                            action_hash,
                        )
                    },
                ),
                Mode::Revoke => run_member_approval_action(
                    &client,
                    &config,
                    "Revoke",
                    |vault_program_id, member, vault, proposal_id, action_hash| {
                        create_revoke_proposal_from_member_instruction(
                            vault_program_id,
                            member,
                            vault,
                            proposal_id,
                            action_hash,
                        )
                    },
                ),
                Mode::Cancel => run_member_approval_action(
                    &client,
                    &config,
                    "Cancel",
                    |vault_program_id, member, vault, proposal_id, action_hash| {
                        create_cancel_proposal_from_member_instruction(
                            vault_program_id,
                            member,
                            vault,
                            proposal_id,
                            action_hash,
                        )
                    },
                ),
                Mode::AcceptPendingMember => {
                    run_accept_pending_member(&client, &config)
                }
                Mode::Execute => run_execute(&client, &config),
                Mode::Close => run_close(&client, &config),
                Mode::GenerateMemberFixture => unreachable!(),
            }
        }
    }
}

fn maybe_emit_manifest(config: &Config) -> Result<()> {
    if config.manifest_file.is_none() && !config.manifest_only {
        return Ok(());
    }

    let manifest = build_manifest(config)?;
    let serialized =
        serde_json::to_string_pretty(&manifest).context("failed to serialize manifest JSON")?;

    if let Some(path) = &config.manifest_file {
        fs::write(path, &serialized)
            .with_context(|| format!("failed to write manifest {}", path.display()))?;
    }
    if config.manifest_only || config.manifest_file.is_none() {
        println!("{serialized}");
    }
    Ok(())
}

fn build_manifest(config: &Config) -> Result<OpsManifest> {
    let vault = match (&config.vault, &config.vault_alias, &config.vault_seed) {
        (Some(vault), _, _) => Some(vault.to_string()),
        (None, Some(_), _) => resolve_vault(config).ok().map(|vault| vault.to_string()),
        (None, None, Some(_)) => resolve_vault(config).ok().map(|vault| vault.to_string()),
        _ => None,
    };
    let member = match (&config.member, &config.member_alias) {
        (Some(member), _) => Some(member.to_string()),
        (None, Some(alias)) => resolve_member_alias_entry(config, alias)
            .ok()
            .and_then(|entry| parse_pubkey_arg(&entry.vector_pda).ok())
            .map(|member| member.to_string()),
        _ => None,
    };
    Ok(OpsManifest {
        version: "v1",
        mode: config.mode.as_str().to_string(),
        description: manifest_description(config),
        script: "./scripts/run_pq_multisig_ops.sh".to_string(),
        args: build_manifest_args(config),
        rpc_url: config.rpc_url.clone(),
        vector_program_id: config.vector_program_id.to_string(),
        vault_program_id: config.vault_program_id.to_string(),
        aliases_file: aliases_path(config).map(|path| path.display().to_string()),
        vault,
        vault_alias: config.vault_alias.clone(),
        member,
        member_alias: config.member_alias.clone(),
        proposal_id: config.proposal_id,
        summary: manifest_summary(config),
    })
}

fn build_manifest_args(config: &Config) -> Vec<String> {
    let mut args = vec!["--mode".to_string(), config.mode.as_str().to_string()];
    args.push("--rpc-url".to_string());
    args.push(config.rpc_url.clone());
    if config.fee_payer_device {
        args.push("--fee-payer-device".to_string());
    }
    if let Some(path) = &config.fee_payer_file {
        args.push("--fee-payer-file".to_string());
        args.push(path.display().to_string());
    }
    args.push("--vector-program-id".to_string());
    args.push(config.vector_program_id.to_string());
    args.push("--vault-program-id".to_string());
    args.push(config.vault_program_id.to_string());
    args.push("--commitment".to_string());
    args.push(commitment_label(config.commitment).to_string());
    if let Some(path) = aliases_path(config) {
        args.push("--aliases-file".to_string());
        args.push(path.display().to_string());
    }
    if config.json {
        args.push("--json".to_string());
    }
    push_string_arg(&mut args, "--vault", config.vault.map(|value| value.to_string()));
    push_string_arg(&mut args, "--vault-alias", config.vault_alias.clone());
    push_string_arg(
        &mut args,
        "--vault-seed",
        config
            .vault_seed
            .as_ref()
            .map(|seed| String::from_utf8_lossy(seed).to_string()),
    );
    push_string_arg(&mut args, "--member", config.member.map(|value| value.to_string()));
    push_string_arg(&mut args, "--member-alias", config.member_alias.clone());
    push_string_arg(&mut args, "--proposal-id", config.proposal_id.map(|value| value.to_string()));
    if config.proposal_state_filter != ProposalStateFilter::All {
        push_string_arg(
            &mut args,
            "--state",
            Some(config.proposal_state_filter.label().to_string()),
        );
    }
    if config.proposal_queue_filter != ProposalQueueFilter::All {
        push_string_arg(
            &mut args,
            "--queue",
            Some(config.proposal_queue_filter.label().to_string()),
        );
    }
    if let Some(kind) = config.action_kind_filter {
        push_string_arg(&mut args, "--action-kind", Some(action_kind_flag(kind).to_string()));
    }
    push_string_arg(&mut args, "--threshold", config.threshold.map(|value| value.to_string()));
    push_string_arg(
        &mut args,
        "--proposal-ttl-slots",
        config.proposal_ttl_slots.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--timelock-slots",
        config.timelock_slots.map(|value| value.to_string()),
    );
    if !config.vault_members.is_empty() {
        push_string_arg(
            &mut args,
            "--vault-members",
            Some(
                config
                    .vault_members
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
    }
    push_string_arg(&mut args, "--receiver", config.receiver.map(|value| value.to_string()));
    push_string_arg(&mut args, "--lamports", config.lamports.map(|value| value.to_string()));
    push_string_arg(&mut args, "--spl-mint", config.spl_mint.map(|value| value.to_string()));
    push_string_arg(
        &mut args,
        "--spl-destination",
        config.spl_destination.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--spl-amount",
        config.spl_amount.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--spl-decimals",
        config.spl_decimals.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--replace-member-index",
        config.replace_member_index.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--new-member-pda",
        config.new_member_pda.map(|value| value.to_string()),
    );
    push_string_arg(&mut args, "--new-member-alias", config.new_member_alias.clone());
    push_string_arg(
        &mut args,
        "--proposal-offset-slots",
        config.proposal_offset_slots.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--not-before-slot",
        config.not_before_slot.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--expires-at-slot",
        config.expires_at_slot.map(|value| value.to_string()),
    );
    push_string_arg(
        &mut args,
        "--member-fixture-file",
        config
            .member_fixture_file
            .as_ref()
            .map(|path| path.display().to_string()),
    );
    push_string_arg(
        &mut args,
        "--output-file",
        config.output_file.as_ref().map(|path| path.display().to_string()),
    );
    if config.overwrite {
        args.push("--overwrite".to_string());
    }
    push_string_arg(&mut args, "--port", config.port.clone());
    if config.pin != "246810" {
        args.push("--pin".to_string());
        args.push(config.pin.clone());
    }
    if config.read_timeout != Duration::from_millis(4_000) {
        push_string_arg(
            &mut args,
            "--read-timeout-ms",
            Some(config.read_timeout.as_millis().to_string()),
        );
    }
    if config.action_timeout != Duration::from_millis(35_000) {
        push_string_arg(
            &mut args,
            "--action-timeout-ms",
            Some(config.action_timeout.as_millis().to_string()),
        );
    }
    if config.open_wait != Duration::from_millis(2_000) {
        push_string_arg(
            &mut args,
            "--open-wait-ms",
            Some(config.open_wait.as_millis().to_string()),
        );
    }
    if !config.auto_init {
        args.push("--no-auto-init".to_string());
    }
    args
}

fn push_string_arg(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value);
    }
}

fn manifest_description(config: &Config) -> String {
    match config.mode {
        Mode::ShowVault => "Inspect vault configuration".to_string(),
        Mode::ListProposals => "Discover proposals directly from chain".to_string(),
        Mode::ShowProposal => "Inspect a specific proposal".to_string(),
        Mode::CreateSolWithdraw => "Create a SOL withdraw proposal".to_string(),
        Mode::CreateSplTransfer => "Create an SPL transfer proposal".to_string(),
        Mode::CreateConfigUpdate => "Create a config update proposal".to_string(),
        Mode::CreateReplaceMember => "Create a replace-member proposal".to_string(),
        Mode::Approve => "Approve a proposal".to_string(),
        Mode::Revoke => "Revoke an existing approval".to_string(),
        Mode::Cancel => "Cancel a proposal".to_string(),
        Mode::AcceptPendingMember => "Accept a pending member replacement".to_string(),
        Mode::Execute => "Execute a threshold-approved proposal".to_string(),
        Mode::Close => "Close an executed, cancelled, or expired proposal".to_string(),
        Mode::InitializeVector => "Initialize the member Vector PDA".to_string(),
        Mode::InitVault => "Initialize a PQ vault".to_string(),
        Mode::GenerateMemberFixture => "Generate a local Falcon member fixture".to_string(),
    }
}

fn manifest_summary(config: &Config) -> Option<String> {
    match config.mode {
        Mode::CreateSolWithdraw => Some(format!(
            "withdraw {} lamports to {}",
            config.lamports.unwrap_or_default(),
            config
                .receiver
                .map(|receiver| receiver.to_string())
                .unwrap_or_else(|| "<missing receiver>".to_string())
        )),
        Mode::CreateSplTransfer => Some(format!(
            "transfer {} raw units of {} to {}",
            config.spl_amount.unwrap_or_default(),
            config
                .spl_mint
                .map(|mint| mint.to_string())
                .unwrap_or_else(|| "<missing mint>".to_string()),
            config
                .spl_destination
                .map(|destination| destination.to_string())
                .unwrap_or_else(|| "<missing destination>".to_string())
        )),
        Mode::CreateConfigUpdate => Some(format!(
            "update threshold={} ttl={} timelock={}",
            config.threshold.unwrap_or_default(),
            config.proposal_ttl_slots.unwrap_or_default(),
            config.timelock_slots.unwrap_or_default()
        )),
        Mode::CreateReplaceMember => Some(format!(
            "replace member[{}] with {}",
            config.replace_member_index.unwrap_or_default(),
            config
                .new_member_pda
                .map(|member| member.to_string())
                .or_else(|| config.new_member_alias.clone())
                .unwrap_or_else(|| "<missing member>".to_string())
        )),
        Mode::Approve
        | Mode::Revoke
        | Mode::Cancel
        | Mode::Execute
        | Mode::Close
        | Mode::ShowProposal => config
            .proposal_id
            .map(|proposal_id| format!("proposal {}", proposal_id)),
        _ => None,
    }
}

fn commitment_label(commitment: CommitmentConfig) -> &'static str {
    if commitment == CommitmentConfig::processed() {
        "processed"
    } else if commitment == CommitmentConfig::finalized() {
        "finalized"
    } else {
        "confirmed"
    }
}

fn action_kind_flag(kind: u8) -> &'static str {
    match kind {
        ACTION_KIND_SOL_WITHDRAW => "sol-withdraw",
        ACTION_KIND_SPL_TRANSFER => "spl-transfer",
        ACTION_KIND_UPDATE_CONFIG => "config-update",
        ACTION_KIND_REPLACE_MEMBER => "replace-member",
        _ => "unknown",
    }
}

fn print_usage() {
    println!(
        "\
PQ Multisig Ops

Modes:
  generate-member-fixture  Write a local Falcon member fixture JSON for localnet simulation
  initialize-vector        Initialize the member's Vector PDA if needed
  init-vault               Initialize a PQ vault PDA with the given member Vector PDAs
  show-vault               Print the current vault configuration
  list-proposals           Scan vault proposals directly from chain with queue filters
  show-proposal            Print proposal and payload state
  create-sol-withdraw      Create a SOL withdraw proposal from the current member
  create-spl-transfer      Create an SPL transfer proposal from the current member
  create-config-update     Create a config update proposal from the current member
  create-replace-member    Create a pending-member rotation proposal from the current member
  approve                  Approve a proposal from the current member
  revoke                   Revoke an existing approval from the current member
  cancel                   Cancel a proposal from the proposer before threshold is reached
  accept-pending-member    Accept a pending member replacement using the incoming member
  execute                  Execute a threshold-approved proposal with auto-derived extra accounts
  close                    Close an executed, cancelled, or expired proposal

Common options:
  --rpc-url <url>              RPC endpoint. Default: http://127.0.0.1:8899
  --fee-payer-device           Use the connected SE050 key as the outer Solana fee payer
  --fee-payer-file <path>      Local Solana keypair used for outer transaction fees
  --vector-program-id <id>     Vector Falcon program id. Default: FaLCn556d...
  --vault-program-id <id>      PQ vault program id. Default: 9v16YaZY...
  --json                       Emit machine-readable JSON for read modes
  --manifest-file <path>       Write a command manifest JSON to the given path
  --manifest-only              Emit the manifest and exit without executing
  --aliases-file <path>        Alias file. Default if present: .pq-multisig-ops.aliases.json
  --vault <pubkey>             Existing vault PDA
  --vault-alias <name>         Named vault alias from the aliases file
  --vault-seed <string>        Vault seed bytes for PDA derivation / initialization
  --member <pubkey>            Member Vector PDA for queue filtering / inbox views
  --member-alias <name>        Named member alias from the aliases file
  --proposal-id <n>            Proposal id for proposal-specific modes
  --commitment <level>         processed | confirmed | finalized. Default: confirmed

Member auth options:
  --member-fixture-file <path> Use a local Falcon fixture JSON
  --port <path>                Use a real Falcon device over serial
  --pin <digits>               Device PIN. Default: 246810
  --read-timeout-ms <n>        Serial read timeout. Default: 4000
  --action-timeout-ms <n>      Serial action timeout. Default: 35000
  --open-wait-ms <n>           Delay after opening serial port. Default: 2000
  --no-auto-init               Do not initialize PQC automatically if blank

Mode-specific options:
  --output-file <path>         Output path for generate-member-fixture
  --overwrite                  Overwrite --output-file if it exists
  --vault-members <csv>        Comma-separated member Vector PDAs for init-vault
  --state <value>              Proposal filter: active | executed | cancelled | expired | all
  --queue <value>              Proposal queue: all | needs-approval | executable | closable
  --action-kind <value>        Proposal action: sol-withdraw | spl-transfer | config-update | replace-member | all
  --threshold <n>              Threshold for init-vault or config update
  --proposal-ttl-slots <n>     Proposal TTL slots for init-vault or config update
  --timelock-slots <n>         Timelock slots for init-vault or config update
  --not-before-slot <n>        Override not-before slot for create-* modes
  --expires-at-slot <n>        Override expiry slot for create-* modes
  --receiver <pubkey>          SOL withdraw receiver
  --lamports <n>               SOL withdraw amount
  --spl-mint <pubkey>          SPL mint for create-spl-transfer
  --spl-destination <pubkey>   Destination token account for create-spl-transfer
  --spl-amount <n>             Raw SPL transfer amount
  --spl-decimals <n>           SPL decimals for create-spl-transfer
  --replace-member-index <n>   Member index to rotate out
  --new-member-pda <pubkey>    Replacement member Vector PDA
  --new-member-alias <name>    Replacement member alias from the aliases file
"
    );
}

fn parse_args() -> Result<Config> {
    let mut args = std::env::args().skip(1);
    let mut mode = None;
    let mut rpc_url = DEFAULT_RPC_URL.to_string();
    let mut fee_payer_file = None;
    let mut fee_payer_device = false;
    let mut vector_program_id =
        Pubkey::from_str(DEFAULT_VECTOR_PROGRAM_ID).context("invalid default vector program id")?;
    let mut vault_program_id =
        Pubkey::from_str(DEFAULT_VAULT_PROGRAM_ID).context("invalid default vault program id")?;
    let mut commitment = CommitmentConfig::confirmed();
    let mut json = false;
    let mut manifest_file = None;
    let mut manifest_only = false;
    let mut aliases_file = None;
    let mut vault_seed = None;
    let mut vault = None;
    let mut vault_alias = None;
    let mut member = None;
    let mut member_alias = None;
    let mut proposal_id = None;
    let mut proposal_state_filter = ProposalStateFilter::All;
    let mut proposal_queue_filter = ProposalQueueFilter::All;
    let mut action_kind_filter = None;
    let mut threshold = None;
    let mut proposal_ttl_slots = None;
    let mut timelock_slots = None;
    let mut vault_members: Vec<Pubkey> = Vec::new();
    let mut receiver = None;
    let mut lamports = None;
    let mut spl_mint = None;
    let mut spl_destination = None;
    let mut spl_amount = None;
    let mut spl_decimals = None;
    let mut replace_member_index = None;
    let mut new_member_pda = None;
    let mut new_member_alias = None;
    let mut proposal_offset_slots = None;
    let mut not_before_slot = None;
    let mut expires_at_slot = None;
    let mut member_fixture_file = None;
    let mut output_file = None;
    let mut overwrite = false;
    let mut port = None;
    let mut pin = "246810".to_string();
    let mut read_timeout = Duration::from_millis(4_000);
    let mut action_timeout = Duration::from_millis(35_000);
    let mut open_wait = Duration::from_millis(2_000);
    let mut auto_init = true;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => mode = Some(Mode::from_str(&take_arg(&mut args, "--mode")?)?),
            "--rpc-url" => rpc_url = take_arg(&mut args, "--rpc-url")?,
            "--fee-payer-device" => fee_payer_device = true,
            "--fee-payer-file" => {
                fee_payer_file = Some(PathBuf::from(take_arg(&mut args, "--fee-payer-file")?))
            }
            "--vector-program-id" => {
                vector_program_id = parse_pubkey_arg(&take_arg(&mut args, "--vector-program-id")?)?
            }
            "--vault-program-id" => {
                vault_program_id = parse_pubkey_arg(&take_arg(&mut args, "--vault-program-id")?)?
            }
            "--commitment" => {
                commitment = parse_commitment(&take_arg(&mut args, "--commitment")?)?
            }
            "--json" => json = true,
            "--manifest-file" => {
                manifest_file = Some(PathBuf::from(take_arg(&mut args, "--manifest-file")?))
            }
            "--manifest-only" => manifest_only = true,
            "--aliases-file" => {
                aliases_file = Some(PathBuf::from(take_arg(&mut args, "--aliases-file")?))
            }
            "--vault-seed" => {
                vault_seed = Some(take_arg(&mut args, "--vault-seed")?.into_bytes())
            }
            "--vault" => vault = Some(parse_pubkey_arg(&take_arg(&mut args, "--vault")?)?),
            "--vault-alias" => vault_alias = Some(take_arg(&mut args, "--vault-alias")?),
            "--member" => member = Some(parse_pubkey_arg(&take_arg(&mut args, "--member")?)?),
            "--member-alias" => member_alias = Some(take_arg(&mut args, "--member-alias")?),
            "--proposal-id" => proposal_id = Some(parse_u64_arg(&take_arg(&mut args, "--proposal-id")?)?),
            "--state" => {
                proposal_state_filter =
                    ProposalStateFilter::from_str(&take_arg(&mut args, "--state")?)?
            }
            "--queue" => {
                proposal_queue_filter =
                    ProposalQueueFilter::from_str(&take_arg(&mut args, "--queue")?)?
            }
            "--action-kind" => {
                action_kind_filter = parse_action_kind_filter(&take_arg(&mut args, "--action-kind")?)?
            }
            "--threshold" => threshold = Some(parse_u8_arg(&take_arg(&mut args, "--threshold")?)?),
            "--proposal-ttl-slots" => {
                proposal_ttl_slots = Some(parse_u64_arg(&take_arg(&mut args, "--proposal-ttl-slots")?)?)
            }
            "--timelock-slots" => {
                timelock_slots = Some(parse_u64_arg(&take_arg(&mut args, "--timelock-slots")?)?)
            }
            "--vault-members" => {
                let raw = take_arg(&mut args, "--vault-members")?;
                vault_members = parse_pubkey_csv(&raw)?;
            }
            "--receiver" => receiver = Some(parse_pubkey_arg(&take_arg(&mut args, "--receiver")?)?),
            "--lamports" => lamports = Some(parse_u64_arg(&take_arg(&mut args, "--lamports")?)?),
            "--spl-mint" => spl_mint = Some(parse_pubkey_arg(&take_arg(&mut args, "--spl-mint")?)?),
            "--spl-destination" => {
                spl_destination = Some(parse_pubkey_arg(&take_arg(&mut args, "--spl-destination")?)?)
            }
            "--spl-amount" => spl_amount = Some(parse_u64_arg(&take_arg(&mut args, "--spl-amount")?)?),
            "--spl-decimals" => spl_decimals = Some(parse_u8_arg(&take_arg(&mut args, "--spl-decimals")?)?),
            "--replace-member-index" => {
                replace_member_index = Some(parse_u8_arg(&take_arg(&mut args, "--replace-member-index")?)?)
            }
            "--new-member-pda" => {
                new_member_pda = Some(parse_pubkey_arg(&take_arg(&mut args, "--new-member-pda")?)?)
            }
            "--new-member-alias" => {
                new_member_alias = Some(take_arg(&mut args, "--new-member-alias")?)
            }
            "--proposal-offset-slots" => {
                proposal_offset_slots =
                    Some(parse_u64_arg(&take_arg(&mut args, "--proposal-offset-slots")?)?)
            }
            "--not-before-slot" => {
                not_before_slot = Some(parse_u64_arg(&take_arg(&mut args, "--not-before-slot")?)?)
            }
            "--expires-at-slot" => {
                expires_at_slot = Some(parse_u64_arg(&take_arg(&mut args, "--expires-at-slot")?)?)
            }
            "--member-fixture-file" => {
                member_fixture_file =
                    Some(PathBuf::from(take_arg(&mut args, "--member-fixture-file")?))
            }
            "--output-file" => output_file = Some(PathBuf::from(take_arg(&mut args, "--output-file")?)),
            "--overwrite" => overwrite = true,
            "--port" => port = Some(take_arg(&mut args, "--port")?),
            "--pin" => pin = take_arg(&mut args, "--pin")?,
            "--read-timeout-ms" => {
                read_timeout =
                    Duration::from_millis(parse_u64_arg(&take_arg(&mut args, "--read-timeout-ms")?)?)
            }
            "--action-timeout-ms" => {
                action_timeout =
                    Duration::from_millis(parse_u64_arg(&take_arg(&mut args, "--action-timeout-ms")?)?)
            }
            "--open-wait-ms" => {
                open_wait =
                    Duration::from_millis(parse_u64_arg(&take_arg(&mut args, "--open-wait-ms")?)?)
            }
            "--no-auto-init" => auto_init = false,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let mode = mode.ok_or_else(|| anyhow!("missing --mode"))?;
    ensure!(
        !(fee_payer_device && fee_payer_file.is_some()),
        "choose exactly one outer fee payer: --fee-payer-device or --fee-payer-file"
    );
    Ok(Config {
        mode,
        rpc_url,
        fee_payer_file,
        fee_payer_device,
        vector_program_id,
        vault_program_id,
        commitment,
        json,
        manifest_file,
        manifest_only,
        aliases_file,
        vault_seed,
        vault,
        vault_alias,
        member,
        member_alias,
        proposal_id,
        proposal_state_filter,
        proposal_queue_filter,
        action_kind_filter,
        threshold,
        proposal_ttl_slots,
        timelock_slots,
        vault_members,
        receiver,
        lamports,
        spl_mint,
        spl_destination,
        spl_amount,
        spl_decimals,
        replace_member_index,
        new_member_pda,
        new_member_alias,
        proposal_offset_slots,
        not_before_slot,
        expires_at_slot,
        member_fixture_file,
        output_file,
        overwrite,
        port,
        pin,
        read_timeout,
        action_timeout,
        open_wait,
        auto_init,
    })
}

fn take_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("missing value after {flag}"))
}

fn parse_commitment(raw: &str) -> Result<CommitmentConfig> {
    match raw {
        "processed" => Ok(CommitmentConfig::processed()),
        "confirmed" => Ok(CommitmentConfig::confirmed()),
        "finalized" => Ok(CommitmentConfig::finalized()),
        _ => bail!("unsupported commitment: {raw}"),
    }
}

fn parse_pubkey_arg(raw: &str) -> Result<Pubkey> {
    Pubkey::from_str(raw).with_context(|| format!("invalid pubkey: {raw}"))
}

fn parse_pubkey_csv(raw: &str) -> Result<Vec<Pubkey>> {
    raw.split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| parse_pubkey_arg(item.trim()))
        .collect()
}

fn parse_u64_arg(raw: &str) -> Result<u64> {
    raw.parse::<u64>()
        .with_context(|| format!("invalid integer value: {raw}"))
}

fn parse_u8_arg(raw: &str) -> Result<u8> {
    raw.parse::<u8>()
        .with_context(|| format!("invalid integer value: {raw}"))
}

fn parse_action_kind_filter(raw: &str) -> Result<Option<u8>> {
    match raw {
        "all" => Ok(None),
        "sol-withdraw" => Ok(Some(ACTION_KIND_SOL_WITHDRAW)),
        "spl-transfer" => Ok(Some(ACTION_KIND_SPL_TRANSFER)),
        "config-update" => Ok(Some(ACTION_KIND_UPDATE_CONFIG)),
        "replace-member" => Ok(Some(ACTION_KIND_REPLACE_MEMBER)),
        _ => bail!("unsupported --action-kind value: {raw}"),
    }
}

fn fee_payer_keypair(config: &Config) -> Result<Keypair> {
    let path = config
        .fee_payer_file
        .as_ref()
        .ok_or_else(|| anyhow!("--fee-payer-file is required for this mode"))?;
    read_keypair_file(path)
        .map_err(|err| anyhow!("failed to read fee payer keypair {}: {err}", path.display()))
}

fn resolve_fee_payer(config: &Config, device_pubkey_hint: Option<Pubkey>) -> Result<FeePayer> {
    if config.fee_payer_device {
        let pubkey = if let Some(pubkey) = device_pubkey_hint {
            pubkey
        } else {
            resolve_device_fee_payer_pubkey(config)?
        };
        return Ok(FeePayer::Device { pubkey });
    }

    Ok(FeePayer::Local(fee_payer_keypair(config)?))
}

fn resolve_device_fee_payer_pubkey(config: &Config) -> Result<Pubkey> {
    let mut port = open_serial_port(config)?;
    let info = get_info(&mut *port, config.read_timeout)?;
    ensure!(
        info.get("AUTH_MODE").map(String::as_str) == Some("PIN"),
        "device is not in PIN auth mode"
    );
    unlock_device(&mut *port, &config.pin, config.read_timeout)?;
    get_se050_pubkey(&mut *port, config.read_timeout)
}

fn load_aliases(config: &Config) -> Result<Option<AliasesFile>> {
    let path = if let Some(path) = &config.aliases_file {
        Some(path.clone())
    } else {
        let default = PathBuf::from(DEFAULT_ALIASES_FILE);
        default.exists().then_some(default)
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read aliases file {}", path.display()))?;
    let aliases: AliasesFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse aliases file {}", path.display()))?;
    Ok(Some(aliases))
}

fn aliases_path(config: &Config) -> Option<PathBuf> {
    config.aliases_file.clone().or_else(|| {
        let default = PathBuf::from(DEFAULT_ALIASES_FILE);
        default.exists().then_some(default)
    })
}

fn resolve_vault_alias(config: &Config, alias: &str) -> Result<Pubkey> {
    let aliases = load_aliases(config)?
        .ok_or_else(|| anyhow!("--vault-alias requires --aliases-file or {}", DEFAULT_ALIASES_FILE))?;
    let entry = aliases
        .vaults
        .get(alias)
        .ok_or_else(|| anyhow!("unknown vault alias: {alias}"))?;
    if let Some(pubkey) = &entry.pubkey {
        return parse_pubkey_arg(pubkey)
            .with_context(|| format!("vault alias {alias} has invalid pubkey"));
    }
    if let Some(seed) = &entry.seed {
        let (vault, _bump) = find_vault_pda(&config.vault_program_id, seed.as_bytes());
        return Ok(vault);
    }
    bail!("vault alias {alias} must define either pubkey or seed")
}

fn resolve_member_alias_entry(config: &Config, alias: &str) -> Result<MemberAliasEntry> {
    let aliases = load_aliases(config)?
        .ok_or_else(|| anyhow!("--member-alias requires --aliases-file or {}", DEFAULT_ALIASES_FILE))?;
    aliases
        .members
        .get(alias)
        .cloned()
        .ok_or_else(|| anyhow!("unknown member alias: {alias}"))
}

fn resolve_vault(config: &Config) -> Result<Pubkey> {
    if let Some(vault) = config.vault {
        Ok(vault)
    } else if let Some(alias) = &config.vault_alias {
        resolve_vault_alias(config, alias)
    } else if let Some(seed) = &config.vault_seed {
        let (vault, _bump) = find_vault_pda(&config.vault_program_id, seed);
        Ok(vault)
    } else {
        bail!("one of --vault, --vault-alias, or --vault-seed is required for this mode")
    }
}

fn open_member_session(config: &Config) -> Result<MemberSession> {
    let alias_fixture = if config.member_fixture_file.is_none() && config.port.is_none() {
        if let Some(alias) = &config.member_alias {
            resolve_member_alias_entry(config, alias)?
                .fixture_file
                .map(PathBuf::from)
        } else {
            None
        }
    } else {
        None
    };
    let fixture_path = config.member_fixture_file.clone().or(alias_fixture);

    match (&fixture_path, &config.port) {
        (Some(path), None) => load_fixture_member(config, path).map(MemberSession::Fixture),
        (None, Some(_)) => open_device_member(config).map(MemberSession::Device),
        (Some(_), Some(_)) => {
            bail!("choose exactly one member auth backend: --member-fixture-file or --port")
        }
        (None, None) => {
            if config.member_alias.is_some() {
                bail!("member alias does not define fixture_file; pass --port or --member-fixture-file")
            }
            bail!("this mode requires --member-fixture-file, --member-alias, or --port")
        }
    }
}

fn run_generate_member_fixture(config: &Config) -> Result<()> {
    let output = config
        .output_file
        .clone()
        .or_else(|| config.member_fixture_file.clone())
        .ok_or_else(|| anyhow!("generate-member-fixture requires --output-file or --member-fixture-file"))?;
    if output.exists() && !config.overwrite {
        bail!(
            "{} already exists; rerun with --overwrite to replace it",
            output.display()
        );
    }

    let (public_key, secret_key) = falcon512::keypair();
    let wire_pubkey = wire_pubkey(&public_key);
    let wire_pubkey_b64 = STANDARD.encode(wire_pubkey);
    let secret_key_b64 = STANDARD.encode(secret_key.as_bytes());
    let identity = falcon_identity(&wire_pubkey);
    let (vector_pda, vector_bump) = find_vector_pda(&config.vector_program_id, &identity);

    let fixture = MemberFixtureFile {
        wire_pubkey_b64,
        secret_key_b64,
    };
    let json = serde_json::to_string_pretty(&fixture).context("failed to serialize fixture JSON")?;
    fs::write(&output, json)
        .with_context(|| format!("failed to write fixture file {}", output.display()))?;

    println!("Member fixture");
    println!("Output file:          {}", output.display());
    println!("Vector PDA:           {}", vector_pda);
    println!("Vector bump:          {}", vector_bump);
    println!("Vector identity hex:  {}", hex::encode(identity));
    Ok(())
}

fn run_initialize_vector(client: &RpcClient, config: &Config) -> Result<()> {
    let mut member = open_member_session(config)?;
    let fee_payer = resolve_fee_payer(config, member.se050_pubkey())?;

    println!("Initialize Vector");
    println!("Fee payer:            {}", fee_payer.pubkey());
    print_member_header(&member);

    if let Some(account) = fetch_vector_account(
        client,
        &config.vector_program_id,
        &member.vector_pda(),
        config.commitment,
    )? {
        println!("Status:               already initialized");
        println!("Nonce hex:            {}", hex::encode(account.nonce));
        println!("Lamports:             {}", account.lamports);
        println!("Data length:          {}", account.data_len);
        return Ok(());
    }

    let ix = create_initialize_falcon512(
        config.vector_program_id,
        fee_payer.pubkey(),
        member.wire_pubkey(),
    )?;
    let signature = send_transaction(
        client,
        config,
        &fee_payer,
        fee_payer
            .device_pubkey()
            .and_then(|pubkey| member.matching_device_port(pubkey)),
        &[ix],
    )?;
    let account = fetch_vector_account(
        client,
        &config.vector_program_id,
        &member.vector_pda(),
        config.commitment,
    )?
        .ok_or_else(|| anyhow!("Vector account is still missing after initialize"))?;

    println!("Initialize tx:        {}", signature);
    println!("Nonce hex:            {}", hex::encode(account.nonce));
    println!("Lamports:             {}", account.lamports);
    println!("Data length:          {}", account.data_len);
    Ok(())
}

fn run_init_vault(client: &RpcClient, config: &Config) -> Result<()> {
    let fee_payer = resolve_fee_payer(config, None)?;
    let seed = config
        .vault_seed
        .as_ref()
        .ok_or_else(|| anyhow!("init-vault requires --vault-seed"))?;
    ensure!(
        !config.vault_members.is_empty() && config.vault_members.len() <= MAX_MEMBERS,
        "init-vault requires --vault-members with 1..={} PDAs",
        MAX_MEMBERS
    );
    let threshold = config
        .threshold
        .ok_or_else(|| anyhow!("init-vault requires --threshold"))?;
    let proposal_ttl_slots = config
        .proposal_ttl_slots
        .ok_or_else(|| anyhow!("init-vault requires --proposal-ttl-slots"))?;
    let timelock_slots = config
        .timelock_slots
        .ok_or_else(|| anyhow!("init-vault requires --timelock-slots"))?;
    ensure!(
        threshold > 0 && threshold as usize <= config.vault_members.len(),
        "threshold must be between 1 and member count"
    );
    for (index, member) in config.vault_members.iter().enumerate() {
        ensure!(*member != Pubkey::default(), "vault member {index} is the zero pubkey");
        ensure!(
            !config.vault_members[..index].contains(member),
            "vault member {member} is duplicated in --vault-members"
        );
        fetch_vector_account(
            client,
            &config.vector_program_id,
            member,
            config.commitment,
        )?
        .ok_or_else(|| {
            anyhow!(
                "vault member {member} is not an initialized Falcon Vector account owned by {}",
                config.vector_program_id
            )
        })?;
    }

    let (vault, bump) = find_vault_pda(&config.vault_program_id, seed);
    println!("Initialize Vault");
    println!("RPC URL:              {}", config.rpc_url);
    println!("Fee payer:            {}", fee_payer.pubkey());
    println!("Vault seed:           {}", String::from_utf8_lossy(seed));
    println!("Vault PDA:            {}", vault);
    println!("Vault bump:           {}", bump);

    if account_exists(client, &vault, config.commitment)? {
        bail!("vault {} already exists", vault);
    }

    let mut members = [Pubkey::default(); MAX_MEMBERS];
    for (index, member) in config.vault_members.iter().enumerate() {
        members[index] = *member;
    }
    let ix = create_initialize_vault_instruction(
        config.vault_program_id,
        fee_payer.pubkey(),
        seed,
        threshold,
        proposal_ttl_slots,
        timelock_slots,
        &members[..config.vault_members.len()],
    )?;
    let signature = send_transaction(client, config, &fee_payer, None, &[ix])?;
    let snapshot = read_vault_snapshot(client, vault)?;

    println!("Initialize tx:        {}", signature);
    print_vault_snapshot(vault, &snapshot);
    Ok(())
}

fn run_show_vault(client: &RpcClient, config: &Config) -> Result<()> {
    let vault = resolve_vault(config)?;
    let snapshot = read_vault_snapshot(client, vault)?;
    if config.json {
        print_json(&vault_snapshot_json(vault, &snapshot))?;
    } else {
        println!("Vault");
        print_vault_snapshot(vault, &snapshot);
    }
    Ok(())
}

fn run_list_proposals(client: &RpcClient, config: &Config) -> Result<()> {
    let vault = resolve_vault(config)?;
    let vault_snapshot = read_vault_snapshot(client, vault)?;
    let current_slot = client
        .get_slot()
        .context("failed to fetch current slot for proposal listing")?;
    let member_pda = resolve_list_member_pda(config)?;
    if config.proposal_queue_filter == ProposalQueueFilter::NeedsApproval {
        ensure!(
            member_pda.is_some(),
            "list-proposals --queue needs-approval requires --member or a local member auth backend"
        );
    }

    let mut entries = Vec::new();
    let mut matches = 0usize;
    for proposal_id in 0..vault_snapshot.next_proposal_id {
        let proposal = match read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id) {
            Ok(proposal) => proposal,
            Err(err) if is_missing_account_error(&err) => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed while loading proposal {proposal_id}"));
            }
        };
        let proposal_tx = read_proposal_tx_snapshot(client, proposal.tx_account)
            .with_context(|| format!("failed while loading proposal_tx for proposal {proposal_id}"))?;
        let effective_state = effective_proposal_state(&proposal, current_slot);
        if !matches_proposal_state_filter(config.proposal_state_filter, effective_state) {
            continue;
        }
        if let Some(action_kind) = config.action_kind_filter {
            if proposal.action_kind != action_kind {
                continue;
            }
        }

        let proposal_view =
            build_proposal_list_view(&vault_snapshot, &proposal, effective_state, current_slot, member_pda);
        if !matches_proposal_queue_filter(config.proposal_queue_filter, proposal_view.queue_status) {
            continue;
        }

        matches += 1;
        let summary = action_summary(proposal.action_kind, &proposal_tx.payload)?;
        entries.push(proposal_list_entry_json(
            &proposal,
            proposal_view,
            &summary,
            vault_snapshot.threshold,
            vault_snapshot.config_version,
        ));
    }

    if config.json {
        print_json(&json!({
            "vault": vault.to_string(),
            "current_slot": current_slot,
            "current_config_version": vault_snapshot.config_version,
            "filters": {
                "state": config.proposal_state_filter.label(),
                "queue": config.proposal_queue_filter.label(),
                "action_kind": config.action_kind_filter.map(action_kind_label),
                "member": member_pda.map(|member| member.to_string()),
            },
            "matches": matches,
            "scanned_proposal_ids": vault_snapshot.next_proposal_id,
            "proposals": entries,
        }))?;
        return Ok(());
    }

    println!("Proposals");
    println!("Vault:                {}", vault);
    println!("Current slot:         {}", current_slot);
    println!("Current config:       {}", vault_snapshot.config_version);
    println!("Filter state:         {}", config.proposal_state_filter.label());
    println!("Filter queue:         {}", config.proposal_queue_filter.label());
    println!(
        "Filter action kind:   {}",
        config
            .action_kind_filter
            .map(action_kind_label)
            .unwrap_or("all")
    );
    println!(
        "Filter member:        {}",
        member_pda
            .map(|member| member.to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!();

    let mut headers = vec![
        "ID",
        "State",
        "Queue",
        "Kind",
        "Appr",
        "Cfg",
        "NotBefore",
        "Expires",
        "Summary",
    ];
    if member_pda.is_some() {
        headers.extend(["Mine", "Approved", "CanApprove"]);
    }

    let mut rows = Vec::new();
    for entry in &entries {
        let mut row = vec![
            entry["proposal_id"].as_u64().unwrap_or_default().to_string(),
            entry["state_label"].as_str().unwrap_or("Unknown").to_string(),
            entry["queue_status"].as_str().unwrap_or("unknown").to_string(),
            entry["action_kind_label"].as_str().unwrap_or("Unknown").to_string(),
            format!(
                "{}/{}",
                entry["approval_count"].as_u64().unwrap_or_default(),
                vault_snapshot.threshold
            ),
            format!(
                "{}/{}",
                entry["config_version"].as_u64().unwrap_or_default(),
                vault_snapshot.config_version
            ),
            entry["not_before_slot"].as_u64().unwrap_or_default().to_string(),
            entry["expires_at_slot"].as_u64().unwrap_or_default().to_string(),
            truncate_text(entry["summary"].as_str().unwrap_or("unknown"), 64),
        ];

        if member_pda.is_some() {
            let member_status = entry.get("member_status").and_then(Value::as_object);
            row.push(bool_word(
                member_status
                    .and_then(|status| status.get("is_active"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ));
            row.push(bool_word(
                member_status
                    .and_then(|status| status.get("has_approved"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ));
            row.push(bool_word(
                member_status
                    .and_then(|status| status.get("can_approve"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ));
        }
        rows.push(row);
    }

    if rows.is_empty() {
        println!("No proposals matched the current filters.");
    } else {
        print_text_table(&headers, &rows);
    }

    println!();
    println!("Matches:              {}", matches);
    println!("Scanned proposal ids: 0..{}", vault_snapshot.next_proposal_id);
    Ok(())
}

fn run_show_proposal(client: &RpcClient, config: &Config) -> Result<()> {
    let vault = resolve_vault(config)?;
    let proposal_id = config
        .proposal_id
        .ok_or_else(|| anyhow!("show-proposal requires --proposal-id"))?;
    let proposal = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;
    let proposal_tx = read_proposal_tx_snapshot(client, proposal.tx_account)?;

    if config.json {
        print_json(&proposal_snapshot_json(vault, &proposal, &proposal_tx)?)?;
    } else {
        println!("Proposal");
        print_proposal_snapshot(vault, &proposal, &proposal_tx)?;
    }
    Ok(())
}

fn run_create_sol_withdraw(client: &RpcClient, config: &Config) -> Result<()> {
    let receiver = config
        .receiver
        .ok_or_else(|| anyhow!("create-sol-withdraw requires --receiver"))?;
    let lamports = config
        .lamports
        .ok_or_else(|| anyhow!("create-sol-withdraw requires --lamports"))?;
    let payload = create_sol_withdraw_action_payload(receiver, lamports);
    run_create_action_proposal(
        client,
        config,
        ACTION_KIND_SOL_WITHDRAW,
        payload,
        "SOL withdraw",
    )
}

fn run_create_spl_transfer(client: &RpcClient, config: &Config) -> Result<()> {
    let mint = config
        .spl_mint
        .ok_or_else(|| anyhow!("create-spl-transfer requires --spl-mint"))?;
    let destination = config
        .spl_destination
        .ok_or_else(|| anyhow!("create-spl-transfer requires --spl-destination"))?;
    let amount = config
        .spl_amount
        .ok_or_else(|| anyhow!("create-spl-transfer requires --spl-amount"))?;
    let decimals = config
        .spl_decimals
        .ok_or_else(|| anyhow!("create-spl-transfer requires --spl-decimals"))?;
    let payload = create_spl_transfer_action_payload(mint, destination, amount, decimals);
    run_create_action_proposal(
        client,
        config,
        ACTION_KIND_SPL_TRANSFER,
        payload,
        "SPL transfer",
    )
}

fn run_create_config_update(client: &RpcClient, config: &Config) -> Result<()> {
    let threshold = config
        .threshold
        .ok_or_else(|| anyhow!("create-config-update requires --threshold"))?;
    let proposal_ttl_slots = config
        .proposal_ttl_slots
        .ok_or_else(|| anyhow!("create-config-update requires --proposal-ttl-slots"))?;
    let timelock_slots = config
        .timelock_slots
        .ok_or_else(|| anyhow!("create-config-update requires --timelock-slots"))?;
    let payload = create_update_config_action_payload(threshold, proposal_ttl_slots, timelock_slots);
    run_create_action_proposal(
        client,
        config,
        ACTION_KIND_UPDATE_CONFIG,
        payload,
        "Config update",
    )
}

fn run_create_replace_member(client: &RpcClient, config: &Config) -> Result<()> {
    let member_index = config
        .replace_member_index
        .ok_or_else(|| anyhow!("create-replace-member requires --replace-member-index"))?;
    let new_member_pda = resolve_new_member_pda(config)?;
    let payload = create_replace_member_action_payload(member_index, new_member_pda);
    run_create_action_proposal(
        client,
        config,
        ACTION_KIND_REPLACE_MEMBER,
        payload,
        "Replace member",
    )
}

fn run_create_action_proposal(
    client: &RpcClient,
    config: &Config,
    action_kind: u8,
    payload: Vec<u8>,
    label: &str,
) -> Result<()> {
    let vault = resolve_vault(config)?;
    let vault_snapshot = read_vault_snapshot(client, vault)?;
    let proposal_id = vault_snapshot.next_proposal_id;
    let mut member = open_member_session(config)?;
    let fee_payer = resolve_fee_payer(config, member.se050_pubkey())?;
    let current_slot = client
        .get_slot()
        .context("failed to fetch current slot before proposal creation")?;
    let proposal_offset_slots = config.proposal_offset_slots.unwrap_or_else(|| {
        if config.port.is_some() {
            DEFAULT_DEVICE_PROPOSAL_SLOT_MARGIN
        } else {
            DEFAULT_PROPOSAL_SLOT_MARGIN
        }
    });
    let not_before_slot = config
        .not_before_slot
        .unwrap_or_else(|| {
            current_slot
                .saturating_add(vault_snapshot.timelock_slots)
                .saturating_add(proposal_offset_slots)
        });
    let expires_at_slot = config
        .expires_at_slot
        .unwrap_or_else(|| not_before_slot.saturating_add(vault_snapshot.proposal_ttl_slots));
    ensure!(
        expires_at_slot >= not_before_slot,
        "expires-at slot cannot be earlier than not-before slot"
    );

    let action_hash = compute_action_hash(
        &vault,
        vault_snapshot.config_version,
        proposal_id,
        action_kind,
        not_before_slot,
        expires_at_slot,
        &payload,
    );
    let sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        member.vector_pda(),
        fee_payer.pubkey(),
        vault,
        proposal_id,
        action_kind,
        not_before_slot,
        expires_at_slot,
        action_hash,
        &payload,
    );
    let result = submit_member_passthrough(client, &fee_payer, config, &mut member, &[sub_ix])?;
    let proposal = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;

    println!("Create Proposal");
    println!("Action:               {}", label);
    println!("Fee payer:            {}", fee_payer.pubkey());
    print_member_header(&member);
    println!("Vault:                {}", vault);
    println!("Proposal ID:          {}", proposal_id);
    println!("Action Kind:          {} ({})", action_kind, action_kind_label(action_kind));
    println!("Action Hash Hex:      {}", hex::encode(action_hash));
    println!("Not-before slot:      {}", not_before_slot);
    println!("Expires-at slot:      {}", expires_at_slot);
    println!("Create tx:            {}", result.tx_signature);
    println!("Advance digest hex:   {}", hex::encode(result.vector_digest));
    println!("Proposal state:       {}", proposal_state_label(proposal.state));
    println!("Approval count:       {}", proposal.approval_count);
    Ok(())
}

fn run_member_approval_action(
    client: &RpcClient,
    config: &Config,
    label: &str,
    builder: impl Fn(Pubkey, Pubkey, Pubkey, u64, [u8; 32]) -> Instruction,
) -> Result<()> {
    let vault = resolve_vault(config)?;
    let proposal_id = config
        .proposal_id
        .ok_or_else(|| anyhow!("{label} requires --proposal-id"))?;
    let proposal = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;
    let mut member = open_member_session(config)?;
    let fee_payer = resolve_fee_payer(config, member.se050_pubkey())?;
    let sub_ix = builder(
        config.vault_program_id,
        member.vector_pda(),
        vault,
        proposal_id,
        proposal.action_hash,
    );
    let result = submit_member_passthrough(client, &fee_payer, config, &mut member, &[sub_ix])?;
    let updated = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;

    println!("{label} Proposal");
    println!("Fee payer:            {}", fee_payer.pubkey());
    print_member_header(&member);
    println!("Vault:                {}", vault);
    println!("Proposal ID:          {}", proposal_id);
    println!("Action Hash Hex:      {}", hex::encode(proposal.action_hash));
    println!("{label} tx:            {}", result.tx_signature);
    println!("Advance digest hex:   {}", hex::encode(result.vector_digest));
    println!("Proposal state:       {}", proposal_state_label(updated.state));
    println!("Approval count:       {}", updated.approval_count);
    Ok(())
}

fn run_accept_pending_member(client: &RpcClient, config: &Config) -> Result<()> {
    let vault = resolve_vault(config)?;
    let before = read_vault_snapshot(client, vault)?;
    ensure!(
        before.pending_member_index != NO_PENDING_MEMBER_INDEX,
        "vault has no pending member to accept"
    );
    let mut member = open_member_session(config)?;
    let fee_payer = resolve_fee_payer(config, member.se050_pubkey())?;
    let pending_index = before.pending_member_index as usize;
    ensure!(
        member.vector_pda() == before.pending_member_pda,
        "connected member {} does not match pending member {} at index {}",
        member.vector_pda(),
        before.pending_member_pda,
        pending_index
    );
    let sub_ix =
        create_accept_pending_member_instruction(config.vault_program_id, member.vector_pda(), vault);
    let result = submit_member_passthrough(client, &fee_payer, config, &mut member, &[sub_ix])?;
    let after = read_vault_snapshot(client, vault)?;

    println!("Accept Pending Member");
    println!("Fee payer:            {}", fee_payer.pubkey());
    print_member_header(&member);
    println!("Vault:                {}", vault);
    println!("Pending index:        {}", pending_index);
    println!("Accept tx:            {}", result.tx_signature);
    println!("Advance digest hex:   {}", hex::encode(result.vector_digest));
    println!(
        "Config version:       {} -> {}",
        before.config_version, after.config_version
    );
    println!(
        "Pending member index: {} -> {}",
        before.pending_member_index, after.pending_member_index
    );
    Ok(())
}

fn run_execute(client: &RpcClient, config: &Config) -> Result<()> {
    let fee_payer = resolve_fee_payer(config, None)?;
    let vault = resolve_vault(config)?;
    let proposal_id = config
        .proposal_id
        .ok_or_else(|| anyhow!("execute requires --proposal-id"))?;
    let proposal = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;
    let proposal_tx = read_proposal_tx_snapshot(client, proposal.tx_account)?;
    ensure!(
        proposal.action_hash == proposal_tx.action_hash,
        "proposal action hash does not match proposal_tx header"
    );

    let extra_accounts = derive_execute_accounts(vault, proposal.action_kind, &proposal_tx.payload)?;
    let ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault,
        proposal_id,
        proposal.action_hash,
        &extra_accounts,
    );
    let signature = send_transaction(client, config, &fee_payer, None, &[ix])?;
    let updated = read_proposal_snapshot(client, &config.vault_program_id, vault, proposal_id)?;

    println!("Execute Proposal");
    println!("Fee payer:            {}", fee_payer.pubkey());
    println!("Vault:                {}", vault);
    println!("Proposal ID:          {}", proposal_id);
    println!("Action Kind:          {} ({})", proposal.action_kind, action_kind_label(proposal.action_kind));
    println!("Action Hash Hex:      {}", hex::encode(proposal.action_hash));
    println!("Execute tx:           {}", signature);
    println!("Proposal state:       {}", proposal_state_label(updated.state));
    println!("Executed slot:        {}", updated.executed_slot);
    Ok(())
}

fn run_close(client: &RpcClient, config: &Config) -> Result<()> {
    let fee_payer = resolve_fee_payer(config, None)?;
    let vault = resolve_vault(config)?;
    let proposal_id = config
        .proposal_id
        .ok_or_else(|| anyhow!("close requires --proposal-id"))?;
    let (proposal_pda, _) = find_proposal_pda(&config.vault_program_id, &vault, proposal_id);
    let (proposal_tx_pda, _) = find_proposal_tx_pda(&config.vault_program_id, &proposal_pda);
    let ix = create_close_proposal_instruction(config.vault_program_id, vault, proposal_id);
    let signature = send_transaction(client, config, &fee_payer, None, &[ix])?;

    println!("Close Proposal");
    println!("Fee payer:            {}", fee_payer.pubkey());
    println!("Vault:                {}", vault);
    println!("Proposal ID:          {}", proposal_id);
    println!("Close tx:             {}", signature);
    println!(
        "Proposal account:     {}",
        if account_exists(client, &proposal_pda, config.commitment)? {
            "still present"
        } else {
            "closed"
        }
    );
    println!(
        "Proposal tx account:  {}",
        if account_exists(client, &proposal_tx_pda, config.commitment)? {
            "still present"
        } else {
            "closed"
        }
    );
    Ok(())
}

fn print_member_header(member: &MemberSession) {
    println!("Member label:         {}", member.label());
    if let Some(se050_pubkey) = member.se050_pubkey() {
        println!("SE050 pubkey:         {}", se050_pubkey);
    }
    println!("Vector PDA:           {}", member.vector_pda());
    println!("Vector identity hex:  {}", hex::encode(member.identity()));
}

impl MemberSession {
    fn label(&self) -> String {
        match self {
            Self::Device(session) => format!("device:{} ({})", session.se050_pubkey, session.vector_pda),
            Self::Fixture(session) => {
                format!("fixture:{} ({})", session.fixture_path.display(), session.vector_pda)
            }
        }
    }

    fn se050_pubkey(&self) -> Option<Pubkey> {
        match self {
            Self::Device(session) => Some(session.se050_pubkey),
            Self::Fixture(_) => None,
        }
    }

    fn wire_pubkey(&self) -> &[u8; FALCON_512_PUBKEY_LEN] {
        match self {
            Self::Device(session) => &session.wire_pubkey,
            Self::Fixture(session) => &session.wire_pubkey,
        }
    }

    fn identity(&self) -> &[u8; 32] {
        match self {
            Self::Device(session) => &session.identity,
            Self::Fixture(session) => &session.identity,
        }
    }

    fn vector_pda(&self) -> Pubkey {
        match self {
            Self::Device(session) => session.vector_pda,
            Self::Fixture(session) => session.vector_pda,
        }
    }

    fn sign_digest(&mut self, digest: &[u8; 32]) -> Result<[u8; FALCON_512_SIGNATURE_LEN]> {
        match self {
            Self::Device(session) => session.sign_digest(digest),
            Self::Fixture(session) => Ok(sign_falcon_digest(digest, &session.secret_key)),
        }
    }

    fn matching_device_port(&mut self, device_pubkey: Pubkey) -> Option<&mut dyn SerialPort> {
        match self {
            Self::Device(session) if session.se050_pubkey == device_pubkey => Some(&mut *session.port),
            _ => None,
        }
    }
}

impl DeviceMemberSession {
    fn sign_digest(&mut self, digest: &[u8; 32]) -> Result<[u8; FALCON_512_SIGNATURE_LEN]> {
        unlock_device(&mut *self.port, &self.pin, self.read_timeout)?;
        let response = send_command(
            &mut *self.port,
            &format!("PQC_SIGN_VECTOR_DIGEST:{}", STANDARD.encode(digest)),
            self.action_timeout,
        )?;
        let signature_vec = STANDARD
            .decode(prefixed_value(&response, "PQC_VECTOR_SIGNATURE:")?)
            .context("failed to decode PQC_VECTOR_SIGNATURE")?;
        let returned_identity = STANDARD
            .decode(prefixed_value(&response, "PQC_VECTOR_IDENTITY:")?)
            .context("failed to decode PQC_VECTOR_IDENTITY")?;
        let returned_pubkey = STANDARD
            .decode(prefixed_value(&response, "PQC_PUBKEY:")?)
            .context("failed to decode PQC_PUBKEY")?;

        ensure!(
            returned_identity.as_slice() == self.identity.as_slice(),
            "device returned a different Vector identity"
        );
        ensure!(
            returned_pubkey.as_slice() == self.wire_pubkey.as_slice(),
            "device returned a different Falcon pubkey"
        );
        ensure!(
            signature_vec.len() <= FALCON_512_SIGNATURE_LEN,
            "device returned an oversized Falcon signature: {} bytes",
            signature_vec.len()
        );

        let mut signature = [0u8; FALCON_512_SIGNATURE_LEN];
        signature[..signature_vec.len()].copy_from_slice(&signature_vec);
        verify_vector_signature(&self.wire_pubkey, digest, &signature)?;
        Ok(signature)
    }
}

fn open_device_member(config: &Config) -> Result<DeviceMemberSession> {
    let mut port = open_serial_port(config)?;
    let boot = read_response(&mut *port, Duration::from_millis(1_500))?;
    if !boot.is_empty() {
        println!("BOOT <<< {}", boot.replace('\n', " | "));
    }

    let info = get_info(&mut *port, config.read_timeout)?;
    ensure!(
        info.get("AUTH_MODE").map(String::as_str) == Some("PIN"),
        "device is not in PIN auth mode"
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
    let identity = falcon_identity(&pqc.wire_pubkey);
    let (vector_pda, _bump) = find_vector_pda(&config.vector_program_id, &identity);

    Ok(DeviceMemberSession {
        port,
        pin: config.pin.clone(),
        read_timeout: config.read_timeout,
        action_timeout: config.action_timeout,
        se050_pubkey,
        wire_pubkey: pqc.wire_pubkey,
        identity,
        vector_pda,
    })
}

fn load_fixture_member(config: &Config, path: &Path) -> Result<FixtureMemberSession> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read member fixture {}", path.display()))?;
    let fixture: MemberFixtureFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse member fixture {}", path.display()))?;
    let wire_pubkey_vec = STANDARD
        .decode(&fixture.wire_pubkey_b64)
        .context("failed to decode fixture wire_pubkey_b64")?;
    ensure!(
        wire_pubkey_vec.len() == FALCON_512_PUBKEY_LEN,
        "fixture wire pubkey length {} != {}",
        wire_pubkey_vec.len(),
        FALCON_512_PUBKEY_LEN
    );
    let mut wire_pubkey = [0u8; FALCON_512_PUBKEY_LEN];
    wire_pubkey.copy_from_slice(&wire_pubkey_vec);
    let secret_key_vec = STANDARD
        .decode(&fixture.secret_key_b64)
        .context("failed to decode fixture secret_key_b64")?;
    let secret_key =
        falcon512::SecretKey::from_bytes(&secret_key_vec).context("failed to decode Falcon secret key")?;
    let identity = falcon_identity(&wire_pubkey);
    let (vector_pda, _bump) = find_vector_pda(&config.vector_program_id, &identity);

    Ok(FixtureMemberSession {
        fixture_path: path.to_path_buf(),
        wire_pubkey,
        secret_key,
        identity,
        vector_pda,
    })
}

fn submit_member_passthrough(
    client: &RpcClient,
    fee_payer: &FeePayer,
    config: &Config,
    member: &mut MemberSession,
    subinstructions: &[Instruction],
) -> Result<MemberSubmitResult> {
    let vector_account = fetch_vector_account(
        client,
        &config.vector_program_id,
        &member.vector_pda(),
        config.commitment,
    )?
        .ok_or_else(|| anyhow!("member Vector account {} is missing; run initialize-vector first", member.vector_pda()))?;
    let normalized = normalize_passthrough_subinstructions(subinstructions);
    let passthrough_ix =
        create_passthrough_instruction(config.vector_program_id, member.identity(), &normalized)?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        member.identity(),
        &vector_account.nonce,
        &[],
        &[passthrough_ix.clone()],
        Some(fee_payer.pubkey()),
    )?;
    let signature = member.sign_digest(&digest)?;
    let advance_ix =
        create_advance_instruction(config.vector_program_id, member.identity(), &signature)?;
    let tx_signature = send_transaction(
        client,
        config,
        fee_payer,
        fee_payer
            .device_pubkey()
            .and_then(|pubkey| member.matching_device_port(pubkey)),
        &[advance_ix, passthrough_ix],
    )?;
    Ok(MemberSubmitResult {
        tx_signature,
        vector_digest: digest,
    })
}

fn normalize_passthrough_subinstructions(subinstructions: &[Instruction]) -> Vec<Instruction> {
    subinstructions
        .iter()
        .map(|ix| Instruction {
            program_id: ix.program_id,
            accounts: ix
                .accounts
                .iter()
                .map(|meta| {
                    let mut normalized = meta.clone();
                    normalized.is_signer = false;
                    normalized
                })
                .collect(),
            data: ix.data.clone(),
        })
        .collect()
}

fn send_transaction(
    client: &RpcClient,
    config: &Config,
    fee_payer: &FeePayer,
    device_port: Option<&mut dyn SerialPort>,
    instructions: &[Instruction],
) -> Result<Signature> {
    let blockhash = client
        .get_latest_blockhash()
        .context("failed to fetch latest blockhash")?;

    match fee_payer {
        FeePayer::Local(keypair) => {
            let tx = Transaction::new_signed_with_payer(
                instructions,
                Some(&keypair.pubkey()),
                &[keypair],
                blockhash,
            );
            client
                .send_and_confirm_transaction(&tx)
                .context("failed to send and confirm transaction")
        }
        FeePayer::Device { pubkey } => {
            let message = Message::new(instructions, Some(pubkey));
            let mut tx = Transaction::new_unsigned(message);
            tx.message.recent_blockhash = blockhash;

            let mut owned_port;
            let port: &mut dyn SerialPort = if let Some(port) = device_port {
                port
            } else {
                owned_port = open_device_fee_payer_port(config, *pubkey)?;
                &mut *owned_port
            };

            unlock_device(port, &config.pin, config.read_timeout)?;
            let signature =
                sign_message_with_se050(port, &tx.message.serialize(), config.action_timeout)?;
            ensure!(
                signature.verify(pubkey.as_ref(), &tx.message.serialize()),
                "device returned an invalid outer Solana signature"
            );
            set_transaction_signature(&mut tx, pubkey, signature)?;
            client
                .send_and_confirm_transaction(&tx)
                .context("failed to send and confirm transaction")
        }
    }
}

fn open_serial_port(config: &Config) -> Result<Box<dyn SerialPort>> {
    let port_path = config
        .port
        .as_ref()
        .ok_or_else(|| anyhow!("missing --port for device-backed operation"))?;
    let port = serialport::new(port_path, BAUD)
        .timeout(Duration::from_millis(50))
        .open()
        .with_context(|| format!("failed to open serial port {}", port_path))?;

    sleep(config.open_wait);
    port.clear(ClearBuffer::Input).ok();
    Ok(port)
}

fn open_device_fee_payer_port(config: &Config, expected_pubkey: Pubkey) -> Result<Box<dyn SerialPort>> {
    let mut port = open_serial_port(config)?;
    let info = get_info(&mut *port, config.read_timeout)?;
    ensure!(
        info.get("AUTH_MODE").map(String::as_str) == Some("PIN"),
        "device is not in PIN auth mode"
    );
    unlock_device(&mut *port, &config.pin, config.read_timeout)?;
    let actual_pubkey = get_se050_pubkey(&mut *port, config.read_timeout)?;
    ensure!(
        actual_pubkey == expected_pubkey,
        "connected device {} does not match fee payer {}",
        actual_pubkey,
        expected_pubkey
    );
    Ok(port)
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
    vector_program_id: &Pubkey,
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
        account.owner == *vector_program_id,
        "Vector account {vector_pda} is owned by {}, expected {}",
        account.owner,
        vector_program_id
    );
    ensure!(
        account.data.len() == VECTOR_FALCON_ACCOUNT_LEN,
        "Vector account {vector_pda} has invalid length: {} bytes (expected {})",
        account.data.len(),
        VECTOR_FALCON_ACCOUNT_LEN
    );
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&account.data[..32]);
    Ok(Some(VectorAccountState {
        nonce,
        lamports: account.lamports,
        data_len: account.data.len(),
    }))
}

struct VectorAccountState {
    nonce: [u8; 32],
    lamports: u64,
    data_len: usize,
}

fn account_exists(client: &RpcClient, account: &Pubkey, commitment: CommitmentConfig) -> Result<bool> {
    Ok(client
        .get_account_with_commitment(account, commitment)
        .with_context(|| format!("failed to fetch account {account}"))?
        .value
        .is_some())
}

fn read_vault_snapshot(client: &RpcClient, vault: Pubkey) -> Result<VaultSnapshot> {
    let data = client
        .get_account_data(&vault)
        .with_context(|| format!("failed to fetch vault account data for {vault}"))?;
    ensure!(data.len() >= VAULT_ACCOUNT_LEN, "vault account data too short");
    let mut members = [VaultMemberSnapshot::default(); MAX_MEMBERS];
    let mut offset = 104usize;
    for member in &mut members {
        member.member_pda = read_pubkey_at(&data, offset)?;
        member.enabled = *data
            .get(offset + 32)
            .context("vault member enabled flag out of bounds")?
            != 0;
        member.weight = *data
            .get(offset + 33)
            .context("vault member weight out of bounds")?;
        offset += MEMBER_RECORD_LEN;
    }
    Ok(VaultSnapshot {
        threshold: *data.get(2).context("vault threshold out of bounds")?,
        member_count: *data.get(3).context("vault member count out of bounds")?,
        pending_member_index: *data.get(5).context("vault pending member index out of bounds")?,
        pending_member_pda: read_pubkey_at(&data, 40)?,
        config_version: read_u64_at(&data, 8)?,
        next_proposal_id: read_u64_at(&data, 16)?,
        timelock_slots: read_u64_at(&data, 24)?,
        proposal_ttl_slots: read_u64_at(&data, 32)?,
        members,
    })
}

fn read_proposal_snapshot(
    client: &RpcClient,
    vault_program_id: &Pubkey,
    vault: Pubkey,
    proposal_id: u64,
) -> Result<ProposalSnapshot> {
    let (proposal, _) = find_proposal_pda(vault_program_id, &vault, proposal_id);
    let data = client
        .get_account_data(&proposal)
        .with_context(|| format!("failed to fetch proposal account data for {proposal}"))?;
    ensure!(data.len() >= PROPOSAL_ACCOUNT_LEN, "proposal account data too short");
    let _vault = read_pubkey_at(&data, 16)?;
    Ok(ProposalSnapshot {
        state: data[1],
        proposer_member_index: data[2],
        approval_bitmap: data[3],
        approval_count: data[4],
        action_kind: data[5],
        proposal_id: read_u64_at(&data, 8)?,
        config_version: read_u64_at(&data, 48)?,
        created_slot: read_u64_at(&data, 56)?,
        not_before_slot: read_u64_at(&data, 64)?,
        expires_at_slot: read_u64_at(&data, 72)?,
        action_hash: read_array32_at(&data, 80)?,
        tx_account: read_pubkey_at(&data, 112)?,
        executed_slot: read_u64_at(&data, 144)?,
    })
}

fn read_proposal_tx_snapshot(client: &RpcClient, proposal_tx: Pubkey) -> Result<ProposalTxSnapshot> {
    let data = client
        .get_account_data(&proposal_tx)
        .with_context(|| format!("failed to fetch proposal_tx account data for {proposal_tx}"))?;
    ensure!(
        data.len() >= PROPOSAL_TX_HEADER_LEN,
        "proposal_tx account data too short"
    );
    let action_hash = read_array32_at(&data, 4)?;
    let data_len = read_u32_at(&data, 36)? as usize;
    let payload_end = PROPOSAL_TX_HEADER_LEN
        .checked_add(data_len)
        .ok_or_else(|| anyhow!("proposal_tx payload length overflowed"))?;
    let payload = data
        .get(PROPOSAL_TX_HEADER_LEN..payload_end)
        .ok_or_else(|| anyhow!("proposal_tx payload truncated"))?
        .to_vec();
    Ok(ProposalTxSnapshot { action_hash, payload })
}

fn print_vault_snapshot(vault: Pubkey, snapshot: &VaultSnapshot) {
    println!("Vault PDA:            {}", vault);
    println!("Threshold:            {}", snapshot.threshold);
    println!("Member count:         {}", snapshot.member_count);
    println!("Config version:       {}", snapshot.config_version);
    println!("Next proposal id:     {}", snapshot.next_proposal_id);
    println!("Timelock slots:       {}", snapshot.timelock_slots);
    println!("Proposal TTL slots:   {}", snapshot.proposal_ttl_slots);
    if snapshot.pending_member_index == NO_PENDING_MEMBER_INDEX {
        println!("Pending member:       none");
    } else {
        println!(
            "Pending member:       index={} pda={}",
            snapshot.pending_member_index, snapshot.pending_member_pda
        );
    }
    println!();
    println!("Members");
    let member_rows = snapshot
        .members
        .iter()
        .take(snapshot.member_count as usize)
        .enumerate()
        .map(|(index, member)| {
            vec![
                index.to_string(),
                member.member_pda.to_string(),
                bool_word(member.enabled),
                member.weight.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    print_text_table(&["Index", "Member PDA", "Enabled", "Weight"], &member_rows);
}

fn print_proposal_snapshot(
    vault: Pubkey,
    proposal: &ProposalSnapshot,
    proposal_tx: &ProposalTxSnapshot,
) -> Result<()> {
    println!("Vault:                {}", vault);
    println!("Proposal ID:          {}", proposal.proposal_id);
    println!("Proposal state:       {}", proposal_state_label(proposal.state));
    println!("Action kind:          {} ({})", proposal.action_kind, action_kind_label(proposal.action_kind));
    println!("Proposer index:       {}", proposal.proposer_member_index);
    println!("Approval bitmap:      0b{:03b}", proposal.approval_bitmap);
    println!("Approval count:       {}", proposal.approval_count);
    println!("Config version:       {}", proposal.config_version);
    println!("Created slot:         {}", proposal.created_slot);
    println!("Not-before slot:      {}", proposal.not_before_slot);
    println!("Expires-at slot:      {}", proposal.expires_at_slot);
    println!("Executed slot:        {}", proposal.executed_slot);
    println!("Action hash hex:      {}", hex::encode(proposal.action_hash));
    println!("Proposal tx account:  {}", proposal.tx_account);
    ensure!(
        proposal.action_hash == proposal_tx.action_hash,
        "proposal action hash does not match proposal_tx header"
    );
    print_decoded_action_payload(proposal.action_kind, &proposal_tx.payload)?;
    Ok(())
}

fn resolve_list_member_pda(config: &Config) -> Result<Option<Pubkey>> {
    if let Some(member) = config.member {
        return Ok(Some(member));
    }
    if let Some(alias) = &config.member_alias {
        let entry = resolve_member_alias_entry(config, alias)?;
        return Ok(Some(parse_pubkey_arg(&entry.vector_pda)?));
    }
    if config.member_fixture_file.is_none() && config.port.is_none() {
        return Ok(None);
    }
    Ok(Some(open_member_session(config)?.vector_pda()))
}

fn resolve_new_member_pda(config: &Config) -> Result<Pubkey> {
    if let Some(new_member_pda) = config.new_member_pda {
        return Ok(new_member_pda);
    }
    if let Some(alias) = &config.new_member_alias {
        let entry = resolve_member_alias_entry(config, alias)?;
        return parse_pubkey_arg(&entry.vector_pda)
            .with_context(|| format!("new member alias {alias} has invalid vector_pda"));
    }
    bail!("create-replace-member requires --new-member-pda or --new-member-alias")
}

fn is_missing_account_error(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains("AccountNotFound")
}

fn effective_proposal_state(
    proposal: &ProposalSnapshot,
    current_slot: u64,
) -> EffectiveProposalState {
    match proposal.state {
        PROPOSAL_STATE_EXECUTED => EffectiveProposalState::Executed,
        PROPOSAL_STATE_CANCELLED => EffectiveProposalState::Cancelled,
        PROPOSAL_STATE_EXPIRED => EffectiveProposalState::Expired,
        PROPOSAL_STATE_ACTIVE if current_slot > proposal.expires_at_slot => {
            EffectiveProposalState::Expired
        }
        _ => EffectiveProposalState::Active,
    }
}

fn matches_proposal_state_filter(
    filter: ProposalStateFilter,
    effective_state: EffectiveProposalState,
) -> bool {
    match filter {
        ProposalStateFilter::All => true,
        ProposalStateFilter::Active => effective_state == EffectiveProposalState::Active,
        ProposalStateFilter::Executed => effective_state == EffectiveProposalState::Executed,
        ProposalStateFilter::Cancelled => effective_state == EffectiveProposalState::Cancelled,
        ProposalStateFilter::Expired => effective_state == EffectiveProposalState::Expired,
    }
}

fn matches_proposal_queue_filter(
    filter: ProposalQueueFilter,
    queue_status: ProposalQueueStatus,
) -> bool {
    match filter {
        ProposalQueueFilter::All => true,
        ProposalQueueFilter::NeedsApproval => queue_status == ProposalQueueStatus::NeedsApproval,
        ProposalQueueFilter::Executable => queue_status == ProposalQueueStatus::Executable,
        ProposalQueueFilter::Closable => queue_status == ProposalQueueStatus::Closable,
    }
}

fn build_proposal_list_view(
    vault_snapshot: &VaultSnapshot,
    proposal: &ProposalSnapshot,
    effective_state: EffectiveProposalState,
    current_slot: u64,
    member_pda: Option<Pubkey>,
) -> ProposalListView {
    let member_status = member_pda.map(|member_pda| {
        let member_index = vault_snapshot.member_index(member_pda);
        let is_active = member_index.is_some();
        let has_approved = member_index
            .map(|index| proposal.has_member_approval(index))
            .unwrap_or(false);
        let can_approve = is_active
            && effective_state == EffectiveProposalState::Active
            && proposal.config_version == vault_snapshot.config_version
            && proposal.approval_count < vault_snapshot.threshold
            && !has_approved;
        ListMemberStatus {
            member_pda,
            is_active,
            has_approved,
            can_approve,
        }
    });

    let queue_status = match effective_state {
        EffectiveProposalState::Executed => ProposalQueueStatus::Closable,
        EffectiveProposalState::Cancelled => ProposalQueueStatus::Closable,
        EffectiveProposalState::Expired => ProposalQueueStatus::Closable,
        EffectiveProposalState::Active if proposal.config_version != vault_snapshot.config_version => {
            ProposalQueueStatus::StaleConfig
        }
        EffectiveProposalState::Active
            if proposal.approval_count >= vault_snapshot.threshold
                && current_slot >= proposal.not_before_slot =>
        {
            ProposalQueueStatus::Executable
        }
        EffectiveProposalState::Active
            if proposal.approval_count >= vault_snapshot.threshold =>
        {
            ProposalQueueStatus::WaitingTimelock
        }
        EffectiveProposalState::Active => {
            if member_status.map(|status| status.can_approve).unwrap_or(false) {
                ProposalQueueStatus::NeedsApproval
            } else {
                ProposalQueueStatus::WaitingThreshold
            }
        }
    };

    ProposalListView {
        display_state: effective_state,
        queue_status,
        member_status,
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).context("failed to serialize JSON output")?
    );
    Ok(())
}

fn bool_word(value: bool) -> String {
    if value {
        "yes".to_string()
    } else {
        "no".to_string()
    }
}

fn truncate_text(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut truncated = input.chars().take(max_chars - 1).collect::<String>();
    truncated.push('~');
    truncated
}

fn print_text_table(headers: &[&str], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

    let mut widths = headers.iter().map(|header| header.chars().count()).collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let mut header_line = String::new();
    for (index, header) in headers.iter().enumerate() {
        if index > 0 {
            header_line.push_str("  ");
        }
        header_line.push_str(&format!("{header:<width$}", width = widths[index]));
    }
    println!("{header_line}");

    let mut separator = String::new();
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            separator.push_str("  ");
        }
        separator.push_str(&"-".repeat(*width));
    }
    println!("{separator}");

    for row in rows {
        let mut line = String::new();
        for (index, width) in widths.iter().enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            let cell = row.get(index).map(String::as_str).unwrap_or_default();
            line.push_str(&format!("{cell:<width$}", width = *width));
        }
        println!("{line}");
    }
}

fn vault_snapshot_json(vault: Pubkey, snapshot: &VaultSnapshot) -> Value {
    json!({
        "vault": vault.to_string(),
        "threshold": snapshot.threshold,
        "member_count": snapshot.member_count,
        "config_version": snapshot.config_version,
        "next_proposal_id": snapshot.next_proposal_id,
        "timelock_slots": snapshot.timelock_slots,
        "proposal_ttl_slots": snapshot.proposal_ttl_slots,
        "pending_member_index": (snapshot.pending_member_index != NO_PENDING_MEMBER_INDEX)
            .then_some(snapshot.pending_member_index),
        "pending_member_pda": (snapshot.pending_member_index != NO_PENDING_MEMBER_INDEX)
            .then_some(snapshot.pending_member_pda.to_string()),
        "members": snapshot
            .members
            .iter()
            .take(snapshot.member_count as usize)
            .enumerate()
            .map(|(index, member)| json!({
                "member_index": index,
                "member_pda": member.member_pda.to_string(),
                "enabled": member.enabled,
                "weight": member.weight,
            }))
            .collect::<Vec<_>>(),
    })
}

fn proposal_list_entry_json(
    proposal: &ProposalSnapshot,
    proposal_view: ProposalListView,
    summary: &str,
    threshold: u8,
    current_config_version: u64,
) -> Value {
    json!({
        "proposal_id": proposal.proposal_id,
        "state": proposal_view.display_state.label().to_ascii_lowercase(),
        "state_label": proposal_view.display_state.label(),
        "queue_status": proposal_view.queue_status.label(),
        "approval_count": proposal.approval_count,
        "threshold": threshold,
        "approval_bitmap": proposal.approval_bitmap,
        "action_kind": proposal.action_kind,
        "action_kind_label": action_kind_label(proposal.action_kind),
        "proposer_member_index": proposal.proposer_member_index,
        "config_version": proposal.config_version,
        "current_config_version": current_config_version,
        "created_slot": proposal.created_slot,
        "not_before_slot": proposal.not_before_slot,
        "expires_at_slot": proposal.expires_at_slot,
        "executed_slot": proposal.executed_slot,
        "action_hash": hex::encode(proposal.action_hash),
        "proposal_tx_account": proposal.tx_account.to_string(),
        "summary": summary,
        "member_status": proposal_view.member_status.map(|member_status| json!({
            "member_pda": member_status.member_pda.to_string(),
            "is_active": member_status.is_active,
            "has_approved": member_status.has_approved,
            "can_approve": member_status.can_approve,
        })),
    })
}

fn decoded_action_payload_json(action_kind: u8, payload: &[u8]) -> Result<Value> {
    match action_kind {
        ACTION_KIND_SOL_WITHDRAW => {
            let (receiver, lamports) = decode_sol_withdraw_payload(payload)?;
            Ok(json!({
                "receiver": receiver.to_string(),
                "lamports": lamports,
            }))
        }
        ACTION_KIND_SPL_TRANSFER => {
            let (mint, destination, amount, decimals) = decode_spl_transfer_payload(payload)?;
            Ok(json!({
                "mint": mint.to_string(),
                "destination": destination.to_string(),
                "amount": amount,
                "decimals": decimals,
            }))
        }
        ACTION_KIND_UPDATE_CONFIG => {
            let (threshold, ttl, timelock) = decode_update_config_payload(payload)?;
            Ok(json!({
                "threshold": threshold,
                "proposal_ttl_slots": ttl,
                "timelock_slots": timelock,
            }))
        }
        ACTION_KIND_REPLACE_MEMBER => {
            let (member_index, new_member) = decode_replace_member_payload(payload)?;
            Ok(json!({
                "member_index": member_index,
                "new_member_pda": new_member.to_string(),
            }))
        }
        other => Ok(json!({
            "unsupported_action_kind": other,
        })),
    }
}

fn proposal_snapshot_json(
    vault: Pubkey,
    proposal: &ProposalSnapshot,
    proposal_tx: &ProposalTxSnapshot,
) -> Result<Value> {
    ensure!(
        proposal.action_hash == proposal_tx.action_hash,
        "proposal action hash does not match proposal_tx header"
    );
    Ok(json!({
        "vault": vault.to_string(),
        "proposal_id": proposal.proposal_id,
        "state": proposal_state_label(proposal.state).to_ascii_lowercase(),
        "state_label": proposal_state_label(proposal.state),
        "action_kind": proposal.action_kind,
        "action_kind_label": action_kind_label(proposal.action_kind),
        "proposer_member_index": proposal.proposer_member_index,
        "approval_bitmap": proposal.approval_bitmap,
        "approval_count": proposal.approval_count,
        "config_version": proposal.config_version,
        "created_slot": proposal.created_slot,
        "not_before_slot": proposal.not_before_slot,
        "expires_at_slot": proposal.expires_at_slot,
        "executed_slot": proposal.executed_slot,
        "action_hash": hex::encode(proposal.action_hash),
        "proposal_tx_account": proposal.tx_account.to_string(),
        "summary": action_summary(proposal.action_kind, &proposal_tx.payload)?,
        "decoded_payload": decoded_action_payload_json(proposal.action_kind, &proposal_tx.payload)?,
    }))
}

fn action_summary(action_kind: u8, payload: &[u8]) -> Result<String> {
    match action_kind {
        ACTION_KIND_SOL_WITHDRAW => {
            let (receiver, lamports) = decode_sol_withdraw_payload(payload)?;
            Ok(format!("withdraw {} lamports -> {}", lamports, receiver))
        }
        ACTION_KIND_SPL_TRANSFER => {
            let (mint, destination, amount, decimals) = decode_spl_transfer_payload(payload)?;
            Ok(format!(
                "transfer {} raw units (decimals={}) mint {} -> {}",
                amount, decimals, mint, destination
            ))
        }
        ACTION_KIND_UPDATE_CONFIG => {
            let (threshold, ttl, timelock) = decode_update_config_payload(payload)?;
            Ok(format!(
                "update threshold={} ttl={} timelock={}",
                threshold, ttl, timelock
            ))
        }
        ACTION_KIND_REPLACE_MEMBER => {
            let (member_index, new_member) = decode_replace_member_payload(payload)?;
            Ok(format!("replace member[{}] -> {}", member_index, new_member))
        }
        other => Ok(format!("unsupported action kind {}", action_kind_label(other))),
    }
}

fn print_decoded_action_payload(action_kind: u8, payload: &[u8]) -> Result<()> {
    match action_kind {
        ACTION_KIND_SOL_WITHDRAW => {
            let (receiver, lamports) = decode_sol_withdraw_payload(payload)?;
            println!("Decoded receiver:     {}", receiver);
            println!("Decoded lamports:     {}", lamports);
        }
        ACTION_KIND_SPL_TRANSFER => {
            let (mint, destination, amount, decimals) = decode_spl_transfer_payload(payload)?;
            println!("Decoded mint:         {}", mint);
            println!("Decoded destination:  {}", destination);
            println!("Decoded amount:       {}", amount);
            println!("Decoded decimals:     {}", decimals);
        }
        ACTION_KIND_UPDATE_CONFIG => {
            let (threshold, ttl, timelock) = decode_update_config_payload(payload)?;
            println!("Decoded threshold:    {}", threshold);
            println!("Decoded ttl slots:    {}", ttl);
            println!("Decoded timelock:     {}", timelock);
        }
        ACTION_KIND_REPLACE_MEMBER => {
            let (member_index, new_member) = decode_replace_member_payload(payload)?;
            println!("Decoded member index: {}", member_index);
            println!("Decoded new member:   {}", new_member);
        }
        other => println!("Decoded payload:      unsupported action kind {}", other),
    }
    Ok(())
}

fn derive_execute_accounts(
    vault: Pubkey,
    action_kind: u8,
    payload: &[u8],
) -> Result<Vec<AccountMeta>> {
    match action_kind {
        ACTION_KIND_SOL_WITHDRAW => {
            let (receiver, _lamports) = decode_sol_withdraw_payload(payload)?;
            Ok(vec![AccountMeta::new(receiver, false)])
        }
        ACTION_KIND_SPL_TRANSFER => {
            let (mint, destination, _amount, _decimals) = decode_spl_transfer_payload(payload)?;
            let token_program = spl_token_program_id();
            let source =
                get_associated_token_address_with_program_id(&vault, &mint, &token_program);
            Ok(vec![
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new(source, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new(destination, false),
            ])
        }
        ACTION_KIND_UPDATE_CONFIG | ACTION_KIND_REPLACE_MEMBER => Ok(vec![]),
        other => bail!(
            "auto execute account derivation is not implemented for action kind {} ({})",
            other,
            action_kind_label(other)
        ),
    }
}

fn decode_sol_withdraw_payload(payload: &[u8]) -> Result<(Pubkey, u64)> {
    ensure!(payload.len() == 40, "SOL withdraw payload must be 40 bytes");
    let receiver = read_pubkey_at(payload, 0)?;
    let lamports = read_u64_at(payload, 32)?;
    Ok((receiver, lamports))
}

fn decode_spl_transfer_payload(payload: &[u8]) -> Result<(Pubkey, Pubkey, u64, u8)> {
    ensure!(payload.len() == 73, "SPL transfer payload must be 73 bytes");
    let mint = read_pubkey_at(payload, 0)?;
    let destination = read_pubkey_at(payload, 32)?;
    let amount = read_u64_at(payload, 64)?;
    let decimals = *payload.get(72).context("SPL decimals missing")?;
    Ok((mint, destination, amount, decimals))
}

fn decode_update_config_payload(payload: &[u8]) -> Result<(u8, u64, u64)> {
    ensure!(payload.len() == 17, "config update payload must be 17 bytes");
    let threshold = *payload.first().context("config threshold missing")?;
    let ttl = read_u64_at(payload, 1)?;
    let timelock = read_u64_at(payload, 9)?;
    Ok((threshold, ttl, timelock))
}

fn decode_replace_member_payload(payload: &[u8]) -> Result<(u8, Pubkey)> {
    ensure!(payload.len() == 33, "replace member payload must be 33 bytes");
    let member_index = *payload.first().context("replace member index missing")?;
    let new_member = read_pubkey_at(payload, 1)?;
    Ok((member_index, new_member))
}

fn read_pubkey_at(data: &[u8], offset: usize) -> Result<Pubkey> {
    let bytes = data
        .get(offset..offset + 32)
        .ok_or_else(|| anyhow!("pubkey slice out of bounds"))?;
    let mut array = [0u8; 32];
    array.copy_from_slice(bytes);
    Ok(Pubkey::from(array))
}

fn read_array32_at(data: &[u8], offset: usize) -> Result<[u8; 32]> {
    let bytes = data
        .get(offset..offset + 32)
        .ok_or_else(|| anyhow!("32-byte slice out of bounds"))?;
    let mut array = [0u8; 32];
    array.copy_from_slice(bytes);
    Ok(array)
}

fn read_u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| anyhow!("u64 slice out of bounds"))?;
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(array))
}

fn read_u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("u32 slice out of bounds"))?;
    let mut array = [0u8; 4];
    array.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(array))
}

fn create_initialize_falcon512(
    vector_program_id: Pubkey,
    payer: Pubkey,
    wire_pubkey: &[u8; FALCON_512_PUBKEY_LEN],
) -> Result<Instruction> {
    let identity = falcon_identity(wire_pubkey);
    let (vector_pda, _) = find_vector_pda(&vector_program_id, &identity);
    let mut data = Vec::with_capacity(1 + wire_pubkey.len());
    data.push(INITIALIZE_VECTOR_DISCRIMINATOR);
    data.extend_from_slice(wire_pubkey);
    Ok(Instruction {
        program_id: vector_program_id,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(Pubkey::from_str(SYSTEM_PROGRAM_ID)?, false),
        ],
        data,
    })
}

fn create_advance_instruction(
    vector_program_id: Pubkey,
    identity: &[u8],
    signature: &[u8],
) -> Result<Instruction> {
    ensure!(
        signature.len() == FALCON_512_SIGNATURE_LEN,
        "unexpected Falcon signature length: {}",
        signature.len()
    );
    let (vector_pda, _) = find_vector_pda(&vector_program_id, identity);
    let mut data = Vec::with_capacity(1 + signature.len());
    data.push(ADVANCE_VECTOR_DISCRIMINATOR);
    data.extend_from_slice(signature);
    Ok(Instruction {
        program_id: vector_program_id,
        accounts: vec![
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID)?, false),
        ],
        data,
    })
}

fn create_passthrough_instruction(
    vector_program_id: Pubkey,
    identity: &[u8],
    instructions: &[Instruction],
) -> Result<Instruction> {
    ensure!(
        instructions.len() <= u8::MAX as usize,
        "too many passthrough subinstructions: {}",
        instructions.len()
    );
    let (vector_pda, _) = find_vector_pda(&vector_program_id, identity);
    let mut accounts =
        Vec::with_capacity(2 + instructions.iter().map(|ix| 1 + ix.accounts.len()).sum::<usize>());
    accounts.push(AccountMeta::new(vector_pda, false));
    accounts.push(AccountMeta::new_readonly(Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID)?, false));
    for ix in instructions {
        accounts.push(AccountMeta::new_readonly(ix.program_id, false));
        accounts.extend(ix.accounts.iter().map(|meta| AccountMeta {
            pubkey: meta.pubkey,
            is_signer: false,
            is_writable: meta.is_writable,
        }));
    }

    let mut data = Vec::new();
    data.push(PASSTHROUGH_VECTOR_DISCRIMINATOR);
    data.push(instructions.len() as u8);
    for ix in instructions {
        ensure!(
            ix.accounts.len() <= u8::MAX as usize,
            "passthrough subinstruction has too many accounts: {}",
            ix.accounts.len()
        );
        ensure!(
            ix.data.len() <= u16::MAX as usize,
            "passthrough subinstruction data too long: {}",
            ix.data.len()
        );
        data.push(ix.accounts.len() as u8);
        data.extend_from_slice(&(ix.data.len() as u16).to_le_bytes());
        data.extend_from_slice(&ix.data);
    }

    Ok(Instruction {
        program_id: vector_program_id,
        accounts,
        data,
    })
}

fn create_initialize_vault_instruction(
    vault_program_id: Pubkey,
    payer: Pubkey,
    vault_seed: &[u8],
    threshold: u8,
    proposal_ttl_slots: u64,
    timelock_slots: u64,
    members: &[Pubkey],
) -> Result<Instruction> {
    let (vault, _) = find_vault_pda(&vault_program_id, vault_seed);
    let mut data = Vec::with_capacity(1 + 1 + vault_seed.len() + 1 + 8 + 8 + 1 + 32 * members.len());
    data.push(INITIALIZE_VAULT_DISCRIMINATOR);
    data.push(vault_seed.len() as u8);
    data.extend_from_slice(vault_seed);
    data.push(threshold);
    data.extend_from_slice(&proposal_ttl_slots.to_le_bytes());
    data.extend_from_slice(&timelock_slots.to_le_bytes());
    data.push(members.len() as u8);
    for member in members {
        data.extend_from_slice(member.as_ref());
    }
    Ok(Instruction {
        program_id: vault_program_id,
        accounts: {
            let mut accounts = vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(Pubkey::from_str(SYSTEM_PROGRAM_ID)?, false),
            ];
            accounts.extend(
                members
                    .iter()
                    .copied()
                    .map(|member| AccountMeta::new_readonly(member, false)),
            );
            accounts
        },
        data,
    })
}

fn create_create_proposal_from_member_instruction(
    vault_program_id: Pubkey,
    member: Pubkey,
    payer: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
    action_kind: u8,
    not_before_slot: u64,
    expires_at_slot: u64,
    expected_action_hash: [u8; 32],
    proposal_tx_bytes: &[u8],
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let (proposal_tx, _) = find_proposal_tx_pda(&vault_program_id, &proposal);
    let mut data = Vec::with_capacity(1 + 8 + 1 + 8 + 8 + 32 + 4 + proposal_tx_bytes.len());
    data.push(CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.push(action_kind);
    data.extend_from_slice(&not_before_slot.to_le_bytes());
    data.extend_from_slice(&expires_at_slot.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    data.extend_from_slice(&(proposal_tx_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(proposal_tx_bytes);
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new_readonly(member, true),
            AccountMeta::new(payer, true),
            AccountMeta::new(vault, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new(proposal_tx, false),
            AccountMeta::new_readonly(Pubkey::from_str(SYSTEM_PROGRAM_ID).expect("system program id"), false),
        ],
        data,
    }
}

fn create_approve_proposal_from_member_instruction(
    vault_program_id: Pubkey,
    member: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new_readonly(member, true),
            AccountMeta::new(vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

fn create_cancel_proposal_from_member_instruction(
    vault_program_id: Pubkey,
    member: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(CANCEL_PROPOSAL_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new_readonly(member, true),
            AccountMeta::new(vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

fn create_revoke_proposal_from_member_instruction(
    vault_program_id: Pubkey,
    member: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new_readonly(member, true),
            AccountMeta::new(vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

fn create_execute_proposal_instruction(
    vault_program_id: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
    extra_accounts: &[AccountMeta],
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let (proposal_tx, _) = find_proposal_tx_pda(&vault_program_id, &proposal);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(EXECUTE_PROPOSAL_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    let mut accounts = vec![
        AccountMeta::new(vault, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(proposal_tx, false),
    ];
    accounts.extend_from_slice(extra_accounts);
    Instruction {
        program_id: vault_program_id,
        accounts,
        data,
    }
}

fn create_close_proposal_instruction(
    vault_program_id: Pubkey,
    vault: Pubkey,
    proposal_id: u64,
) -> Instruction {
    let (proposal, _) = find_proposal_pda(&vault_program_id, &vault, proposal_id);
    let (proposal_tx, _) = find_proposal_tx_pda(&vault_program_id, &proposal);
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new(vault, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new(proposal_tx, false),
        ],
        data: vec![CLOSE_PROPOSAL_DISCRIMINATOR],
    }
}

fn create_accept_pending_member_instruction(
    vault_program_id: Pubkey,
    member: Pubkey,
    vault: Pubkey,
) -> Instruction {
    Instruction {
        program_id: vault_program_id,
        accounts: vec![
            AccountMeta::new_readonly(member, true),
            AccountMeta::new(vault, false),
        ],
        data: vec![ACCEPT_PENDING_MEMBER_DISCRIMINATOR],
    }
}

fn create_sol_withdraw_action_payload(receiver: Pubkey, lamports: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32 + 8);
    payload.extend_from_slice(receiver.as_ref());
    payload.extend_from_slice(&lamports.to_le_bytes());
    payload
}

fn create_spl_transfer_action_payload(
    mint: Pubkey,
    destination: Pubkey,
    amount: u64,
    decimals: u8,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32 + 32 + 8 + 1);
    payload.extend_from_slice(mint.as_ref());
    payload.extend_from_slice(destination.as_ref());
    payload.extend_from_slice(&amount.to_le_bytes());
    payload.push(decimals);
    payload
}

fn create_replace_member_action_payload(member_index: u8, new_member: Pubkey) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + 32);
    payload.push(member_index);
    payload.extend_from_slice(new_member.as_ref());
    payload
}

fn create_update_config_action_payload(
    threshold: u8,
    proposal_ttl_slots: u64,
    timelock_slots: u64,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + 8 + 8);
    payload.push(threshold);
    payload.extend_from_slice(&proposal_ttl_slots.to_le_bytes());
    payload.extend_from_slice(&timelock_slots.to_le_bytes());
    payload
}

fn compute_action_hash(
    vault: &Pubkey,
    config_version: u64,
    proposal_id: u64,
    action_kind: u8,
    not_before_slot: u64,
    expires_at_slot: u64,
    payload: &[u8],
) -> [u8; 32] {
    let bundle_hash = Sha256::digest(payload);
    Sha256::new()
        .chain_update(ACTION_HASH_DOMAIN)
        .chain_update(vault.as_ref())
        .chain_update(config_version.to_le_bytes())
        .chain_update(proposal_id.to_le_bytes())
        .chain_update([action_kind])
        .chain_update(not_before_slot.to_le_bytes())
        .chain_update(expires_at_slot.to_le_bytes())
        .chain_update(bundle_hash)
        .finalize()
        .into()
}

fn find_vector_pda(program_id: &Pubkey, identity: &[u8]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VECTOR_PDA_SEED, identity], program_id)
}

fn find_vault_pda(program_id: &Pubkey, vault_seed: &[u8]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_PDA_SEED, vault_seed], program_id)
}

fn find_proposal_pda(program_id: &Pubkey, vault: &Pubkey, proposal_id: u64) -> (Pubkey, u8) {
    let proposal_id_le = proposal_id.to_le_bytes();
    Pubkey::find_program_address(&[PROPOSAL_PDA_SEED, vault.as_ref(), &proposal_id_le], program_id)
}

fn find_proposal_tx_pda(program_id: &Pubkey, proposal: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[PROPOSAL_TX_PDA_SEED, proposal.as_ref()], program_id)
}

fn falcon_identity(wire_pubkey: &[u8; FALCON_512_PUBKEY_LEN]) -> [u8; 32] {
    Sha256::digest(wire_pubkey).into()
}

fn sign_falcon_digest(
    digest: &[u8; 32],
    secret_key: &falcon512::SecretKey,
) -> [u8; FALCON_512_SIGNATURE_LEN] {
    let signature = falcon512::detached_sign(digest, secret_key);
    let signature_bytes = signature.as_bytes();
    let mut out = [0u8; FALCON_512_SIGNATURE_LEN];
    out[..signature_bytes.len()].copy_from_slice(signature_bytes);
    out
}

fn wire_pubkey(public_key: &falcon512::PublicKey) -> [u8; FALCON_512_PUBKEY_LEN] {
    let bytes = public_key.as_bytes();
    let mut out = [0u8; FALCON_512_PUBKEY_LEN];
    out.copy_from_slice(bytes);
    out
}

fn advance_vector_digest(
    vector_program_id: Pubkey,
    identity: &[u8],
    nonce: &[u8; 32],
    pre_instructions: &[Instruction],
    post_instructions: &[Instruction],
    fee_payer: Option<Pubkey>,
) -> Result<[u8; 32]> {
    let placeholder = vec![0u8; FALCON_512_SIGNATURE_LEN];
    let advance_ix = create_advance_instruction(vector_program_id, identity, &placeholder)?;
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

fn get_info(port: &mut dyn SerialPort, timeout: Duration) -> Result<BTreeMap<String, String>> {
    let response = send_command(port, "GET_INFO", timeout)?;
    parse_info(&response)
}

fn unlock_device(port: &mut dyn SerialPort, pin: &str, timeout: Duration) -> Result<()> {
    let response = send_command(port, &format!("UNLOCK:PIN:{pin}"), timeout)?;
    expect_line_prefix("UNLOCK:PIN", &response, "UNLOCKED_UNTIL:")
}

fn get_se050_pubkey(port: &mut dyn SerialPort, timeout: Duration) -> Result<Pubkey> {
    let response = send_command(port, "GET_PUBKEY", timeout)?;
    let pubkey = prefixed_value(&response, "PUBKEY:")?;
    Pubkey::from_str(pubkey).context("failed to parse SE050 pubkey")
}

struct PqcState {
    wire_pubkey: [u8; FALCON_512_PUBKEY_LEN],
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
    let wire_pubkey_vec = STANDARD
        .decode(&wire_pubkey_b64)
        .context("failed to decode PQC wire pubkey")?;
    ensure!(
        wire_pubkey_vec.len() == FALCON_512_PUBKEY_LEN,
        "unexpected Falcon pubkey length: {}",
        wire_pubkey_vec.len()
    );
    verify_binding_signature(se050_pubkey, &wire_pubkey_vec, &binding_sig)?;
    let mut wire_pubkey = [0u8; FALCON_512_PUBKEY_LEN];
    wire_pubkey.copy_from_slice(&wire_pubkey_vec);

    Ok(PqcState { wire_pubkey })
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

fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

fn proposal_state_label(state: u8) -> &'static str {
    match state {
        PROPOSAL_STATE_ACTIVE => "Active",
        PROPOSAL_STATE_EXECUTED => "Executed",
        PROPOSAL_STATE_CANCELLED => "Cancelled",
        PROPOSAL_STATE_EXPIRED => "Expired",
        _ => "Unknown",
    }
}

fn action_kind_label(kind: u8) -> &'static str {
    match kind {
        ACTION_KIND_CPI_BUNDLE => "Reserved/disabled CPI bundle",
        ACTION_KIND_SOL_WITHDRAW => "SOL withdraw",
        ACTION_KIND_SPL_TRANSFER => "SPL transfer",
        ACTION_KIND_UPDATE_CONFIG => "Config update",
        ACTION_KIND_REPLACE_MEMBER => "Replace member",
        _ => "Unknown",
    }
}
