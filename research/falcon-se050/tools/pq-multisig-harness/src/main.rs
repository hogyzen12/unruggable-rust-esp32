use anyhow::{bail, ensure, Context, Result};
use pqcrypto_falcon::falcon512;
use pqcrypto_traits::sign::{DetachedSignature, PublicKey as _};
use sha2::{Digest, Sha256};
use solana_commitment_config::CommitmentConfig;
use solana_falcon512::{FALCON_512_PUBKEY_LEN, FALCON_512_SIGNATURE_LEN};
use solana_instruction::{AccountMeta, BorrowedAccountMeta, BorrowedInstruction, Instruction};
use solana_instructions_sysvar::construct_instructions_data;
use solana_keypair::{read_keypair_file, Keypair};
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_rpc_client::rpc_client::RpcClient;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account as system_create_account;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::{
    address::get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};
use spl_token_interface::{
    id as spl_token_program_id,
    instruction::{initialize_mint2 as spl_initialize_mint2, mint_to as spl_mint_to},
    state::Mint as SplTokenMint,
};
use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    str::FromStr,
    thread::sleep,
    time::Duration,
};

const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_VECTOR_PROGRAM_ID: &str = "FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU";
const DEFAULT_VAULT_PROGRAM_ID: &str = "9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52";
const DEFAULT_VAULT_SEED_PREFIX: &str = "localnet-";
const DEFAULT_WITHDRAW_LAMPORTS: u64 = 1_000_000;
const DEFAULT_VAULT_BUFFER_LAMPORTS: u64 = 500_000;
const DEFAULT_PROPOSAL_OFFSET_SLOTS: u64 = 16;
const DEFAULT_PROPOSAL_TTL_SLOTS: u64 = 512;
const LOCALNET_AIRDROP_LAMPORTS: u64 = 10_000_000_000;
const DEFAULT_RECIPIENT_LAMPORTS: u64 = 1_000_000;
const VECTOR_PDA_SEED: &[u8] = b"vector";
const VAULT_PDA_SEED: &[u8] = b"pq_vault";
const PROPOSAL_PDA_SEED: &[u8] = b"proposal";
const PROPOSAL_TX_PDA_SEED: &[u8] = b"proposal_tx";
const ACTION_HASH_DOMAIN: &[u8] = b"UNRUGGABLE_PQ_MULTISIG_PROPOSAL_V1";
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
const ACTION_KIND_REPLACE_MEMBER: u8 = 1;
const ACTION_KIND_SOL_WITHDRAW: u8 = 4;
const ACTION_KIND_SPL_TRANSFER: u8 = 5;
const ACTION_KIND_UPDATE_CONFIG: u8 = 6;
const VAULT_ACCOUNT_LEN: usize = 224;
const PROPOSAL_ACCOUNT_LEN: usize = 160;
const MAX_MEMBERS: usize = 3;
const MEMBER_RECORD_LEN: usize = 40;
const PROPOSAL_STATE_ACTIVE: u8 = 0;
const PROPOSAL_STATE_EXECUTED: u8 = 1;
const PROPOSAL_STATE_CANCELLED: u8 = 2;
const NO_PENDING_MEMBER_INDEX: u8 = u8::MAX;
const SYSTEM_TRANSFER_DISCRIMINATOR: u32 = 2;
const DEFAULT_SPL_TRANSFER_AMOUNT: u64 = 250_000;
const DEFAULT_SPL_MINT_AMOUNT: u64 = 1_000_000;
const DEFAULT_SPL_DECIMALS: u8 = 6;
const DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS: u64 = 250_000;
const UPDATED_THRESHOLD: u8 = 3;
const UPDATED_TIMELOCK_SLOTS: u64 = 4;
const UPDATED_PROPOSAL_TTL_DELTA: u64 = 256;
const ROTATED_MEMBER_INDEX: usize = 2;

#[derive(Debug)]
struct Config {
    rpc_url: String,
    fee_payer_file: PathBuf,
    vector_program_id: Pubkey,
    vault_program_id: Pubkey,
    vault_seed: Vec<u8>,
    withdraw_lamports: u64,
    vault_buffer_lamports: u64,
    proposal_offset_slots: u64,
    proposal_ttl_slots: u64,
    commitment: CommitmentConfig,
}

struct Member {
    index: usize,
    secret_key: falcon512::SecretKey,
    wire_pubkey: [u8; FALCON_512_PUBKEY_LEN],
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
    approval_count: u8,
    executed_slot: u64,
}

struct MemberAdvanceResult {
    signature: Signature,
    digest: [u8; 32],
}

struct SplFixture {
    token_program: Pubkey,
    mint: Pubkey,
    source_ata: Pubkey,
    destination_ata: Pubkey,
    decimals: u8,
    mint_amount: u64,
    setup_signature: Signature,
    mint_to_signature: Signature,
}

fn main() -> Result<()> {
    let config = parse_args()?;
    run(config)
}

