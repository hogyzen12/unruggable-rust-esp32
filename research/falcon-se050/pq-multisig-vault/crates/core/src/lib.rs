use sha2::{Digest as Sha2Digest, Sha256};
use solana_address::{address, Address};
use solana_instruction::{AccountMeta, Instruction};

pub const PROGRAM_ID: Address = address!("9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52");
pub const SYSTEM_PROGRAM_ID: Address = address!("11111111111111111111111111111111");

pub const VAULT_PDA_SEED: &[u8] = b"pq_vault";
pub const PROPOSAL_PDA_SEED: &[u8] = b"proposal";
pub const PROPOSAL_TX_PDA_SEED: &[u8] = b"proposal_tx";

pub const MAX_MEMBERS: usize = 3;
pub const FLAG_WRITABLE: u8 = 1 << 0;
pub const FLAG_SIGNER_REQUESTED: u8 = 1 << 1;

pub const INITIALIZE_VAULT_DISCRIMINATOR: u8 = 0;
pub const CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 1;
pub const APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 2;
pub const EXECUTE_PROPOSAL_DISCRIMINATOR: u8 = 3;
pub const CANCEL_PROPOSAL_DISCRIMINATOR: u8 = 4;
pub const CLOSE_PROPOSAL_DISCRIMINATOR: u8 = 5;
pub const ACCEPT_PENDING_MEMBER_DISCRIMINATOR: u8 = 6;
pub const REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR: u8 = 7;

// Reserved for a future audited extension. Generic CPI bundle proposals are
// intentionally disabled in FalconOnly v1.
pub const ACTION_KIND_CPI_BUNDLE: u8 = 0;
pub const ACTION_KIND_REPLACE_MEMBER: u8 = 1;
pub const ACTION_KIND_CHANGE_THRESHOLD: u8 = 2;
pub const ACTION_KIND_SET_TIMELOCK: u8 = 3;
pub const ACTION_KIND_SOL_WITHDRAW: u8 = 4;
pub const ACTION_KIND_SPL_TRANSFER: u8 = 5;
pub const ACTION_KIND_UPDATE_CONFIG: u8 = 6;

pub const ACTION_HASH_DOMAIN: &[u8] = b"UNRUGGABLE_PQ_MULTISIG_PROPOSAL_V1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleAccountMeta {
    pub pubkey: Address,
    pub is_writable: bool,
    pub is_signer_requested: bool,
}

