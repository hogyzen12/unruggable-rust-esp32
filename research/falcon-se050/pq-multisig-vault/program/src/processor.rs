extern crate alloc;

use alloc::vec::Vec;

use pinocchio::{
    cpi::{invoke_signed_with_slice, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use pinocchio_system::instructions::CreateAccount;
use solana_address::address;
use solana_nostd_sha256::hashv;

use crate::{
    helpers::{read_address, read_u16, read_u64, read_u8, take_bytes},
    instruction::{ApprovalArgs, CreateProposalFromMemberArgs, InitializeVaultArgs},
    state::{
        MemberRecord, ProposalState, ProposalTxHeader, VaultConfig, PROPOSAL_STATE_ACTIVE,
        PROPOSAL_STATE_CANCELLED, PROPOSAL_STATE_EXECUTED, PROPOSAL_STATE_EXPIRED,
        NO_PENDING_MEMBER_INDEX,
    },
};

const ACTION_HASH_DOMAIN: &[u8] = b"UNRUGGABLE_PQ_MULTISIG_PROPOSAL_V1";
const VAULT_PDA_SEED: &[u8] = b"pq_vault";
const PROPOSAL_PDA_SEED: &[u8] = b"proposal";
const PROPOSAL_TX_PDA_SEED: &[u8] = b"proposal_tx";
const FLAG_WRITABLE: u8 = 1 << 0;
const FLAG_SIGNER_REQUESTED: u8 = 1 << 1;
const ZERO_ADDRESS: Address = Address::new_from_array([0u8; 32]);
const VECTOR_FALCON_PROGRAM_ID: Address =
    address!("FaLCn556dZWXHTXvkzVsieWfrvfZTki8rkFeHxaP8btU");
const VECTOR_FALCON_ACCOUNT_LEN: usize = 1090;

// Reserved for a future audited extension. Generic CPI bundle proposals are
// intentionally disabled in FalconOnly v1.
pub const ACTION_KIND_CPI_BUNDLE: u8 = 0;
pub const ACTION_KIND_REPLACE_MEMBER: u8 = 1;
pub const ACTION_KIND_SOL_WITHDRAW: u8 = 4;
pub const ACTION_KIND_SPL_TRANSFER: u8 = 5;
pub const ACTION_KIND_UPDATE_CONFIG: u8 = 6;
const SPL_TOKEN_TRANSFER_CHECKED_DISCRIMINATOR: u8 = 12;
const SPL_TOKEN_PROGRAM_ID: Address = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ASSOCIATED_TOKEN_PROGRAM_ID: Address =
    address!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub fn initialize_vault(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: InitializeVaultArgs,
) -> ProgramResult {
    let [payer, vault, _system_program, member_accounts @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !payer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if args.threshold == 0 || args.threshold > args.member_count {
        return Err(ProgramError::InvalidInstructionData);
    }
    if member_accounts.len() != args.member_count as usize {
        return Err(ProgramError::NotEnoughAccountKeys);
    }

    let vault_seed = &args.vault_seed[..args.vault_seed_len as usize];
    let (expected_vault, bump) = Address::find_program_address(&[VAULT_PDA_SEED, vault_seed], program_id);
    if vault.address() != &expected_vault {
        return Err(ProgramError::InvalidAccountData);
    }

    let lamports = pinocchio::sysvars::rent::Rent::get()?.try_minimum_balance(VaultConfig::LEN)?;
    let bump_arr = [bump];
    let seeds = [
        Seed::from(VAULT_PDA_SEED),
        Seed::from(vault_seed),
        Seed::from(&bump_arr),
    ];
    let signers = [Signer::from(&seeds)];

    CreateAccount {
        from: payer,
        to: vault,
        lamports,
        space: VaultConfig::LEN as u64,
        owner: program_id,
    }
    .invoke_signed(&signers)?;

    let mut members = [MemberRecord::disabled(); crate::state::MAX_MEMBERS];
    for (index, member) in members
        .iter_mut()
        .take(args.member_count as usize)
        .enumerate()
    {
        let member_address = args.members[index];
        if member_address == ZERO_ADDRESS {
            return Err(ProgramError::InvalidInstructionData);
        }
        if args.members[..index].contains(&member_address) {
            return Err(ProgramError::InvalidInstructionData);
        }
        let member_account = member_accounts
            .get(index)
            .ok_or(ProgramError::NotEnoughAccountKeys)?;
        if member_account.address() != &member_address {
            return Err(ProgramError::InvalidAccountData);
        }
        validate_vector_member_account(member_account)?;
        *member = MemberRecord {
            member_pda: member_address,
            enabled: 1,
            weight: 1,
        };
    }

    let state = VaultConfig {
        version: VaultConfig::VERSION,
        bump,
        threshold: args.threshold,
        member_count: args.member_count,
        vault_seed_len: args.vault_seed_len,
        pending_member_index: NO_PENDING_MEMBER_INDEX,
        config_version: 0,
        next_proposal_id: 0,
        timelock_slots: args.timelock_slots,
        proposal_ttl_slots: args.proposal_ttl_slots,
        pending_member_pda: ZERO_ADDRESS,
        vault_seed: args.vault_seed,
        members,
    };
    state.store(vault)?;
    Ok(())
}

pub fn create_proposal_from_member(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: CreateProposalFromMemberArgs<'_>,
) -> ProgramResult {
    let [member, payer, vault, proposal, proposal_tx, _system_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !member.is_signer() || !payer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    validate_vector_member_account(member)?;

    let mut vault_state = VaultConfig::load(vault, program_id)?;
    if !is_supported_action_kind(args.action_kind) {
        return Err(ProgramError::InvalidInstructionData);
    }
    let member_index = vault_state
        .member_index(member.address())
        .ok_or(ProgramError::MissingRequiredSignature)?;

    if args.proposal_id != vault_state.next_proposal_id {
        return Err(ProgramError::InvalidInstructionData);
    }

    let proposal_id_le = args.proposal_id.to_le_bytes();
    let (expected_proposal, proposal_bump) = Address::find_program_address(
        &[PROPOSAL_PDA_SEED, vault.address().as_ref(), &proposal_id_le],
        program_id,
    );
    if proposal.address() != &expected_proposal {
        return Err(ProgramError::InvalidAccountData);
    }
    let (expected_proposal_tx, proposal_tx_bump) = Address::find_program_address(
        &[PROPOSAL_TX_PDA_SEED, proposal.address().as_ref()],
        program_id,
    );
    if proposal_tx.address() != &expected_proposal_tx {
        return Err(ProgramError::InvalidAccountData);
    }

    let clock = Clock::get()?;
    let created_slot = clock.slot;
    let min_not_before = created_slot.saturating_add(vault_state.timelock_slots);
    let not_before_slot = if args.not_before_slot == 0 {
        min_not_before
    } else if args.not_before_slot < min_not_before {
        return Err(ProgramError::InvalidInstructionData);
    } else {
        args.not_before_slot
    };
    let expires_at_slot = if args.expires_at_slot == 0 {
        not_before_slot.saturating_add(vault_state.proposal_ttl_slots)
    } else {
        args.expires_at_slot
    };
    if expires_at_slot < not_before_slot {
        return Err(ProgramError::InvalidInstructionData);
    }

    let action_hash = compute_action_hash(
        vault.address(),
        vault_state.config_version,
        args.proposal_id,
        args.action_kind,
        not_before_slot,
        expires_at_slot,
        args.proposal_tx_bytes,
    );
    if action_hash != args.expected_action_hash {
        return Err(ProgramError::InvalidInstructionData);
    }

    let proposal_lamports =
        pinocchio::sysvars::rent::Rent::get()?.try_minimum_balance(ProposalState::LEN)?;
    let proposal_tx_len = ProposalTxHeader::account_len(args.proposal_tx_bytes.len());
    let proposal_tx_lamports =
        pinocchio::sysvars::rent::Rent::get()?.try_minimum_balance(proposal_tx_len)?;

    let proposal_bump_arr = [proposal_bump];
    let proposal_seeds = [
        Seed::from(PROPOSAL_PDA_SEED),
        Seed::from(vault.address().as_ref()),
        Seed::from(&proposal_id_le),
        Seed::from(&proposal_bump_arr),
    ];
    let proposal_signers = [Signer::from(&proposal_seeds)];
    CreateAccount {
        from: payer,
        to: proposal,
        lamports: proposal_lamports,
        space: ProposalState::LEN as u64,
        owner: program_id,
    }
    .invoke_signed(&proposal_signers)?;

    let proposal_tx_bump_arr = [proposal_tx_bump];
    let proposal_tx_seeds = [
        Seed::from(PROPOSAL_TX_PDA_SEED),
        Seed::from(proposal.address().as_ref()),
        Seed::from(&proposal_tx_bump_arr),
    ];
    let proposal_tx_signers = [Signer::from(&proposal_tx_seeds)];
    CreateAccount {
        from: payer,
        to: proposal_tx,
        lamports: proposal_tx_lamports,
        space: proposal_tx_len as u64,
        owner: program_id,
    }
    .invoke_signed(&proposal_tx_signers)?;

    let mut proposal_state = ProposalState {
        version: ProposalState::VERSION,
        state: PROPOSAL_STATE_ACTIVE,
        proposer_member_index: member_index as u8,
        approval_bitmap: 0,
        approval_count: 0,
        action_kind: args.action_kind,
        proposal_id: args.proposal_id,
        vault: *vault.address(),
        config_version: vault_state.config_version,
        created_slot,
        not_before_slot,
        expires_at_slot,
        action_hash,
        tx_account: *proposal_tx.address(),
        executed_slot: 0,
    };
    proposal_state.add_member_approval(member_index);
    proposal_state.store(proposal)?;

    let proposal_tx_header = ProposalTxHeader {
        version: ProposalTxHeader::VERSION,
        action_hash,
        data_len: args.proposal_tx_bytes.len() as u32,
    };
    proposal_tx_header.store_with_bytes(proposal_tx, args.proposal_tx_bytes)?;

    vault_state.next_proposal_id = vault_state.next_proposal_id.saturating_add(1);
    vault_state.store(vault)?;
    Ok(())
}

pub fn approve_proposal_from_member(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: ApprovalArgs,
) -> ProgramResult {
    let [member, vault, proposal] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !member.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    validate_vector_member_account(member)?;

    let vault_state = VaultConfig::load(vault, program_id)?;
    let member_index = vault_state
        .member_index(member.address())
        .ok_or(ProgramError::MissingRequiredSignature)?;
    let mut proposal_state = ProposalState::load(proposal, program_id)?;
    validate_proposal_active(
        &proposal_state,
        &vault_state,
        vault.address(),
        args.proposal_id,
        &args.expected_action_hash,
    )?;

    let current_slot = Clock::get()?.slot;
    if current_slot > proposal_state.expires_at_slot {
        proposal_state.state = PROPOSAL_STATE_EXPIRED;
        proposal_state.store(proposal)?;
        return Err(ProgramError::InvalidAccountData);
    }

    proposal_state.add_member_approval(member_index);
    proposal_state.store(proposal)?;
    Ok(())
}

pub fn execute_proposal(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: ApprovalArgs,
) -> ProgramResult {
    let [vault, proposal, proposal_tx, remaining @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    let mut vault_state = VaultConfig::load(vault, program_id)?;
    let mut proposal_state = ProposalState::load(proposal, program_id)?;
    validate_proposal_active(
        &proposal_state,
        &vault_state,
        vault.address(),
        args.proposal_id,
        &args.expected_action_hash,
    )?;

    if proposal_state.approval_count < vault_state.threshold {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let current_slot = Clock::get()?.slot;
    if current_slot > proposal_state.expires_at_slot {
        proposal_state.state = PROPOSAL_STATE_EXPIRED;
        proposal_state.store(proposal)?;
        return Err(ProgramError::InvalidAccountData);
    }
    if current_slot < proposal_state.not_before_slot {
        return Err(ProgramError::InvalidAccountData);
    }

    let proposal_tx_header = ProposalTxHeader::load(proposal_tx, program_id)?;
    if proposal_tx_header.action_hash != proposal_state.action_hash {
        return Err(ProgramError::InvalidAccountData);
    }
    let bundle_bytes = proposal_tx_header.proposal_bytes(proposal_tx)?;

    match proposal_state.action_kind {
        ACTION_KIND_CPI_BUNDLE => return Err(ProgramError::InvalidInstructionData),
        ACTION_KIND_SOL_WITHDRAW => {
            execute_sol_withdraw(vault, remaining, bundle_bytes)?;
        }
        ACTION_KIND_SPL_TRANSFER => {
            execute_spl_transfer(vault, &vault_state, remaining, bundle_bytes)?;
        }
        ACTION_KIND_REPLACE_MEMBER => {
            execute_replace_member(vault, &mut vault_state, bundle_bytes)?;
        }
        ACTION_KIND_UPDATE_CONFIG => {
            execute_update_config(vault, &mut vault_state, bundle_bytes)?;
        }
        _ => return Err(ProgramError::InvalidInstructionData),
    }

    proposal_state.state = PROPOSAL_STATE_EXECUTED;
    proposal_state.executed_slot = current_slot;
    proposal_state.store(proposal)?;
    Ok(())
}

pub fn cancel_proposal(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: ApprovalArgs,
) -> ProgramResult {
    let [member, vault, proposal] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !member.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    validate_vector_member_account(member)?;

    let vault_state = VaultConfig::load(vault, program_id)?;
    let member_index = vault_state
        .member_index(member.address())
        .ok_or(ProgramError::MissingRequiredSignature)?;
    let mut proposal_state = ProposalState::load(proposal, program_id)?;
    validate_proposal_active(
        &proposal_state,
        &vault_state,
        vault.address(),
        args.proposal_id,
        &args.expected_action_hash,
    )?;

    let current_slot = Clock::get()?.slot;
    if current_slot > proposal_state.expires_at_slot {
        proposal_state.state = PROPOSAL_STATE_EXPIRED;
    } else if proposal_state.proposer_member_index as usize == member_index
        && proposal_state.approval_count < vault_state.threshold
    {
        proposal_state.state = PROPOSAL_STATE_CANCELLED;
    } else {
        return Err(ProgramError::MissingRequiredSignature);
    }

    proposal_state.store(proposal)?;
    Ok(())
}

pub fn revoke_proposal_from_member(
    program_id: &Address,
    accounts: &mut [AccountView],
    args: ApprovalArgs,
) -> ProgramResult {
    let [member, vault, proposal] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !member.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    validate_vector_member_account(member)?;

    let vault_state = VaultConfig::load(vault, program_id)?;
    let member_index = vault_state
        .member_index(member.address())
        .ok_or(ProgramError::MissingRequiredSignature)?;
    let mut proposal_state = ProposalState::load(proposal, program_id)?;
    validate_proposal_active(
        &proposal_state,
        &vault_state,
        vault.address(),
        args.proposal_id,
        &args.expected_action_hash,
    )?;

    let current_slot = Clock::get()?.slot;
    if current_slot > proposal_state.expires_at_slot {
        proposal_state.state = PROPOSAL_STATE_EXPIRED;
        proposal_state.store(proposal)?;
        return Err(ProgramError::InvalidAccountData);
    }
    if proposal_state.proposer_member_index as usize == member_index {
        return Err(ProgramError::MissingRequiredSignature);
    }

    proposal_state.remove_member_approval(member_index)?;
    proposal_state.store(proposal)?;
    Ok(())
}

pub fn close_proposal(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let [vault, proposal, proposal_tx] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !vault.is_writable() || !proposal.is_writable() || !proposal_tx.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut proposal_state = ProposalState::load(proposal, program_id)?;
    if proposal_state.vault != *vault.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    let _vault_state = VaultConfig::load(vault, program_id)?;
    let current_slot = Clock::get()?.slot;
    let close_allowed = match proposal_state.state {
        PROPOSAL_STATE_EXECUTED | PROPOSAL_STATE_CANCELLED | PROPOSAL_STATE_EXPIRED => true,
        PROPOSAL_STATE_ACTIVE if current_slot > proposal_state.expires_at_slot => {
            proposal_state.state = PROPOSAL_STATE_EXPIRED;
            proposal_state.store(proposal)?;
            true
        }
        _ => false,
    };
    if !close_allowed {
        return Err(ProgramError::InvalidAccountData);
    }

    let proposal_tx_header = ProposalTxHeader::load(proposal_tx, program_id)?;
    if proposal_state.tx_account != *proposal_tx.address()
        || proposal_tx_header.action_hash != proposal_state.action_hash
    {
        return Err(ProgramError::InvalidAccountData);
    }

    close_program_account(vault, proposal)?;
    close_program_account(vault, proposal_tx)?;
    Ok(())
}

pub fn accept_pending_member(program_id: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    let [member, vault] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !member.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    validate_vector_member_account(member)?;

    let mut vault_state = VaultConfig::load(vault, program_id)?;
    let (member_index, pending_member_pda) = vault_state
        .pending_member()
        .ok_or(ProgramError::InvalidAccountData)?;
    if pending_member_pda != *member.address() || member_index >= vault_state.member_count as usize {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if vault_state
        .members
        .iter()
        .enumerate()
        .any(|(index, existing)| {
            index != member_index && existing.enabled != 0 && existing.member_pda == pending_member_pda
        })
    {
        return Err(ProgramError::InvalidInstructionData);
    }

    vault_state.members[member_index] = MemberRecord {
        member_pda: pending_member_pda,
        enabled: 1,
        weight: 1,
    };
    clear_pending_member(&mut vault_state);
    vault_state.config_version = vault_state.config_version.saturating_add(1);
    vault_state.store(vault)?;
    Ok(())
}

fn validate_proposal_active(
    proposal_state: &ProposalState,
    vault_state: &VaultConfig,
    vault_address: &Address,
    proposal_id: u64,
    expected_action_hash: &[u8; 32],
) -> Result<(), ProgramError> {
    if !proposal_state.is_active() {
        return Err(ProgramError::InvalidAccountData);
    }
    if proposal_state.proposal_id != proposal_id {
        return Err(ProgramError::InvalidInstructionData);
    }
    if proposal_state.vault != *vault_address {
        return Err(ProgramError::InvalidAccountData);
    }
    if proposal_state.config_version != vault_state.config_version {
        return Err(ProgramError::InvalidAccountData);
    }
    if &proposal_state.action_hash != expected_action_hash {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok(())
}

fn is_supported_action_kind(action_kind: u8) -> bool {
    matches!(
        action_kind,
        ACTION_KIND_SOL_WITHDRAW
            | ACTION_KIND_SPL_TRANSFER
            | ACTION_KIND_REPLACE_MEMBER
            | ACTION_KIND_UPDATE_CONFIG
    )
}

#[allow(dead_code)]
fn replay_bundle(
    vault: &AccountView,
    vault_state: &VaultConfig,
    remaining: &[AccountView],
    mut payload: &[u8],
) -> ProgramResult {
    let instruction_count = read_u16(&mut payload)? as usize;

    let bump_arr = [vault_state.bump];
    let seeds = [
        Seed::from(VAULT_PDA_SEED),
        Seed::from(vault_state.vault_seed()),
        Seed::from(&bump_arr),
    ];
    let signers = [Signer::from(&seeds)];

    let mut all_accounts = Vec::with_capacity(1 + remaining.len());
    all_accounts.push(vault.clone());
    all_accounts.extend_from_slice(remaining);

    for _ in 0..instruction_count {
        let program_address = read_address(&mut payload)?;
        let program_view = find_account(&all_accounts, &program_address)?;
        let account_count = read_u16(&mut payload)? as usize;

        let mut account_views = Vec::with_capacity(account_count);
        let mut metas = Vec::with_capacity(account_count);
        for _ in 0..account_count {
            let account_address = read_address(&mut payload)?;
            let flags = read_u8(&mut payload)?;
            let account_view = find_account(&all_accounts, &account_address)?;
            if (flags & FLAG_WRITABLE) != 0 && !account_view.is_writable() {
                return Err(ProgramError::InvalidAccountData);
            }
            if (flags & FLAG_SIGNER_REQUESTED) != 0 && account_address != *vault.address() {
                return Err(ProgramError::MissingRequiredSignature);
            }
            account_views.push(account_view.clone());
            metas.push(InstructionAccount::new(
                account_view.address(),
                (flags & FLAG_WRITABLE) != 0,
                (flags & FLAG_SIGNER_REQUESTED) != 0 && account_address == *vault.address(),
            ));
        }

        let data_len = read_u16(&mut payload)? as usize;
        let data = take_bytes(&mut payload, data_len)?;
        invoke_signed_with_slice(
            &InstructionView {
                program_id: program_view.address(),
                accounts: &metas,
                data,
            },
            &account_views,
            &signers,
        )?;
    }

    if !payload.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    Ok(())
}

fn execute_sol_withdraw(
    vault: &mut AccountView,
    remaining: &mut [AccountView],
    mut payload: &[u8],
) -> ProgramResult {
    let receiver_address = read_address(&mut payload)?;
    let lamports = u64::from_le_bytes(
        take_bytes(&mut payload, 8)?
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    );
    if !payload.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    let receiver = find_account_mut(remaining, &receiver_address)?;
    if !receiver.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }

    let rent_min = pinocchio::sysvars::rent::Rent::get()?.try_minimum_balance(vault.data_len())?;
    let new_vault_lamports = vault
        .lamports()
        .checked_sub(lamports)
        .ok_or(ProgramError::InsufficientFunds)?;
    if new_vault_lamports < rent_min {
        return Err(ProgramError::InsufficientFunds);
    }
    let new_receiver_lamports = receiver
        .lamports()
        .checked_add(lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    vault.set_lamports(new_vault_lamports);
    receiver.set_lamports(new_receiver_lamports);
    Ok(())
}

fn execute_spl_transfer(
    vault: &AccountView,
    vault_state: &VaultConfig,
    remaining: &[AccountView],
    mut payload: &[u8],
) -> ProgramResult {
    let mint_address = read_address(&mut payload)?;
    let destination_address = read_address(&mut payload)?;
    let amount = read_u64(&mut payload)?;
    let decimals = read_u8(&mut payload)?;
    if amount == 0 || !payload.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    let token_program = find_account(remaining, &SPL_TOKEN_PROGRAM_ID)?;
    let expected_source_address =
        find_associated_token_account(vault.address(), &mint_address, token_program.address());
    let source = find_account(remaining, &expected_source_address)?;
    let mint = find_account(remaining, &mint_address)?;
    let destination = find_account(remaining, &destination_address)?;
    if !source.is_writable() || !destination.is_writable() {
        return Err(ProgramError::InvalidAccountData);
    }
    validate_token_account_owner_mint(source, vault.address(), &mint_address)?;

    let bump_arr = [vault_state.bump];
    let seeds = [
        Seed::from(VAULT_PDA_SEED),
        Seed::from(vault_state.vault_seed()),
        Seed::from(&bump_arr),
    ];
    let signers = [Signer::from(&seeds)];

    let data = {
        let mut out = [0u8; 10];
        out[0] = SPL_TOKEN_TRANSFER_CHECKED_DISCRIMINATOR;
        out[1..9].copy_from_slice(&amount.to_le_bytes());
        out[9] = decimals;
        out
    };

    let account_views = [source.clone(), mint.clone(), destination.clone(), vault.clone()];
    let metas = [
        InstructionAccount::new(source.address(), true, false),
        InstructionAccount::new(mint.address(), false, false),
        InstructionAccount::new(destination.address(), true, false),
        InstructionAccount::new(vault.address(), false, true),
    ];

    invoke_signed_with_slice(
        &InstructionView {
            program_id: token_program.address(),
            accounts: &metas,
            data: &data,
        },
        &account_views,
        &signers,
    )?;

    Ok(())
}

fn execute_update_config(
    vault: &mut AccountView,
    vault_state: &mut VaultConfig,
    mut payload: &[u8],
) -> ProgramResult {
    let threshold = read_u8(&mut payload)?;
    let proposal_ttl_slots = read_u64(&mut payload)?;
    let timelock_slots = read_u64(&mut payload)?;
    if threshold == 0 || threshold > vault_state.member_count {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !payload.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    vault_state.threshold = threshold;
    vault_state.proposal_ttl_slots = proposal_ttl_slots;
    vault_state.timelock_slots = timelock_slots;
    vault_state.config_version = vault_state.config_version.saturating_add(1);
    vault_state.store(vault)?;
    Ok(())
}

fn execute_replace_member(
    vault: &mut AccountView,
    vault_state: &mut VaultConfig,
    mut payload: &[u8],
) -> ProgramResult {
    let member_index = read_u8(&mut payload)? as usize;
    let new_member_pda = read_address(&mut payload)?;
    if member_index >= vault_state.member_count as usize || new_member_pda == Address::new_from_array([0u8; 32]) {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !payload.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    let current_member = vault_state
        .members
        .get(member_index)
        .ok_or(ProgramError::InvalidInstructionData)?;
    if current_member.enabled == 0 || current_member.member_pda == new_member_pda {
        return Err(ProgramError::InvalidInstructionData);
    }
    if vault_state
        .members
        .iter()
        .enumerate()
        .any(|(index, member)| index != member_index && member.enabled != 0 && member.member_pda == new_member_pda)
    {
        return Err(ProgramError::InvalidInstructionData);
    }

    vault_state.pending_member_index = member_index as u8;
    vault_state.pending_member_pda = new_member_pda;
    vault_state.store(vault)?;
    Ok(())
}

fn find_account<'a>(accounts: &'a [AccountView], address: &Address) -> Result<&'a AccountView, ProgramError> {
    accounts
        .iter()
        .find(|account| account.address() == address)
        .ok_or(ProgramError::NotEnoughAccountKeys)
}

fn validate_token_account_owner_mint(
    token_account: &AccountView,
    owner_address: &Address,
    mint_address: &Address,
) -> Result<(), ProgramError> {
    if token_account.data_len() < 72 {
        return Err(ProgramError::InvalidAccountData);
    }
    let data = token_account.try_borrow()?;
    let data = &data[..];
    if data[0..32] != mint_address.as_ref()[..] || data[32..64] != owner_address.as_ref()[..] {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

fn find_associated_token_account(
    owner_address: &Address,
    mint_address: &Address,
    token_program_address: &Address,
) -> Address {
    let (associated_token_address, _bump) = Address::find_program_address(
        &[owner_address.as_ref(), token_program_address.as_ref(), mint_address.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    );
    associated_token_address
}

fn find_account_mut<'a>(
    accounts: &'a mut [AccountView],
    address: &Address,
) -> Result<&'a mut AccountView, ProgramError> {
    accounts
        .iter_mut()
        .find(|account| account.address() == address)
        .ok_or(ProgramError::NotEnoughAccountKeys)
}

fn clear_pending_member(vault_state: &mut VaultConfig) {
    vault_state.pending_member_index = NO_PENDING_MEMBER_INDEX;
    vault_state.pending_member_pda = ZERO_ADDRESS;
}

fn close_program_account(receiver: &mut AccountView, account: &mut AccountView) -> ProgramResult {
    let receiver_lamports = receiver
        .lamports()
        .checked_add(account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    receiver.set_lamports(receiver_lamports);
    account.set_lamports(0);
    let mut data = account.try_borrow_mut()?;
    data.fill(0);
    Ok(())
}

fn validate_vector_member_account(account: &AccountView) -> ProgramResult {
    if account.address() == &ZERO_ADDRESS {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !account.owned_by(&VECTOR_FALCON_PROGRAM_ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }
    if account.data_len() != VECTOR_FALCON_ACCOUNT_LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(())
}

fn compute_action_hash(
    vault: &Address,
    config_version: u64,
    proposal_id: u64,
    action_kind: u8,
    not_before_slot: u64,
    expires_at_slot: u64,
    bundle_bytes: &[u8],
) -> [u8; 32] {
    let config_version_le = config_version.to_le_bytes();
    let proposal_id_le = proposal_id.to_le_bytes();
    let action_kind_arr = [action_kind];
    let not_before_slot_le = not_before_slot.to_le_bytes();
    let expires_at_slot_le = expires_at_slot.to_le_bytes();
    let bundle_hash = hashv(&[bundle_bytes]);
    hashv(&[
        ACTION_HASH_DOMAIN,
        vault.as_ref(),
        &config_version_le,
        &proposal_id_le,
        &action_kind_arr,
        &not_before_slot_le,
        &expires_at_slot_le,
        &bundle_hash,
    ])
}