fn run(config: Config) -> Result<()> {
    let client = RpcClient::new_with_commitment(config.rpc_url.clone(), config.commitment);
    let fee_payer = read_keypair_file(&config.fee_payer_file).map_err(|err| {
        anyhow::anyhow!(
            "failed to read fee payer keypair {}: {err}",
            config.fee_payer_file.display()
        )
    })?;

    println!("============================================================");
    println!("PQ Multisig Localnet Harness");
    println!("============================================================");
    println!("RPC URL:              {}", config.rpc_url);
    println!("Fee payer:            {}", fee_payer.pubkey());
    println!("Vector program:       {}", config.vector_program_id);
    println!("Vault program:        {}", config.vault_program_id);
    println!(
        "Vault seed:           {}",
        String::from_utf8_lossy(&config.vault_seed)
    );
    println!("Withdraw lamports:    {}", config.withdraw_lamports);

    let mut payer_balance = client
        .get_balance(&fee_payer.pubkey())
        .context("failed to fetch fee payer balance")?;
    if payer_balance == 0 && is_localnet_rpc(&config.rpc_url) {
        println!();
        println!("Airdrop fee payer");
        let airdrop_signature = client
            .request_airdrop(&fee_payer.pubkey(), LOCALNET_AIRDROP_LAMPORTS)
            .context("failed to request localnet airdrop for fee payer")?;
        println!("Airdrop tx:           {}", airdrop_signature);
        wait_for_balance_at_least(&client, fee_payer.pubkey(), LOCALNET_AIRDROP_LAMPORTS)?;
        payer_balance = client
            .get_balance(&fee_payer.pubkey())
            .context("failed to fetch fee payer balance after airdrop")?;
    }
    println!("Fee payer balance:    {}", payer_balance);

    let members = generate_members(config.vector_program_id);
    let replacement_member = generate_member(MAX_MEMBERS, config.vector_program_id);
    println!();
    println!("Members");
    for member in &members {
        println!(
            "Member {}: vector_pda={} identity={}",
            member.index,
            member.vector_pda,
            hex::encode(member.identity)
        );
    }
    println!(
        "Replacement candidate {}: vector_pda={} identity={}",
        replacement_member.index,
        replacement_member.vector_pda,
        hex::encode(replacement_member.identity)
    );

    println!();
    println!("Initialize member Vector PDAs");
    for member in &members {
        let signature = initialize_member_vector(&client, &fee_payer, &config, member)?;
        println!("Member {} init tx:    {}", member.index, signature);
    }
    let replacement_signature =
        initialize_member_vector(&client, &fee_payer, &config, &replacement_member)?;
    println!(
        "Replacement init tx:  {}",
        replacement_signature
    );

    println!();
    println!("Initialize vault");
    let member_pdas = [
        members[0].vector_pda,
        members[1].vector_pda,
        members[2].vector_pda,
    ];
    let vault_signature = initialize_vault(&client, &fee_payer, &config, &member_pdas)?;
    let (vault_pda, vault_bump) = find_vault_pda(&config.vault_program_id, &config.vault_seed);
    println!("Vault init tx:        {}", vault_signature);
    println!("Vault PDA:            {}", vault_pda);
    println!("Vault bump:           {}", vault_bump);

    println!();
    println!("Create recipient");
    let recipient = Keypair::new();
    let recipient_signature = create_recipient(&client, &fee_payer, &recipient)?;
    println!("Recipient create tx:  {}", recipient_signature);
    println!("Recipient:            {}", recipient.pubkey());

    println!();
    println!("Fund vault");
    let vault_rent = client
        .get_minimum_balance_for_rent_exemption(VAULT_ACCOUNT_LEN)
        .context("failed to fetch vault rent floor")?;
    let desired_vault_balance =
        vault_rent + config.withdraw_lamports + config.vault_buffer_lamports;
    let vault_fund_signature =
        fund_vault(&client, &fee_payer, vault_pda, desired_vault_balance)?;
    println!("Vault fund tx:        {}", vault_fund_signature);
    println!(
        "Vault balance target: {}",
        client
            .get_balance(&vault_pda)
            .context("failed to fetch funded vault balance")?
    );

    let initial_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    assert_no_pending_member(&initial_vault_snapshot)?;
    let proposal_id = initial_vault_snapshot.next_proposal_id;
    let proposal_slot = client
        .get_slot()
        .context("failed to fetch current slot before proposal")?;
    let not_before_slot = proposal_slot.saturating_add(config.proposal_offset_slots);
    let expires_at_slot = not_before_slot.saturating_add(config.proposal_ttl_slots);
    let action_payload =
        create_sol_withdraw_action_payload(recipient.pubkey(), config.withdraw_lamports);
    let action_hash = compute_action_hash(
        &vault_pda,
        initial_vault_snapshot.config_version,
        proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        not_before_slot,
        expires_at_slot,
        &action_payload,
    );

    println!();
    println!("Create proposal (member 0)");
    let create_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        not_before_slot,
        expires_at_slot,
        action_hash,
        &action_payload,
    );
    let create_result =
        submit_member_passthrough(&client, &fee_payer, &config, &members[0], &[create_sub_ix])?;
    println!("Create proposal tx:   {}", create_result.signature);
    println!("Create digest hex:    {}", hex::encode(create_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "proposal was not created with the first approval"
    );

    println!();
    println!("Approve proposal (member 1)");
    let approve_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        vault_pda,
        proposal_id,
        action_hash,
    );
    let approve_result =
        submit_member_passthrough(&client, &fee_payer, &config, &members[1], &[approve_sub_ix])?;
    println!("Approve proposal tx:  {}", approve_result.signature);
    println!("Approve digest hex:   {}", hex::encode(approve_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 2,
        "proposal approvals did not reach 2-of-3"
    );

    println!();
    println!("Wait for proposal not-before slot");
    println!(
        "Current slot:         {}",
        client.get_slot().context("failed to fetch slot")?
    );
    println!("Not-before slot:      {}", not_before_slot);
    wait_for_slot(&client, not_before_slot)?;

    let vault_balance_before = client
        .get_balance(&vault_pda)
        .context("failed to fetch vault balance before execute")?;
    let recipient_balance_before = client
        .get_balance(&recipient.pubkey())
        .context("failed to fetch recipient balance before execute")?;

    println!();
    println!("Execute proposal");
    let execute_ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        proposal_id,
        action_hash,
        &[AccountMeta::new(recipient.pubkey(), false)],
    );
    let execute_signature = send_transaction(&client, &fee_payer, &[execute_ix], &[])?;
    println!("Execute tx:           {}", execute_signature);

    let vault_balance_after = client
        .get_balance(&vault_pda)
        .context("failed to fetch vault balance after execute")?;
    let recipient_balance_after = client
        .get_balance(&recipient.pubkey())
        .context("failed to fetch recipient balance after execute")?;
    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, proposal_id)?;

    ensure!(
        vault_balance_before.saturating_sub(vault_balance_after) == config.withdraw_lamports,
        "vault balance delta {} did not match withdraw amount {}",
        vault_balance_before.saturating_sub(vault_balance_after),
        config.withdraw_lamports
    );
    ensure!(
        recipient_balance_after.saturating_sub(recipient_balance_before) == config.withdraw_lamports,
        "recipient balance delta {} did not match withdraw amount {}",
        recipient_balance_after.saturating_sub(recipient_balance_before),
        config.withdraw_lamports
    );
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_EXECUTED && proposal_snapshot.executed_slot != 0,
        "proposal did not enter executed state"
    );

    println!();
    println!("SOL withdraw success");
    println!("Proposal ID:          {}", proposal_id);
    println!("Proposal action hash: {}", hex::encode(action_hash));
    println!(
        "Vault balance:        {} -> {}",
        vault_balance_before, vault_balance_after
    );
    println!(
        "Recipient balance:    {} -> {}",
        recipient_balance_before, recipient_balance_after
    );
    println!("Executed slot:        {}", proposal_snapshot.executed_slot);

    println!();
    println!("Create revocable proposal (member 0)");
    let cancel_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    let cancel_proposal_id = cancel_vault_snapshot.next_proposal_id;
    let cancel_proposal_slot = client
        .get_slot()
        .context("failed to fetch current slot before cancel proposal")?;
    let cancel_not_before_slot = cancel_proposal_slot.saturating_add(config.proposal_offset_slots);
    let cancel_expires_at_slot = cancel_not_before_slot.saturating_add(config.proposal_ttl_slots);
    let revoke_action_payload =
        create_sol_withdraw_action_payload(recipient.pubkey(), DEFAULT_RECIPIENT_LAMPORTS / 2);
    let revoke_action_hash = compute_action_hash(
        &vault_pda,
        cancel_vault_snapshot.config_version,
        cancel_proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        cancel_not_before_slot,
        cancel_expires_at_slot,
        &revoke_action_payload,
    );
    let create_cancelable_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        cancel_proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        cancel_not_before_slot,
        cancel_expires_at_slot,
        revoke_action_hash,
        &revoke_action_payload,
    );
    let create_cancelable_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[0],
        &[create_cancelable_sub_ix],
    )?;
    println!("Create proposal tx:   {}", create_cancelable_result.signature);
    println!("Create digest hex:    {}", hex::encode(create_cancelable_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, cancel_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "cancelable proposal was not created with exactly one approval"
    );

    println!();
    println!("Approve revocable proposal (member 1)");
    let approve_cancelable_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        vault_pda,
        cancel_proposal_id,
        revoke_action_hash,
    );
    let approve_cancelable_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[1],
        &[approve_cancelable_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_cancelable_result.signature);
    println!(
        "Approve digest hex:   {}",
        hex::encode(approve_cancelable_result.digest)
    );

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, cancel_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 2,
        "revocable proposal did not reach 2 approvals before revoke"
    );

    println!();
    println!("Revoke proposal (member 1)");
    let revoke_sub_ix = create_revoke_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        vault_pda,
        cancel_proposal_id,
        revoke_action_hash,
    );
    let revoke_result =
        submit_member_passthrough(&client, &fee_payer, &config, &members[1], &[revoke_sub_ix])?;
    println!("Revoke proposal tx:   {}", revoke_result.signature);
    println!("Revoke digest hex:    {}", hex::encode(revoke_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, cancel_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "revocable proposal approval count did not drop back to 1 after revoke"
    );

    println!();
    println!("Cancel proposal (member 0)");
    let cancel_sub_ix = create_cancel_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        vault_pda,
        cancel_proposal_id,
        revoke_action_hash,
    );
    let cancel_result =
        submit_member_passthrough(&client, &fee_payer, &config, &members[0], &[cancel_sub_ix])?;
    println!("Cancel proposal tx:   {}", cancel_result.signature);
    println!("Cancel digest hex:    {}", hex::encode(cancel_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, cancel_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_CANCELLED,
        "cancelable proposal did not enter cancelled state"
    );

    println!();
    println!("Close cancelled proposal");
    let close_cancelled_ix = create_close_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        cancel_proposal_id,
    );
    let close_cancelled_signature =
        send_transaction(&client, &fee_payer, &[close_cancelled_ix], &[])?;
    println!("Close proposal tx:    {}", close_cancelled_signature);
    let (cancel_proposal_pda, _) =
        find_proposal_pda(&config.vault_program_id, &vault_pda, cancel_proposal_id);
    let (cancel_proposal_tx_pda, _) =
        find_proposal_tx_pda(&config.vault_program_id, &cancel_proposal_pda);
    assert_account_missing(&client, cancel_proposal_pda)?;
    assert_account_missing(&client, cancel_proposal_tx_pda)?;

    println!();
    println!("Proposal revoke/cancel/close success");
    println!("Proposal ID:          {}", cancel_proposal_id);
    println!("Proposal action hash: {}", hex::encode(revoke_action_hash));

    println!();
    println!("Prepare SPL fixture");
    let spl_fixture = setup_spl_fixture(
        &client,
        &fee_payer,
        vault_pda,
        recipient.pubkey(),
        DEFAULT_SPL_DECIMALS,
        DEFAULT_SPL_MINT_AMOUNT,
    )?;
    println!("SPL setup tx:         {}", spl_fixture.setup_signature);
    println!("SPL mint-to tx:       {}", spl_fixture.mint_to_signature);
    println!("SPL mint:             {}", spl_fixture.mint);
    println!("Vault source ATA:     {}", spl_fixture.source_ata);
    println!("Recipient dest ATA:   {}", spl_fixture.destination_ata);

    let spl_source_before = read_token_amount(&client, spl_fixture.source_ata)?;
    let spl_destination_before = read_token_amount(&client, spl_fixture.destination_ata)?;
    ensure!(
        spl_source_before == spl_fixture.mint_amount,
        "unexpected initial vault source token amount {} (expected {})",
        spl_source_before,
        spl_fixture.mint_amount
    );
    ensure!(
        spl_destination_before == 0,
        "unexpected initial destination token amount {}",
        spl_destination_before
    );

    let spl_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    let spl_proposal_id = spl_vault_snapshot.next_proposal_id;
    let spl_proposal_slot = client
        .get_slot()
        .context("failed to fetch current slot before SPL proposal")?;
    let spl_not_before_slot = spl_proposal_slot.saturating_add(config.proposal_offset_slots);
    let spl_expires_at_slot = spl_not_before_slot.saturating_add(config.proposal_ttl_slots);
    let spl_action_payload = create_spl_transfer_action_payload(
        spl_fixture.mint,
        spl_fixture.destination_ata,
        DEFAULT_SPL_TRANSFER_AMOUNT,
        spl_fixture.decimals,
    );
    let spl_action_hash = compute_action_hash(
        &vault_pda,
        spl_vault_snapshot.config_version,
        spl_proposal_id,
        ACTION_KIND_SPL_TRANSFER,
        spl_not_before_slot,
        spl_expires_at_slot,
        &spl_action_payload,
    );

    println!();
    println!("Create SPL transfer proposal (member 0)");
    let create_spl_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        spl_proposal_id,
        ACTION_KIND_SPL_TRANSFER,
        spl_not_before_slot,
        spl_expires_at_slot,
        spl_action_hash,
        &spl_action_payload,
    );
    let create_spl_result =
        submit_member_passthrough(&client, &fee_payer, &config, &members[0], &[create_spl_sub_ix])?;
    println!("Create proposal tx:   {}", create_spl_result.signature);
    println!("Create digest hex:    {}", hex::encode(create_spl_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, spl_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "SPL proposal was not created with the first approval"
    );

    println!();
    println!("Approve SPL transfer proposal (member 2)");
    let approve_spl_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[2].vector_pda,
        vault_pda,
        spl_proposal_id,
        spl_action_hash,
    );
    let approve_spl_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[2],
        &[approve_spl_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_spl_result.signature);
    println!("Approve digest hex:   {}", hex::encode(approve_spl_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, spl_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 2,
        "SPL proposal approvals did not reach 2-of-3"
    );

    println!();
    println!("Wait for SPL proposal not-before slot");
    println!(
        "Current slot:         {}",
        client.get_slot().context("failed to fetch slot")?
    );
    println!("Not-before slot:      {}", spl_not_before_slot);
    wait_for_slot(&client, spl_not_before_slot)?;

    println!();
    println!("Execute SPL transfer proposal");
    let execute_spl_ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        spl_proposal_id,
        spl_action_hash,
        &[
            AccountMeta::new_readonly(spl_fixture.token_program, false),
            AccountMeta::new(spl_fixture.source_ata, false),
            AccountMeta::new_readonly(spl_fixture.mint, false),
            AccountMeta::new(spl_fixture.destination_ata, false),
        ],
    );
    let execute_spl_signature = send_transaction(&client, &fee_payer, &[execute_spl_ix], &[])?;
    println!("Execute tx:           {}", execute_spl_signature);

    let spl_source_after = read_token_amount(&client, spl_fixture.source_ata)?;
    let spl_destination_after = read_token_amount(&client, spl_fixture.destination_ata)?;
    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, spl_proposal_id)?;
    ensure!(
        spl_source_before.saturating_sub(spl_source_after) == DEFAULT_SPL_TRANSFER_AMOUNT,
        "vault source token delta {} did not match SPL transfer amount {}",
        spl_source_before.saturating_sub(spl_source_after),
        DEFAULT_SPL_TRANSFER_AMOUNT
    );
    ensure!(
        spl_destination_after.saturating_sub(spl_destination_before) == DEFAULT_SPL_TRANSFER_AMOUNT,
        "destination token delta {} did not match SPL transfer amount {}",
        spl_destination_after.saturating_sub(spl_destination_before),
        DEFAULT_SPL_TRANSFER_AMOUNT
    );
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_EXECUTED && proposal_snapshot.executed_slot != 0,
        "SPL proposal did not enter executed state"
    );

    println!();
    println!("SPL transfer success");
    println!("Proposal ID:          {}", spl_proposal_id);
    println!("Proposal action hash: {}", hex::encode(spl_action_hash));
    println!("Vault source amount:  {} -> {}", spl_source_before, spl_source_after);
    println!(
        "Recipient amount:     {} -> {}",
        spl_destination_before, spl_destination_after
    );
    println!("Executed slot:        {}", proposal_snapshot.executed_slot);

    let config_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    let config_proposal_id = config_vault_snapshot.next_proposal_id;
    let config_proposal_slot = client
        .get_slot()
        .context("failed to fetch current slot before config proposal")?;
    let config_not_before_slot = config_proposal_slot.saturating_add(config.proposal_offset_slots);
    let config_expires_at_slot = config_not_before_slot.saturating_add(config.proposal_ttl_slots);
    let updated_proposal_ttl_slots = config_vault_snapshot
        .proposal_ttl_slots
        .saturating_add(UPDATED_PROPOSAL_TTL_DELTA);
    let config_action_payload = create_update_config_action_payload(
        UPDATED_THRESHOLD,
        updated_proposal_ttl_slots,
        UPDATED_TIMELOCK_SLOTS,
    );
    let config_action_hash = compute_action_hash(
        &vault_pda,
        config_vault_snapshot.config_version,
        config_proposal_id,
        ACTION_KIND_UPDATE_CONFIG,
        config_not_before_slot,
        config_expires_at_slot,
        &config_action_payload,
    );

    println!();
    println!("Create config update proposal (member 1)");
    let create_config_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        config_proposal_id,
        ACTION_KIND_UPDATE_CONFIG,
        config_not_before_slot,
        config_expires_at_slot,
        config_action_hash,
        &config_action_payload,
    );
    let create_config_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[1],
        &[create_config_sub_ix],
    )?;
    println!("Create proposal tx:   {}", create_config_result.signature);
    println!("Create digest hex:    {}", hex::encode(create_config_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, config_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "config proposal was not created with the first approval"
    );

    println!();
    println!("Approve config update proposal (member 2)");
    let approve_config_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[2].vector_pda,
        vault_pda,
        config_proposal_id,
        config_action_hash,
    );
    let approve_config_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[2],
        &[approve_config_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_config_result.signature);
    println!("Approve digest hex:   {}", hex::encode(approve_config_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, config_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 2,
        "config proposal approvals did not reach 2-of-3"
    );

    println!();
    println!("Wait for config proposal not-before slot");
    println!(
        "Current slot:         {}",
        client.get_slot().context("failed to fetch slot")?
    );
    println!("Not-before slot:      {}", config_not_before_slot);
    wait_for_slot(&client, config_not_before_slot)?;

    println!();
    println!("Execute config update proposal");
    let execute_config_ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        config_proposal_id,
        config_action_hash,
        &[],
    );
    let execute_config_signature =
        send_transaction(&client, &fee_payer, &[execute_config_ix], &[])?;
    println!("Execute tx:           {}", execute_config_signature);

    let updated_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, config_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_EXECUTED && proposal_snapshot.executed_slot != 0,
        "config proposal did not enter executed state"
    );
    ensure!(
        updated_vault_snapshot.config_version == config_vault_snapshot.config_version + 1,
        "vault config version {} did not increment from {}",
        updated_vault_snapshot.config_version,
        config_vault_snapshot.config_version
    );
    ensure!(
        updated_vault_snapshot.threshold == UPDATED_THRESHOLD,
        "vault threshold {} did not match updated threshold {}",
        updated_vault_snapshot.threshold,
        UPDATED_THRESHOLD
    );
    ensure!(
        updated_vault_snapshot.timelock_slots == UPDATED_TIMELOCK_SLOTS,
        "vault timelock {} did not match updated timelock {}",
        updated_vault_snapshot.timelock_slots,
        UPDATED_TIMELOCK_SLOTS
    );
    ensure!(
        updated_vault_snapshot.proposal_ttl_slots == updated_proposal_ttl_slots,
        "vault proposal ttl {} did not match updated ttl {}",
        updated_vault_snapshot.proposal_ttl_slots,
        updated_proposal_ttl_slots
    );
    assert_no_pending_member(&updated_vault_snapshot)?;
    assert_enabled_members(&updated_vault_snapshot, &member_pdas)?;

    println!();
    println!("Config update success");
    println!("Proposal ID:          {}", config_proposal_id);
    println!("Proposal action hash: {}", hex::encode(config_action_hash));
    println!(
        "Config version:       {} -> {}",
        config_vault_snapshot.config_version, updated_vault_snapshot.config_version
    );
    println!(
        "Threshold:            {} -> {}",
        config_vault_snapshot.threshold, updated_vault_snapshot.threshold
    );
    println!(
        "Proposal TTL:         {} -> {}",
        config_vault_snapshot.proposal_ttl_slots, updated_vault_snapshot.proposal_ttl_slots
    );
    println!(
        "Timelock slots:       {} -> {}",
        config_vault_snapshot.timelock_slots, updated_vault_snapshot.timelock_slots
    );
    println!("Executed slot:        {}", proposal_snapshot.executed_slot);

    let rotated_member_pdas = [
        members[0].vector_pda,
        members[1].vector_pda,
        replacement_member.vector_pda,
    ];
    let rotation_proposal_id = updated_vault_snapshot.next_proposal_id;
    let rotation_proposal_slot = client
        .get_slot()
        .context("failed to fetch current slot before member rotation proposal")?;
    let rotation_not_before_slot =
        rotation_proposal_slot.saturating_add(config.proposal_offset_slots);
    let rotation_expires_at_slot = rotation_not_before_slot
        .saturating_add(updated_vault_snapshot.proposal_ttl_slots);
    let rotation_action_payload = create_replace_member_action_payload(
        ROTATED_MEMBER_INDEX as u8,
        replacement_member.vector_pda,
    );
    let rotation_action_hash = compute_action_hash(
        &vault_pda,
        updated_vault_snapshot.config_version,
        rotation_proposal_id,
        ACTION_KIND_REPLACE_MEMBER,
        rotation_not_before_slot,
        rotation_expires_at_slot,
        &rotation_action_payload,
    );

    println!();
    println!("Create member rotation proposal (member 0)");
    let create_rotation_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        rotation_proposal_id,
        ACTION_KIND_REPLACE_MEMBER,
        rotation_not_before_slot,
        rotation_expires_at_slot,
        rotation_action_hash,
        &rotation_action_payload,
    );
    let create_rotation_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[0],
        &[create_rotation_sub_ix],
    )?;
    println!("Create proposal tx:   {}", create_rotation_result.signature);
    println!("Create digest hex:    {}", hex::encode(create_rotation_result.digest));

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, rotation_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "member rotation proposal was not created with the first approval"
    );

    println!();
    println!("Approve member rotation proposal (member 1)");
    let approve_rotation_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        vault_pda,
        rotation_proposal_id,
        rotation_action_hash,
    );
    let approve_rotation_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[1],
        &[approve_rotation_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_rotation_result.signature);
    println!("Approve digest hex:   {}", hex::encode(approve_rotation_result.digest));

    println!();
    println!("Approve member rotation proposal (member 2)");
    let approve_rotation_final_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[ROTATED_MEMBER_INDEX].vector_pda,
        vault_pda,
        rotation_proposal_id,
        rotation_action_hash,
    );
    let approve_rotation_final_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[ROTATED_MEMBER_INDEX],
        &[approve_rotation_final_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_rotation_final_result.signature);
    println!(
        "Approve digest hex:   {}",
        hex::encode(approve_rotation_final_result.digest)
    );

    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, rotation_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE
            && proposal_snapshot.approval_count == UPDATED_THRESHOLD,
        "member rotation proposal approvals did not reach {}-of-{}",
        UPDATED_THRESHOLD,
        MAX_MEMBERS
    );

    println!();
    println!("Wait for member rotation proposal not-before slot");
    println!(
        "Current slot:         {}",
        client.get_slot().context("failed to fetch slot")?
    );
    println!("Not-before slot:      {}", rotation_not_before_slot);
    wait_for_slot(&client, rotation_not_before_slot)?;

    println!();
    println!("Execute member rotation proposal");
    let execute_rotation_ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        rotation_proposal_id,
        rotation_action_hash,
        &[],
    );
    let execute_rotation_signature =
        send_transaction(&client, &fee_payer, &[execute_rotation_ix], &[])?;
    println!("Execute tx:           {}", execute_rotation_signature);

    let pending_rotation_snapshot = read_vault_snapshot(&client, vault_pda)?;
    let proposal_snapshot =
        read_proposal_snapshot(&client, &config.vault_program_id, vault_pda, rotation_proposal_id)?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_EXECUTED && proposal_snapshot.executed_slot != 0,
        "member rotation proposal did not enter executed state"
    );
    ensure!(
        pending_rotation_snapshot.config_version == updated_vault_snapshot.config_version,
        "vault config version changed unexpectedly during pending member rotation"
    );
    ensure!(
        pending_rotation_snapshot.threshold == updated_vault_snapshot.threshold,
        "vault threshold changed unexpectedly during member rotation"
    );
    ensure!(
        pending_rotation_snapshot.proposal_ttl_slots == updated_vault_snapshot.proposal_ttl_slots,
        "vault proposal ttl changed unexpectedly during member rotation"
    );
    ensure!(
        pending_rotation_snapshot.timelock_slots == updated_vault_snapshot.timelock_slots,
        "vault timelock changed unexpectedly during member rotation"
    );
    assert_pending_member(
        &pending_rotation_snapshot,
        ROTATED_MEMBER_INDEX,
        replacement_member.vector_pda,
    )?;
    assert_enabled_members(&pending_rotation_snapshot, &member_pdas)?;

    println!();
    println!("Accept pending member rotation (new member)");
    let accept_member_sub_ix = create_accept_pending_member_instruction(
        config.vault_program_id,
        replacement_member.vector_pda,
        vault_pda,
    );
    let accept_member_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &replacement_member,
        &[accept_member_sub_ix],
    )?;
    println!("Accept member tx:     {}", accept_member_result.signature);
    println!("Accept digest hex:    {}", hex::encode(accept_member_result.digest));

    let rotated_vault_snapshot = read_vault_snapshot(&client, vault_pda)?;
    ensure!(
        rotated_vault_snapshot.config_version == updated_vault_snapshot.config_version + 1,
        "vault config version {} did not increment from {} after member acceptance",
        rotated_vault_snapshot.config_version,
        updated_vault_snapshot.config_version
    );
    assert_no_pending_member(&rotated_vault_snapshot)?;
    assert_enabled_members(&rotated_vault_snapshot, &rotated_member_pdas)?;

    println!();
    println!("Member rotation success");
    println!("Proposal ID:          {}", rotation_proposal_id);
    println!("Proposal action hash: {}", hex::encode(rotation_action_hash));
    println!(
        "Rotated member index: {} -> {}",
        ROTATED_MEMBER_INDEX, replacement_member.vector_pda
    );
    println!("Executed slot:        {}", proposal_snapshot.executed_slot);

    println!();
    println!("Create post-rotation recipient");
    let rotated_recipient = Keypair::new();
    let rotated_recipient_signature = create_recipient(&client, &fee_payer, &rotated_recipient)?;
    println!("Recipient create tx:  {}", rotated_recipient_signature);
    println!("Recipient:            {}", rotated_recipient.pubkey());

    let post_rotation_proposal_id = rotated_vault_snapshot.next_proposal_id;
    let post_rotation_slot = client
        .get_slot()
        .context("failed to fetch current slot before post-rotation proposal")?;
    let post_rotation_not_before_slot =
        post_rotation_slot.saturating_add(config.proposal_offset_slots);
    let post_rotation_expires_at_slot = post_rotation_not_before_slot
        .saturating_add(rotated_vault_snapshot.proposal_ttl_slots);
    let post_rotation_action_payload = create_sol_withdraw_action_payload(
        rotated_recipient.pubkey(),
        DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS,
    );
    let post_rotation_action_hash = compute_action_hash(
        &vault_pda,
        rotated_vault_snapshot.config_version,
        post_rotation_proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        post_rotation_not_before_slot,
        post_rotation_expires_at_slot,
        &post_rotation_action_payload,
    );

    println!();
    println!("Create post-rotation SOL withdraw proposal (new member)");
    let create_post_rotation_sub_ix = create_create_proposal_from_member_instruction(
        config.vault_program_id,
        replacement_member.vector_pda,
        fee_payer.pubkey(),
        vault_pda,
        post_rotation_proposal_id,
        ACTION_KIND_SOL_WITHDRAW,
        post_rotation_not_before_slot,
        post_rotation_expires_at_slot,
        post_rotation_action_hash,
        &post_rotation_action_payload,
    );
    let create_post_rotation_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &replacement_member,
        &[create_post_rotation_sub_ix],
    )?;
    println!("Create proposal tx:   {}", create_post_rotation_result.signature);
    println!(
        "Create digest hex:    {}",
        hex::encode(create_post_rotation_result.digest)
    );

    let proposal_snapshot = read_proposal_snapshot(
        &client,
        &config.vault_program_id,
        vault_pda,
        post_rotation_proposal_id,
    )?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "post-rotation proposal was not created with the new member's approval"
    );

    println!();
    println!("Verify rotated-out member can no longer approve");
    let stale_approve_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[ROTATED_MEMBER_INDEX].vector_pda,
        vault_pda,
        post_rotation_proposal_id,
        post_rotation_action_hash,
    );
    match submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[ROTATED_MEMBER_INDEX],
        &[stale_approve_sub_ix],
    ) {
        Ok(result) => bail!(
            "rotated-out member unexpectedly approved post-rotation proposal with tx {}",
            result.signature
        ),
        Err(err) => println!("Old member approve rejected: {err:#}"),
    }

    let proposal_snapshot = read_proposal_snapshot(
        &client,
        &config.vault_program_id,
        vault_pda,
        post_rotation_proposal_id,
    )?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE && proposal_snapshot.approval_count == 1,
        "post-rotation proposal approval count changed after stale member rejection"
    );

    println!();
    println!("Approve post-rotation proposal (member 0)");
    let approve_post_rotation_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[0].vector_pda,
        vault_pda,
        post_rotation_proposal_id,
        post_rotation_action_hash,
    );
    let approve_post_rotation_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[0],
        &[approve_post_rotation_sub_ix],
    )?;
    println!("Approve proposal tx:  {}", approve_post_rotation_result.signature);
    println!(
        "Approve digest hex:   {}",
        hex::encode(approve_post_rotation_result.digest)
    );

    println!();
    println!("Approve post-rotation proposal (member 1)");
    let approve_post_rotation_final_sub_ix = create_approve_proposal_from_member_instruction(
        config.vault_program_id,
        members[1].vector_pda,
        vault_pda,
        post_rotation_proposal_id,
        post_rotation_action_hash,
    );
    let approve_post_rotation_final_result = submit_member_passthrough(
        &client,
        &fee_payer,
        &config,
        &members[1],
        &[approve_post_rotation_final_sub_ix],
    )?;
    println!(
        "Approve proposal tx:  {}",
        approve_post_rotation_final_result.signature
    );
    println!(
        "Approve digest hex:   {}",
        hex::encode(approve_post_rotation_final_result.digest)
    );

    let proposal_snapshot = read_proposal_snapshot(
        &client,
        &config.vault_program_id,
        vault_pda,
        post_rotation_proposal_id,
    )?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_ACTIVE
            && proposal_snapshot.approval_count == rotated_vault_snapshot.threshold,
        "post-rotation proposal approvals did not reach {}-of-{}",
        rotated_vault_snapshot.threshold,
        MAX_MEMBERS
    );

    println!();
    println!("Wait for post-rotation proposal not-before slot");
    println!(
        "Current slot:         {}",
        client.get_slot().context("failed to fetch slot")?
    );
    println!("Not-before slot:      {}", post_rotation_not_before_slot);
    wait_for_slot(&client, post_rotation_not_before_slot)?;

    let vault_balance_before_post_rotation = client
        .get_balance(&vault_pda)
        .context("failed to fetch vault balance before post-rotation execute")?;
    let rotated_recipient_balance_before = client
        .get_balance(&rotated_recipient.pubkey())
        .context("failed to fetch rotated recipient balance before execute")?;

    println!();
    println!("Execute post-rotation SOL withdraw proposal");
    let execute_post_rotation_ix = create_execute_proposal_instruction(
        config.vault_program_id,
        vault_pda,
        post_rotation_proposal_id,
        post_rotation_action_hash,
        &[AccountMeta::new(rotated_recipient.pubkey(), false)],
    );
    let execute_post_rotation_signature =
        send_transaction(&client, &fee_payer, &[execute_post_rotation_ix], &[])?;
    println!("Execute tx:           {}", execute_post_rotation_signature);

    let vault_balance_after_post_rotation = client
        .get_balance(&vault_pda)
        .context("failed to fetch vault balance after post-rotation execute")?;
    let rotated_recipient_balance_after = client
        .get_balance(&rotated_recipient.pubkey())
        .context("failed to fetch rotated recipient balance after execute")?;
    let proposal_snapshot = read_proposal_snapshot(
        &client,
        &config.vault_program_id,
        vault_pda,
        post_rotation_proposal_id,
    )?;
    ensure!(
        proposal_snapshot.state == PROPOSAL_STATE_EXECUTED && proposal_snapshot.executed_slot != 0,
        "post-rotation proposal did not enter executed state"
    );
    ensure!(
        vault_balance_before_post_rotation.saturating_sub(vault_balance_after_post_rotation)
            == DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS,
        "vault balance delta {} did not match post-rotation withdraw amount {}",
        vault_balance_before_post_rotation.saturating_sub(vault_balance_after_post_rotation),
        DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS
    );
    ensure!(
        rotated_recipient_balance_after.saturating_sub(rotated_recipient_balance_before)
            == DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS,
        "post-rotation recipient delta {} did not match withdraw amount {}",
        rotated_recipient_balance_after.saturating_sub(rotated_recipient_balance_before),
        DEFAULT_POST_ROTATION_WITHDRAW_LAMPORTS
    );

    println!();
    println!("Post-rotation approval success");
    println!("Proposal ID:          {}", post_rotation_proposal_id);
    println!(
        "Proposal action hash: {}",
        hex::encode(post_rotation_action_hash)
    );
    println!(
        "Vault balance:        {} -> {}",
        vault_balance_before_post_rotation, vault_balance_after_post_rotation
    );
    println!(
        "Recipient balance:    {} -> {}",
        rotated_recipient_balance_before, rotated_recipient_balance_after
    );
    println!("Executed slot:        {}", proposal_snapshot.executed_slot);

    println!();
    println!("Success");
    println!("SOL withdraw tx:      {}", execute_signature);
    println!("SPL transfer tx:      {}", execute_spl_signature);
    println!("Config update tx:     {}", execute_config_signature);
    println!("Member rotation tx:   {}", execute_rotation_signature);
    println!("Post-rotation tx:     {}", execute_post_rotation_signature);

    Ok(())
}

