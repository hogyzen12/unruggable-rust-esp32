#![no_std]

extern crate alloc;

use pinocchio::{entrypoint, nostd_panic_handler, AccountView, Address, ProgramResult};
use solana_address::declare_id;

mod helpers;
mod instruction;
mod processor;
mod state;

use instruction::{
    ensure_empty, ApprovalArgs, CreateProposalFromMemberArgs, InitializeVaultArgs, VaultInstruction,
};

entrypoint!(process_instruction);
nostd_panic_handler!();

declare_id!("9v16YaZYWXrMdVjGnhFQTE9yHerSchNJg18bqSCZUF52");

fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let (discriminator, rest) = instruction_data
        .split_first()
        .ok_or(pinocchio::error::ProgramError::InvalidInstructionData)?;

    match VaultInstruction::try_from(discriminator)? {
        VaultInstruction::InitializeVault => {
            let args = InitializeVaultArgs::parse(rest)?;
            processor::initialize_vault(program_id, accounts, args)
        }
        VaultInstruction::CreateProposalFromMember => {
            let args = CreateProposalFromMemberArgs::parse(rest)?;
            processor::create_proposal_from_member(program_id, accounts, args)
        }
        VaultInstruction::ApproveProposalFromMember => {
            let args = ApprovalArgs::parse(rest)?;
            processor::approve_proposal_from_member(program_id, accounts, args)
        }
        VaultInstruction::ExecuteProposal => {
            let args = ApprovalArgs::parse(rest)?;
            processor::execute_proposal(program_id, accounts, args)
        }
        VaultInstruction::CancelProposal => {
            let args = ApprovalArgs::parse(rest)?;
            processor::cancel_proposal(program_id, accounts, args)
        }
        VaultInstruction::CloseProposal => {
            ensure_empty(rest)?;
            processor::close_proposal(program_id, accounts)
        }
        VaultInstruction::AcceptPendingMember => {
            ensure_empty(rest)?;
            processor::accept_pending_member(program_id, accounts)
        }
        VaultInstruction::RevokeProposalFromMember => {
            let args = ApprovalArgs::parse(rest)?;
            processor::revoke_proposal_from_member(program_id, accounts, args)
        }
    }
}
