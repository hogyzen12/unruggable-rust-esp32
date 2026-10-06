use pinocchio::error::ProgramError;
use solana_address::Address;

use crate::{
    helpers::{read_address, read_array, read_u32, read_u64, read_u8, take_bytes},
    state::MAX_MEMBERS,
};

pub const INITIALIZE_VAULT_DISCRIMINATOR: u8 = 0;
pub const CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 1;
pub const APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 2;
pub const EXECUTE_PROPOSAL_DISCRIMINATOR: u8 = 3;
pub const CANCEL_PROPOSAL_DISCRIMINATOR: u8 = 4;
pub const CLOSE_PROPOSAL_DISCRIMINATOR: u8 = 5;
pub const ACCEPT_PENDING_MEMBER_DISCRIMINATOR: u8 = 6;
pub const REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 7;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultInstruction {
    InitializeVault = INITIALIZE_VAULT_DISCRIMINATOR,
    CreateProposalFromMember = CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR,
    ApproveProposalFromMember = APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR,
    ExecuteProposal = EXECUTE_PROPOSAL_DISCRIMINATOR,
    CancelProposal = CANCEL_PROPOSAL_DISCRIMINATOR,
    CloseProposal = CLOSE_PROPOSAL_DISCRIMINATOR,
    AcceptPendingMember = ACCEPT_PENDING_MEMBER_DISCRIMINATOR,
    RevokeProposalFromMember = REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR,
}

impl TryFrom<&u8> for VaultInstruction {
    type Error = ProgramError;

    fn try_from(value: &u8) -> Result<Self, Self::Error> {
        match *value {
            INITIALIZE_VAULT_DISCRIMINATOR => Ok(Self::InitializeVault),
            CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR => Ok(Self::CreateProposalFromMember),
            APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR => Ok(Self::ApproveProposalFromMember),
            EXECUTE_PROPOSAL_DISCRIMINATOR => Ok(Self::ExecuteProposal),
            CANCEL_PROPOSAL_DISCRIMINATOR => Ok(Self::CancelProposal),
            CLOSE_PROPOSAL_DISCRIMINATOR => Ok(Self::CloseProposal),
            ACCEPT_PENDING_MEMBER_DISCRIMINATOR => Ok(Self::AcceptPendingMember),
            REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR => Ok(Self::RevokeProposalFromMember),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializeVaultArgs {
    pub vault_seed: [u8; 32],
    pub vault_seed_len: u8,
    pub threshold: u8,
    pub proposal_ttl_slots: u64,
    pub timelock_slots: u64,
    pub member_count: u8,
    pub members: [Address; MAX_MEMBERS],
}

impl InitializeVaultArgs {
    pub fn parse(mut payload: &[u8]) -> Result<Self, ProgramError> {
        let vault_seed_len = read_u8(&mut payload)?;
        if vault_seed_len as usize > 32 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut vault_seed = [0u8; 32];
        vault_seed[..vault_seed_len as usize]
            .copy_from_slice(take_bytes(&mut payload, vault_seed_len as usize)?);

        let threshold = read_u8(&mut payload)?;
        let proposal_ttl_slots = read_u64(&mut payload)?;
        let timelock_slots = read_u64(&mut payload)?;
        let member_count = read_u8(&mut payload)?;
        if member_count as usize > MAX_MEMBERS || member_count == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }

        let mut members = [Address::new_from_array([0u8; 32]); MAX_MEMBERS];
        for member in members.iter_mut().take(member_count as usize) {
            *member = read_address(&mut payload)?;
        }

        if !payload.is_empty() {
            return Err(ProgramError::InvalidInstructionData);
        }

        Ok(Self {
            vault_seed,
            vault_seed_len,
            threshold,
            proposal_ttl_slots,
            timelock_slots,
            member_count,
            members,
        })
    }
}

pub struct CreateProposalFromMemberArgs<'a> {
    pub proposal_id: u64,
    pub action_kind: u8,
    pub not_before_slot: u64,
    pub expires_at_slot: u64,
    pub expected_action_hash: [u8; 32],
    pub proposal_tx_bytes: &'a [u8],
}

impl<'a> CreateProposalFromMemberArgs<'a> {
    pub fn parse(mut payload: &'a [u8]) -> Result<Self, ProgramError> {
        let proposal_id = read_u64(&mut payload)?;
        let action_kind = read_u8(&mut payload)?;
        let not_before_slot = read_u64(&mut payload)?;
        let expires_at_slot = read_u64(&mut payload)?;
        let expected_action_hash = read_array::<32>(&mut payload)?;
        let tx_len = read_u32(&mut payload)? as usize;
        let proposal_tx_bytes = take_bytes(&mut payload, tx_len)?;
        if !payload.is_empty() {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            proposal_id,
            action_kind,
            not_before_slot,
            expires_at_slot,
            expected_action_hash,
            proposal_tx_bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalArgs {
    pub proposal_id: u64,
    pub expected_action_hash: [u8; 32],
}

impl ApprovalArgs {
    pub fn parse(mut payload: &[u8]) -> Result<Self, ProgramError> {
        let proposal_id = read_u64(&mut payload)?;
        let expected_action_hash = read_array::<32>(&mut payload)?;
        if !payload.is_empty() {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            proposal_id,
            expected_action_hash,
        })
    }
}

pub fn ensure_empty(payload: &[u8]) -> Result<(), ProgramError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(ProgramError::InvalidInstructionData)
    }
}