fn parse_args() -> Result<Config> {
    let mut rpc_url =
        env::var("VECTOR_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.to_string());
    let mut fee_payer_file = default_fee_payer_file();
    let mut vector_program_id =
        Pubkey::from_str(DEFAULT_VECTOR_PROGRAM_ID).context("invalid default vector program id")?;
    let mut vault_program_id =
        Pubkey::from_str(DEFAULT_VAULT_PROGRAM_ID).context("invalid default vault program id")?;
    let mut vault_seed = default_vault_seed();
    let mut withdraw_lamports = DEFAULT_WITHDRAW_LAMPORTS;
    let mut vault_buffer_lamports = DEFAULT_VAULT_BUFFER_LAMPORTS;
    let mut proposal_offset_slots = DEFAULT_PROPOSAL_OFFSET_SLOTS;
    let mut proposal_ttl_slots = DEFAULT_PROPOSAL_TTL_SLOTS;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rpc-url" => rpc_url = args.next().context("missing value for --rpc-url")?,
            "--fee-payer-file" => {
                fee_payer_file = Some(PathBuf::from(
                    args.next().context("missing value for --fee-payer-file")?,
                ))
            }
            "--vector-program-id" => {
                vector_program_id = Pubkey::from_str(
                    &args
                        .next()
                        .context("missing value for --vector-program-id")?,
                )
                .context("invalid --vector-program-id")?
            }
            "--vault-program-id" => {
                vault_program_id = Pubkey::from_str(
                    &args
                        .next()
                        .context("missing value for --vault-program-id")?,
                )
                .context("invalid --vault-program-id")?
            }
            "--vault-seed" => {
                vault_seed = args
                    .next()
                    .context("missing value for --vault-seed")?
                    .into_bytes();
            }
            "--withdraw-lamports" => {
                withdraw_lamports = args
                    .next()
                    .context("missing value for --withdraw-lamports")?
                    .parse()
                    .context("invalid --withdraw-lamports")?;
            }
            "--vault-buffer-lamports" => {
                vault_buffer_lamports = args
                    .next()
                    .context("missing value for --vault-buffer-lamports")?
                    .parse()
                    .context("invalid --vault-buffer-lamports")?;
            }
            "--proposal-offset-slots" => {
                proposal_offset_slots = args
                    .next()
                    .context("missing value for --proposal-offset-slots")?
                    .parse()
                    .context("invalid --proposal-offset-slots")?;
            }
            "--proposal-ttl-slots" => {
                proposal_ttl_slots = args
                    .next()
                    .context("missing value for --proposal-ttl-slots")?
                    .parse()
                    .context("invalid --proposal-ttl-slots")?;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => bail!("unknown argument: {arg}"),
        }
    }

    ensure!(
        !vault_seed.is_empty() && vault_seed.len() <= 32,
        "--vault-seed must be between 1 and 32 bytes"
    );
    let fee_payer_file =
        fee_payer_file.context("missing --fee-payer-file and no default Solana keypair found")?;

    Ok(Config {
        rpc_url,
        fee_payer_file,
        vector_program_id,
        vault_program_id,
        vault_seed,
        withdraw_lamports,
        vault_buffer_lamports,
        proposal_offset_slots,
        proposal_ttl_slots,
        commitment: CommitmentConfig::confirmed(),
    })
}

