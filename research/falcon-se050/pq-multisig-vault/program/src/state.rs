use pinocchio::{error::ProgramError, AccountView, Address};

use crate::helpers::{
    read_address, read_array, read_u32, read_u64, read_u8, take_bytes, write_address, write_bytes,
    write_u32, write_u64, write_u8,
};

pub const MAX_MEMBERS: usize = 3;
pub const VAULT_SEED_MAX_LEN: usize = 32;
pub const MEMBER_RESERVED_LEN: usize = 6;
pub const MEMBER_RECORD_LEN: usize = 32 + 1 + 1 + MEMBER_RESERVED_LEN;

pub const PROPOSAL_STATE_ACTIVE: u8 = 0;
pub const PROPOSAL_STATE_EXECUTED: u8 = 1;
pub const PROPOSAL_STATE_CANCELLED: u8 = 2;
pub const PROPOSAL_STATE_EXPIRED: u8 = 3;
pub const NO_PENDING_MEMBER_INDEX: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRecord {
    pub member_pda: Address,
    pub enabled: u8,
    pub weight: u8,
}

impl MemberRecord {
    pub const LEN: usize = MEMBER_RECORD_LEN;

    pub fn disabled() -> Self {
        Self {
            member_pda: Address::new_from_array([0u8; 32]),
            enabled: 0,
            weight: 0,
        }
    }

    pub fn read(mut data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let member_pda = read_address(&mut data)?;
        let enabled = read_u8(&mut data)?;
        let weight = read_u8(&mut data)?;
        let _reserved = take_bytes(&mut data, MEMBER_RESERVED_LEN)?;
        Ok(Self {
            member_pda,
            enabled,
            weight,
        })
    }