impl BundleAccountMeta {
    pub fn flag_byte(&self) -> u8 {
        let mut flags = 0u8;
        if self.is_writable {
            flags |= FLAG_WRITABLE;
        }
        if self.is_signer_requested {
            flags |= FLAG_SIGNER_REQUESTED;
        }
        flags
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleInstruction {
    pub program_id: Address,
    pub accounts: Vec<BundleAccountMeta>,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigMember {
    pub member_pda: Address,
    pub enabled: bool,
    pub weight: u8,
}

pub fn find_vault_pda(vault_seed: &[u8], program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[VAULT_PDA_SEED, vault_seed], program_id)
}

pub fn find_proposal_pda(vault: &Address, proposal_id: u64, program_id: &Address) -> (Address, u8) {
    let proposal_id_le = proposal_id.to_le_bytes();
    Address::find_program_address(&[PROPOSAL_PDA_SEED, vault.as_ref(), &proposal_id_le], program_id)
}

pub fn find_proposal_tx_pda(proposal: &Address, program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[PROPOSAL_TX_PDA_SEED, proposal.as_ref()], program_id)
}

pub fn serialize_bundle(instructions: &[BundleInstruction]) -> Vec<u8> {
    assert!(
        instructions.len() <= u16::MAX as usize,
        "too many bundle instructions"
    );

    let mut out = Vec::new();
    out.extend_from_slice(&(instructions.len() as u16).to_le_bytes());
    for ix in instructions {
        assert!(ix.accounts.len() <= u16::MAX as usize, "too many ix accounts");
        assert!(ix.data.len() <= u16::MAX as usize, "ix data too long");
        out.extend_from_slice(ix.program_id.as_ref());
        out.extend_from_slice(&(ix.accounts.len() as u16).to_le_bytes());
        for account in &ix.accounts {
            out.extend_from_slice(account.pubkey.as_ref());
            out.push(account.flag_byte());
        }
        out.extend_from_slice(&(ix.data.len() as u16).to_le_bytes());
        out.extend_from_slice(&ix.data);
    }
    out
}

pub fn compute_action_hash(
    vault: &Address,
    config_version: u64,
    proposal_id: u64,
    action_kind: u8,
    not_before_slot: u64,
    expires_at_slot: u64,
    bundle_bytes: &[u8],
) -> [u8; 32] {
    let bundle_hash = Sha256::digest(bundle_bytes);
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

pub fn create_initialize_vault_instruction(
    payer: &Address,
    vault_seed: &[u8],
    threshold: u8,
    proposal_ttl_slots: u64,
    timelock_slots: u64,
    members: &[Address],
) -> Instruction {
    assert!(vault_seed.len() <= 32, "vault seed too long");
    assert!(!members.is_empty(), "need at least one member");
    assert!(
        members.len() <= MAX_MEMBERS,
        "too many members (max {})",
        MAX_MEMBERS
    );
    assert!(threshold > 0, "threshold must be positive");
    assert!(
        threshold as usize <= members.len(),
        "threshold cannot exceed member count"
    );

    let (vault, _bump) = find_vault_pda(vault_seed, &PROGRAM_ID);
    let mut data = Vec::with_capacity(1 + 1 + vault_seed.len() + 1 + 8 + 8 + 1 + (32 * members.len()));
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
        program_id: PROGRAM_ID,
        accounts: {
            let mut accounts = vec![
                AccountMeta::new(*payer, true),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
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

pub fn create_create_proposal_from_member_instruction(
    member: &Address,
    payer: &Address,
    vault: &Address,
    proposal_id: u64,
    action_kind: u8,
    not_before_slot: u64,
    expires_at_slot: u64,
    expected_action_hash: [u8; 32],
    bundle_bytes: &[u8],
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let (proposal_tx, _proposal_tx_bump) = find_proposal_tx_pda(&proposal, &PROGRAM_ID);

    assert!(bundle_bytes.len() <= u32::MAX as usize, "bundle too large");

    let mut data = Vec::with_capacity(1 + 8 + 1 + 8 + 8 + 32 + 4 + bundle_bytes.len());
    data.push(CREATE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.push(action_kind);
    data.extend_from_slice(&not_before_slot.to_le_bytes());
    data.extend_from_slice(&expires_at_slot.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);
    data.extend_from_slice(&(bundle_bytes.len() as u32).to_le_bytes());
    data.extend_from_slice(bundle_bytes);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*member, true),
            AccountMeta::new(*payer, true),
            AccountMeta::new(*vault, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new(proposal_tx, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

pub fn create_approve_proposal_from_member_instruction(
    member: &Address,
    vault: &Address,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(APPROVE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*member, true),
            AccountMeta::new(*vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

pub fn create_execute_proposal_instruction(
    vault: &Address,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
    bundle_accounts: &[AccountMeta],
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let (proposal_tx, _proposal_tx_bump) = find_proposal_tx_pda(&proposal, &PROGRAM_ID);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(EXECUTE_PROPOSAL_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);

    let mut accounts = vec![
        AccountMeta::new(*vault, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(proposal_tx, false),
    ];
    accounts.extend_from_slice(bundle_accounts);

    Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data,
    }
}

pub fn create_cancel_proposal_from_member_instruction(
    member: &Address,
    vault: &Address,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(CANCEL_PROPOSAL_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*member, true),
            AccountMeta::new(*vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

pub fn create_revoke_proposal_from_member_instruction(
    member: &Address,
    vault: &Address,
    proposal_id: u64,
    expected_action_hash: [u8; 32],
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let mut data = Vec::with_capacity(1 + 8 + 32);
    data.push(REVOKE_PROPOSAL_FROM_MEMBER_DISCRIMINATOR);
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data.extend_from_slice(&expected_action_hash);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*member, true),
            AccountMeta::new(*vault, false),
            AccountMeta::new(proposal, false),
        ],
        data,
    }
}

pub fn create_close_proposal_instruction(
    vault: &Address,
    proposal_id: u64,
) -> Instruction {
    let (proposal, _proposal_bump) = find_proposal_pda(vault, proposal_id, &PROGRAM_ID);
    let (proposal_tx, _proposal_tx_bump) = find_proposal_tx_pda(&proposal, &PROGRAM_ID);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*vault, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new(proposal_tx, false),
        ],
        data: vec![CLOSE_PROPOSAL_DISCRIMINATOR],
    }
}

pub fn create_accept_pending_member_instruction(member: &Address, vault: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*member, true),
            AccountMeta::new(*vault, false),
        ],
        data: vec![ACCEPT_PENDING_MEMBER_DISCRIMINATOR],
    }
}

pub fn bundle_account_metas(bundle: &[BundleInstruction]) -> Vec<AccountMeta> {
    let mut accounts = Vec::new();

    for ix in bundle {
        merge_account_meta(
            &mut accounts,
            AccountMeta::new_readonly(ix.program_id, false),
        );
        for account in &ix.accounts {
            let meta = if account.is_writable {
                AccountMeta::new(account.pubkey, account.is_signer_requested)
            } else {
                AccountMeta::new_readonly(account.pubkey, account.is_signer_requested)
            };
            merge_account_meta(&mut accounts, meta);
        }
    }

    accounts
}

pub fn create_sol_withdraw_action_payload(receiver: &Address, lamports: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32 + 8);
    payload.extend_from_slice(receiver.as_ref());
    payload.extend_from_slice(&lamports.to_le_bytes());
    payload
}

pub fn create_spl_transfer_action_payload(
    mint: &Address,
    destination: &Address,
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

pub fn create_replace_member_action_payload(member_index: u8, new_member_pda: &Address) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + 32);
    payload.push(member_index);
    payload.extend_from_slice(new_member_pda.as_ref());
    payload
}

pub fn create_update_config_action_payload(
    threshold: u8,
    proposal_ttl_slots: u64,
    timelock_slots: u64,
) -> Vec<u8> {
    assert!(threshold > 0, "threshold must be positive");

    let mut payload = Vec::with_capacity(1 + 8 + 8);
    payload.push(threshold);
    payload.extend_from_slice(&proposal_ttl_slots.to_le_bytes());
    payload.extend_from_slice(&timelock_slots.to_le_bytes());
    payload
}

fn merge_account_meta(accounts: &mut Vec<AccountMeta>, meta: AccountMeta) {
    if let Some(existing) = accounts.iter_mut().find(|existing| existing.pubkey == meta.pubkey) {
        existing.is_writable |= meta.is_writable;
        existing.is_signer |= meta.is_signer;
    } else {
        accounts.push(meta);
    }
}