fn print_usage() {
    println!(
        "Usage:\n  cargo run --manifest-path tools/pq-multisig-harness/Cargo.toml -- [options]\n\n\
Options:\n  --rpc-url <url>                RPC endpoint. Default: {DEFAULT_RPC_URL}\n  --fee-payer-file <path>        Local Solana keypair file for fee payer\n  --vector-program-id <pubkey>   Vector Falcon program id\n  --vault-program-id <pubkey>    PQ vault program id\n  --vault-seed <string>          PDA seed for the vault. Default: {DEFAULT_VAULT_SEED_PREFIX}<unix-seconds>\n  --withdraw-lamports <n>        SOL withdraw amount. Default: {DEFAULT_WITHDRAW_LAMPORTS}\n  --vault-buffer-lamports <n>    Extra lamports to leave above rent after funding. Default: {DEFAULT_VAULT_BUFFER_LAMPORTS}\n  --proposal-offset-slots <n>    Slots to wait before proposal execution. Default: {DEFAULT_PROPOSAL_OFFSET_SLOTS}\n  --proposal-ttl-slots <n>       Proposal ttl in slots. Default: {DEFAULT_PROPOSAL_TTL_SLOTS}\n  -h, --help                     Show this help text"
    );
}

fn default_fee_payer_file() -> Option<PathBuf> {
    if let Ok(path) = env::var("SOLANA_KEYPAIR") {
        return Some(PathBuf::from(path));
    }
    let home = env::var("HOME").ok()?;
    let default = Path::new(&home).join(".config/solana/id.json");
    default.exists().then_some(default)
}