    pub fn write(&self, out: &mut [u8]) -> Result<(), ProgramError> {
        if out.len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut offset = 0usize;
        write_address(out, &mut offset, &self.member_pda)?;
        write_u8(out, &mut offset, self.enabled)?;
        write_u8(out, &mut offset, self.weight)?;
        write_bytes(out, &mut offset, &[0u8; MEMBER_RESERVED_LEN])?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultConfig {
    pub version: u8,
    pub bump: u8,
    pub threshold: u8,
    pub member_count: u8,
    pub vault_seed_len: u8,
    pub pending_member_index: u8,
    pub config_version: u64,
    pub next_proposal_id: u64,
    pub timelock_slots: u64,
    pub proposal_ttl_slots: u64,
    pub pending_member_pda: Address,
    pub vault_seed: [u8; VAULT_SEED_MAX_LEN],
    pub members: [MemberRecord; MAX_MEMBERS],
}

impl VaultConfig {
    pub const LEN: usize =
        8 + 8 + 8 + 8 + 8 + 32 + VAULT_SEED_MAX_LEN + (MemberRecord::LEN * MAX_MEMBERS);
    pub const VERSION: u8 = 1;

    pub fn load(account: &AccountView, program_id: &Address) -> Result<Self, ProgramError> {
        if !account.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        if account.data_len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let borrowed = account.try_borrow()?;
        let mut data: &[u8] = &borrowed[..];
        let version = read_u8(&mut data)?;
        let bump = read_u8(&mut data)?;
        let threshold = read_u8(&mut data)?;
        let member_count = read_u8(&mut data)?;
        let vault_seed_len = read_u8(&mut data)?;
        let pending_member_index = read_u8(&mut data)?;
        let _reserved = take_bytes(&mut data, 2)?;
        let config_version = read_u64(&mut data)?;
        let next_proposal_id = read_u64(&mut data)?;
        let timelock_slots = read_u64(&mut data)?;
        let proposal_ttl_slots = read_u64(&mut data)?;
        let pending_member_pda = read_address(&mut data)?;
        let vault_seed = read_array::<VAULT_SEED_MAX_LEN>(&mut data)?;
        let mut members = [MemberRecord::disabled(); MAX_MEMBERS];
        for member in &mut members {
            let chunk = take_bytes(&mut data, MemberRecord::LEN)?;
            *member = MemberRecord::read(chunk)?;
        }
        Ok(Self {
            version,
            bump,
            threshold,
            member_count,
            vault_seed_len,
            pending_member_index,
            config_version,
            next_proposal_id,
            timelock_slots,
            proposal_ttl_slots,
            pending_member_pda,
            vault_seed,
            members,
        })
    }

    pub fn store(&self, account: &mut AccountView) -> Result<(), ProgramError> {
        if account.data_len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut out = account.try_borrow_mut()?;
        let out = &mut out[..Self::LEN];
        let mut offset = 0usize;
        write_u8(out, &mut offset, self.version)?;
        write_u8(out, &mut offset, self.bump)?;
        write_u8(out, &mut offset, self.threshold)?;
        write_u8(out, &mut offset, self.member_count)?;
        write_u8(out, &mut offset, self.vault_seed_len)?;
        write_u8(out, &mut offset, self.pending_member_index)?;
        write_bytes(out, &mut offset, &[0u8; 2])?;
        write_u64(out, &mut offset, self.config_version)?;
        write_u64(out, &mut offset, self.next_proposal_id)?;
        write_u64(out, &mut offset, self.timelock_slots)?;
        write_u64(out, &mut offset, self.proposal_ttl_slots)?;
        write_address(out, &mut offset, &self.pending_member_pda)?;
        write_bytes(out, &mut offset, &self.vault_seed)?;
        for member in &self.members {
            let end = offset
                .checked_add(MemberRecord::LEN)
                .ok_or(ProgramError::InvalidAccountData)?;
            member.write(
                out.get_mut(offset..end)
                    .ok_or(ProgramError::AccountDataTooSmall)?,
            )?;
            offset = end;
        }
        Ok(())
    }

    pub fn member_index(&self, candidate: &Address) -> Option<usize> {
        self.members
            .iter()
            .take(self.member_count as usize)
            .enumerate()
            .find_map(|(index, member)| {
                if member.enabled != 0 && member.member_pda == *candidate {
                    Some(index)
                } else {
                    None
                }
            })
    }

    pub fn vault_seed(&self) -> &[u8] {
        &self.vault_seed[..self.vault_seed_len as usize]
    }

    pub fn pending_member(&self) -> Option<(usize, Address)> {
        (self.pending_member_index != NO_PENDING_MEMBER_INDEX)
            .then_some((self.pending_member_index as usize, self.pending_member_pda))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposalState {
    pub version: u8,
    pub state: u8,
    pub proposer_member_index: u8,
    pub approval_bitmap: u8,
    pub approval_count: u8,
    pub action_kind: u8,
    pub proposal_id: u64,
    pub vault: Address,
    pub config_version: u64,
    pub created_slot: u64,
    pub not_before_slot: u64,
    pub expires_at_slot: u64,
    pub action_hash: [u8; 32],
    pub tx_account: Address,
    pub executed_slot: u64,
}

impl ProposalState {
    pub const LEN: usize = 160;
    pub const VERSION: u8 = 1;

    pub fn load(account: &AccountView, program_id: &Address) -> Result<Self, ProgramError> {
        if !account.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        if account.data_len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let borrowed = account.try_borrow()?;
        let mut data: &[u8] = &borrowed[..];
        let version = read_u8(&mut data)?;
        let state = read_u8(&mut data)?;
        let proposer_member_index = read_u8(&mut data)?;
        let approval_bitmap = read_u8(&mut data)?;
        let approval_count = read_u8(&mut data)?;
        let action_kind = read_u8(&mut data)?;
        let _reserved = take_bytes(&mut data, 2)?;
        let proposal_id = read_u64(&mut data)?;
        let vault = read_address(&mut data)?;
        let config_version = read_u64(&mut data)?;
        let created_slot = read_u64(&mut data)?;
        let not_before_slot = read_u64(&mut data)?;
        let expires_at_slot = read_u64(&mut data)?;
        let action_hash = read_array::<32>(&mut data)?;
        let tx_account = read_address(&mut data)?;
        let executed_slot = read_u64(&mut data)?;
        let _reserved_tail = take_bytes(&mut data, 8)?;
        Ok(Self {
            version,
            state,
            proposer_member_index,
            approval_bitmap,
            approval_count,
            action_kind,
            proposal_id,
            vault,
            config_version,
            created_slot,
            not_before_slot,
            expires_at_slot,
            action_hash,
            tx_account,
            executed_slot,
        })
    }

    pub fn store(&self, account: &mut AccountView) -> Result<(), ProgramError> {
        if account.data_len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut out = account.try_borrow_mut()?;
        let out = &mut out[..Self::LEN];
        let mut offset = 0usize;
        write_u8(out, &mut offset, self.version)?;
        write_u8(out, &mut offset, self.state)?;
        write_u8(out, &mut offset, self.proposer_member_index)?;
        write_u8(out, &mut offset, self.approval_bitmap)?;
        write_u8(out, &mut offset, self.approval_count)?;
        write_u8(out, &mut offset, self.action_kind)?;
        write_bytes(out, &mut offset, &[0u8; 2])?;
        write_u64(out, &mut offset, self.proposal_id)?;
        write_address(out, &mut offset, &self.vault)?;
        write_u64(out, &mut offset, self.config_version)?;
        write_u64(out, &mut offset, self.created_slot)?;
        write_u64(out, &mut offset, self.not_before_slot)?;
        write_u64(out, &mut offset, self.expires_at_slot)?;
        write_bytes(out, &mut offset, &self.action_hash)?;
        write_address(out, &mut offset, &self.tx_account)?;
        write_u64(out, &mut offset, self.executed_slot)?;
        write_bytes(out, &mut offset, &[0u8; 8])?;
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.state == PROPOSAL_STATE_ACTIVE
    }

    pub fn add_member_approval(&mut self, member_index: usize) {
        let bit = 1u8 << member_index;
        if (self.approval_bitmap & bit) == 0 {
            self.approval_bitmap |= bit;
            self.approval_count = self.approval_count.saturating_add(1);
        }
    }

    pub fn remove_member_approval(&mut self, member_index: usize) -> Result<(), ProgramError> {
        let bit = 1u8 << member_index;
        if (self.approval_bitmap & bit) == 0 {
            return Err(ProgramError::InvalidAccountData);
        }
        self.approval_bitmap &= !bit;
        self.approval_count = self.approval_count.saturating_sub(1);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposalTxHeader {
    pub version: u8,
    pub action_hash: [u8; 32],
    pub data_len: u32,
}

impl ProposalTxHeader {
    pub const LEN: usize = 40;
    pub const VERSION: u8 = 1;

    pub fn account_len(data_len: usize) -> usize {
        Self::LEN + data_len
    }

    pub fn load(account: &AccountView, program_id: &Address) -> Result<Self, ProgramError> {
        if !account.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        if account.data_len() < Self::LEN {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let borrowed = account.try_borrow()?;
        let mut data: &[u8] = &borrowed[..];
        let version = read_u8(&mut data)?;
        let _reserved = take_bytes(&mut data, 3)?;
        let action_hash = read_array::<32>(&mut data)?;
        let data_len = read_u32(&mut data)?;
        Ok(Self {
            version,
            action_hash,
            data_len,
        })
    }

    pub fn proposal_bytes<'a>(&self, account: &'a AccountView) -> Result<&'a [u8], ProgramError> {
        let data = account.try_borrow()?;
        let end = Self::LEN
            .checked_add(self.data_len as usize)
            .ok_or(ProgramError::InvalidAccountData)?;
        let bytes = data
            .get(Self::LEN..end)
            .ok_or(ProgramError::AccountDataTooSmall)?;
        let ptr = bytes.as_ptr();
        let len = bytes.len();
        core::mem::forget(data);
        Ok(unsafe { core::slice::from_raw_parts(ptr, len) })
    }

    pub fn store_with_bytes(
        &self,
        account: &mut AccountView,
        payload: &[u8],
    ) -> Result<(), ProgramError> {
        let expected_len = Self::account_len(payload.len());
        if account.data_len() < expected_len {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut out = account.try_borrow_mut()?;
        let out = &mut out[..expected_len];
        let mut offset = 0usize;
        write_u8(out, &mut offset, self.version)?;
        write_bytes(out, &mut offset, &[0u8; 3])?;
        write_bytes(out, &mut offset, &self.action_hash)?;
        write_u32(out, &mut offset, self.data_len)?;
        write_bytes(out, &mut offset, payload)?;
        Ok(())
    }
}