fn default_vault_seed() -> Vec<u8> {
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("{DEFAULT_VAULT_SEED_PREFIX}{unix_seconds}").into_bytes()
}

fn generate_members(vector_program_id: Pubkey) -> [Member; 3] {
    core::array::from_fn(|index| generate_member(index, vector_program_id))
}

fn generate_member(index: usize, vector_program_id: Pubkey) -> Member {
    let (public_key, secret_key) = falcon512::keypair();
    let wire_pubkey = wire_pubkey(&public_key);
    let identity = falcon_identity(&wire_pubkey);
    let (vector_pda, _) = find_vector_pda(&vector_program_id, &identity);
    Member {
        index,
        secret_key,
        wire_pubkey,
        identity,
        vector_pda,
    }
}

fn initialize_member_vector(
    client: &RpcClient,
    fee_payer: &Keypair,
    config: &Config,
    member: &Member,
) -> Result<Signature> {
    let ix = create_initialize_falcon512(
        config.vector_program_id,
        fee_payer.pubkey(),
        &member.wire_pubkey,
    );
    send_transaction(client, fee_payer, &[ix], &[])
}

fn initialize_vault(
    client: &RpcClient,
    fee_payer: &Keypair,
    config: &Config,
    members: &[Pubkey; 3],
) -> Result<Signature> {
    let ix = create_initialize_vault_instruction(
        config.vault_program_id,
        fee_payer.pubkey(),
        &config.vault_seed,
        2,
        config.proposal_ttl_slots,
        0,
        members,
    );
    send_transaction(client, fee_payer, &[ix], &[])
}

fn create_recipient(client: &RpcClient, fee_payer: &Keypair, recipient: &Keypair) -> Result<Signature> {
    let ix = system_create_account(
        &fee_payer.pubkey(),
        &recipient.pubkey(),
        DEFAULT_RECIPIENT_LAMPORTS,
        0,
        &system_program_pubkey()?,
    );
    let recipient_signer: &dyn Signer = recipient;
    send_transaction(client, fee_payer, &[ix], &[recipient_signer])
}

fn fund_vault(
    client: &RpcClient,
    fee_payer: &Keypair,
    vault: Pubkey,
    desired_balance: u64,
) -> Result<Signature> {
    let current_balance = client
        .get_balance(&vault)
        .context("failed to fetch current vault balance")?;
    let top_up_amount = desired_balance.saturating_sub(current_balance);
    ensure!(top_up_amount > 0, "vault already has at least the desired balance");

    let ix = create_system_transfer_instruction(fee_payer.pubkey(), vault, top_up_amount)?;
    send_transaction(client, fee_payer, &[ix], &[])
}

fn submit_member_passthrough(
    client: &RpcClient,
    fee_payer: &Keypair,
    config: &Config,
    member: &Member,
    subinstructions: &[Instruction],
) -> Result<MemberAdvanceResult> {
    let nonce = read_vector_nonce(client, member.vector_pda)?;
    let normalized_subinstructions =
        normalize_member_passthrough_subinstructions(subinstructions, member.vector_pda);
    let passthrough_ix =
        create_passthrough_instruction(
            config.vector_program_id,
            &member.identity,
            &normalized_subinstructions,
        )?;
    let digest = advance_vector_digest(
        config.vector_program_id,
        &member.identity,
        &nonce,
        &[],
        &[passthrough_ix.clone()],
        Some(fee_payer.pubkey()),
    );
    let signature = sign_falcon_digest(&digest, &member.secret_key);
    let advance_ix =
        create_advance_instruction(config.vector_program_id, &member.identity, &signature)?;
    let tx_signature = send_transaction(client, fee_payer, &[advance_ix, passthrough_ix], &[])?;
    Ok(MemberAdvanceResult {
        signature: tx_signature,
        digest,
    })
}

fn normalize_member_passthrough_subinstructions(
    subinstructions: &[Instruction],
    _member_vector_pda: Pubkey,
) -> Vec<Instruction> {
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
    fee_payer: &Keypair,
    instructions: &[Instruction],
    extra_signers: &[&dyn Signer],
) -> Result<Signature> {
    let blockhash = client
        .get_latest_blockhash()
        .context("failed to fetch latest blockhash")?;
    let mut signers: Vec<&dyn Signer> = Vec::with_capacity(1 + extra_signers.len());
    signers.push(fee_payer);
    signers.extend_from_slice(extra_signers);
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&fee_payer.pubkey()),
        &signers,
        blockhash,
    );
    client
        .send_and_confirm_transaction(&tx)
        .context("failed to send and confirm transaction")
}

fn wait_for_slot(client: &RpcClient, target_slot: u64) -> Result<()> {
    loop {
        let current_slot = client.get_slot().context("failed to fetch current slot")?;
        if current_slot >= target_slot {
            return Ok(());
        }
        sleep(Duration::from_millis(200));
    }
}

fn wait_for_balance_at_least(client: &RpcClient, account: Pubkey, minimum_balance: u64) -> Result<()> {
    for _ in 0..100 {
        let balance = client
            .get_balance(&account)
            .with_context(|| format!("failed to fetch balance for {account}"))?;
        if balance >= minimum_balance {
            return Ok(());
        }
        sleep(Duration::from_millis(200));
    }
    bail!(
        "balance for {} did not reach {} lamports after airdrop",
        account,
        minimum_balance
    );
}

fn is_localnet_rpc(rpc_url: &str) -> bool {
    rpc_url.contains("127.0.0.1") || rpc_url.contains("localhost")
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
    Ok(ProposalSnapshot {
        state: data[1],
        approval_count: data[4],
        executed_slot: read_u64_at(&data, 144)?,
    })
}

fn read_vector_nonce(client: &RpcClient, vector_pda: Pubkey) -> Result<[u8; 32]> {
    let data = client
        .get_account_data(&vector_pda)
        .with_context(|| format!("failed to fetch vector account data for {vector_pda}"))?;
    ensure!(data.len() >= 32, "vector account data too short");
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&data[..32]);
    Ok(nonce)
}

fn create_initialize_falcon512(
    vector_program_id: Pubkey,
    payer: Pubkey,
    wire_pubkey: &[u8; FALCON_512_PUBKEY_LEN],
) -> Instruction {
    let identity = falcon_identity(wire_pubkey);
    let (vector_pda, _) = find_vector_pda(&vector_program_id, &identity);
    let mut data = Vec::with_capacity(1 + wire_pubkey.len());
    data.push(INITIALIZE_VECTOR_DISCRIMINATOR);
    data.extend_from_slice(wire_pubkey);
    Instruction {
        program_id: vector_program_id,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(system_program_pubkey().expect("system program id"), false),
        ],
        data,
    }
}

fn create_advance_instruction(
    vector_program_id: Pubkey,
    identity: &[u8],
    signature: &[u8],
) -> Result<Instruction> {
    let (vector_pda, _) = find_vector_pda(&vector_program_id, identity);
    Ok(Instruction {
        program_id: vector_program_id,
        accounts: vec![
            AccountMeta::new(vector_pda, false),
            AccountMeta::new_readonly(instructions_sysvar_pubkey()?, false),
        ],
        data: {
            let mut out = Vec::with_capacity(1 + signature.len());
            out.push(ADVANCE_VECTOR_DISCRIMINATOR);
            out.extend_from_slice(signature);
            out
        },
    })
}

fn create_passthrough_instruction(
    vector_program_id: Pubkey,
    identity: &[u8],
    instructions: &[Instruction],
) -> Result<Instruction> {
    ensure!(
        instructions.len() <= u8::MAX as usize,
        "too many subinstructions"
    );

    let (vector_pda, _) = find_vector_pda(&vector_program_id, identity);
    let flattened_accounts: usize = instructions.iter().map(|ix| 1 + ix.accounts.len()).sum();
    let mut accounts = Vec::with_capacity(2 + flattened_accounts);
    accounts.push(AccountMeta::new(vector_pda, false));
    accounts.push(AccountMeta::new_readonly(instructions_sysvar_pubkey()?, false));
    for ix in instructions {
        accounts.push(AccountMeta::new_readonly(ix.program_id, false));
        accounts.extend(ix.accounts.iter().cloned());
    }

    let mut data = Vec::new();
    data.push(PASSTHROUGH_VECTOR_DISCRIMINATOR);
    data.push(instructions.len() as u8);
    for ix in instructions {
        ensure!(ix.accounts.len() <= u8::MAX as usize, "subinstruction has too many accounts");
        ensure!(ix.data.len() <= u16::MAX as usize, "subinstruction data too long");
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

fn advance_vector_digest(
    vector_program_id: Pubkey,
    identity: &[u8],
    nonce: &[u8; 32],
    pre_instructions: &[Instruction],
    post_instructions: &[Instruction],
    fee_payer: Option<Pubkey>,
) -> [u8; 32] {
    let placeholder = vec![0u8; FALCON_512_SIGNATURE_LEN];
    let advance_ix = create_advance_instruction(vector_program_id, identity, &placeholder)
        .expect("placeholder advance instruction");

    let mut all_owned = Vec::with_capacity(pre_instructions.len() + 1 + post_instructions.len());
    all_owned.extend(pre_instructions.iter().cloned());
    let advance_index = all_owned.len();
    all_owned.push(advance_ix);
    all_owned.extend(post_instructions.iter().cloned());

    let promoted = promote_to_message_flags(&all_owned, fee_payer);
    let borrowed_ixs: Vec<BorrowedInstruction> = promoted
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

    let mut buffer = construct_instructions_data(&borrowed_ixs);
    let footer_offset = buffer
        .len()
        .checked_sub(2)
        .expect("instructions sysvar buffer too short");
    buffer[footer_offset..].copy_from_slice(&(advance_index as u16).to_le_bytes());

    let ix_offset_pos = 2 + 2 * advance_index;
    let ix_offset = u16::from_le_bytes(
        buffer[ix_offset_pos..ix_offset_pos + 2]
            .try_into()
            .expect("vector buffer header truncated"),
    ) as usize;
    let num_accounts = promoted[advance_index].accounts.len();
    let sig_start = ix_offset + 2 + 33 * num_accounts + 32 + 2 + 1;
    let sig_end = sig_start + FALCON_512_SIGNATURE_LEN;

    let mut hasher = Sha256::new();
    hasher.update(&buffer[..sig_start]);
    hasher.update(nonce);
    hasher.update(identity);
    hasher.update(&buffer[sig_end..]);
    hasher.finalize().into()
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

fn create_initialize_vault_instruction(
    vault_program_id: Pubkey,
    payer: Pubkey,
    vault_seed: &[u8],
    threshold: u8,
    proposal_ttl_slots: u64,
    timelock_slots: u64,
    members: &[Pubkey; 3],
) -> Instruction {
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

    Instruction {
        program_id: vault_program_id,
        accounts: {
            let mut accounts = vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program_pubkey().expect("system program id"), false),
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
    }
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
            AccountMeta::new_readonly(system_program_pubkey().expect("system program id"), false),
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

fn falcon_identity(wire_pubkey: &[u8; FALCON_512_PUBKEY_LEN]) -> [u8; 32] {
    Sha256::digest(wire_pubkey).into()
}

fn create_system_transfer_instruction(from: Pubkey, to: Pubkey, lamports: u64) -> Result<Instruction> {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&SYSTEM_TRANSFER_DISCRIMINATOR.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());

    Ok(Instruction {
        program_id: system_program_pubkey()?,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data,
    })
}

fn system_program_pubkey() -> Result<Pubkey> {
    Pubkey::from_str(SYSTEM_PROGRAM_ID).context("failed to parse system program id")
}

fn instructions_sysvar_pubkey() -> Result<Pubkey> {
    Pubkey::from_str(INSTRUCTIONS_SYSVAR_ID).context("failed to parse instructions sysvar id")
}

fn read_u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let end = offset.checked_add(8).context("u64 offset overflow")?;
    let bytes = data.get(offset..end).context("u64 out of bounds")?;
    Ok(u64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("failed to read u64 at offset {offset}"))?,
    ))
}

fn read_pubkey_at(data: &[u8], offset: usize) -> Result<Pubkey> {
    let end = offset.checked_add(32).context("pubkey offset overflow")?;
    let bytes = data.get(offset..end).context("pubkey out of bounds")?;
    Pubkey::try_from(bytes)
        .map_err(|err| anyhow::anyhow!("failed to read pubkey at offset {offset}: {err}"))
}

fn setup_spl_fixture(
    client: &RpcClient,
    fee_payer: &Keypair,
    vault: Pubkey,
    destination_owner: Pubkey,
    decimals: u8,
    mint_amount: u64,
) -> Result<SplFixture> {
    let token_program = spl_token_program_id();
    let mint = Keypair::new();
    let source_ata =
        get_associated_token_address_with_program_id(&vault, &mint.pubkey(), &token_program);
    let destination_ata = get_associated_token_address_with_program_id(
        &destination_owner,
        &mint.pubkey(),
        &token_program,
    );
    let mint_rent = client
        .get_minimum_balance_for_rent_exemption(SplTokenMint::LEN)
        .context("failed to fetch mint rent floor")?;

    let create_mint_ix = system_create_account(
        &fee_payer.pubkey(),
        &mint.pubkey(),
        mint_rent,
        SplTokenMint::LEN as u64,
        &token_program,
    );
    let initialize_mint_ix = spl_initialize_mint2(
        &token_program,
        &mint.pubkey(),
        &fee_payer.pubkey(),
        None,
        decimals,
    )
    .map_err(|err| anyhow::anyhow!("failed to build initialize_mint2 instruction: {err}"))?;
    let create_source_ata_ix = create_associated_token_account_idempotent(
        &fee_payer.pubkey(),
        &vault,
        &mint.pubkey(),
        &token_program,
    );
    let create_destination_ata_ix = create_associated_token_account_idempotent(
        &fee_payer.pubkey(),
        &destination_owner,
        &mint.pubkey(),
        &token_program,
    );
    let mint_signer: &dyn Signer = &mint;
    let setup_signature = send_transaction(
        client,
        fee_payer,
        &[
            create_mint_ix,
            initialize_mint_ix,
            create_source_ata_ix,
            create_destination_ata_ix,
        ],
        &[mint_signer],
    )?;

    let mint_to_ix = spl_mint_to(
        &token_program,
        &mint.pubkey(),
        &source_ata,
        &fee_payer.pubkey(),
        &[],
        mint_amount,
    )
    .map_err(|err| anyhow::anyhow!("failed to build mint_to instruction: {err}"))?;
    let mint_to_signature = send_transaction(client, fee_payer, &[mint_to_ix], &[])?;

    Ok(SplFixture {
        token_program,
        mint: mint.pubkey(),
        source_ata,
        destination_ata,
        decimals,
        mint_amount,
        setup_signature,
        mint_to_signature,
    })
}

fn read_token_amount(client: &RpcClient, token_account: Pubkey) -> Result<u64> {
    let balance = client
        .get_token_account_balance(&token_account)
        .with_context(|| format!("failed to fetch token balance for {token_account}"))?;
    balance.amount.parse().with_context(|| {
        format!(
            "failed to parse raw token amount '{}' for {token_account}",
            balance.amount
        )
    })
}

fn assert_enabled_members(
    snapshot: &VaultSnapshot,
    expected_members: &[Pubkey; MAX_MEMBERS],
) -> Result<()> {
    ensure!(
        snapshot.member_count as usize == MAX_MEMBERS,
        "vault member count {} did not match expected {}",
        snapshot.member_count,
        MAX_MEMBERS
    );
    for (index, expected) in expected_members.iter().enumerate() {
        let member = snapshot
            .members
            .get(index)
            .context("missing vault member snapshot")?;
        ensure!(
            member.member_pda == *expected && member.enabled && member.weight == 1,
            "vault member {} mismatch: expected {} enabled=1 weight=1, got {} enabled={} weight={}",
            index,
            expected,
            member.member_pda,
            member.enabled,
            member.weight
        );
    }
    Ok(())
}

fn assert_no_pending_member(snapshot: &VaultSnapshot) -> Result<()> {
    ensure!(
        snapshot.pending_member_index == NO_PENDING_MEMBER_INDEX
            && snapshot.pending_member_pda == Pubkey::default(),
        "expected no pending member, got index {} pda {}",
        snapshot.pending_member_index,
        snapshot.pending_member_pda
    );
    Ok(())
}

fn assert_pending_member(snapshot: &VaultSnapshot, member_index: usize, member_pda: Pubkey) -> Result<()> {
    ensure!(
        snapshot.pending_member_index as usize == member_index && snapshot.pending_member_pda == member_pda,
        "expected pending member {} -> {}, got {} -> {}",
        member_index,
        member_pda,
        snapshot.pending_member_index,
        snapshot.pending_member_pda
    );
    Ok(())
}

fn assert_account_missing(client: &RpcClient, account: Pubkey) -> Result<()> {
    let balance = client
        .get_balance(&account)
        .with_context(|| format!("failed to fetch balance for account {account}"))?;
    ensure!(
        balance == 0,
        "account {} still has lamports after close",
        account
    );
    Ok(())
}
