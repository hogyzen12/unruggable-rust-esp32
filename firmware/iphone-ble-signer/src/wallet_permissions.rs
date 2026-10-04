//! Least privilege for the wallet's group of eight Tropic ECC slots.
//! I-Config bits are one-way: preserve reserved bits and all other groups.
use anyhow::{ensure, Result};

pub const WALLET_PAIRING_SLOT: u8 = 1;
pub const WALLET_REGISTERS: [u16; 4] = [0x130, 0x134, 0x13c, 0x144];

pub fn forbidden_mask(wallet_slot: u8, pairing_slot: u8) -> Result<u32> {
    ensure!(
        wallet_slot < 32 && pairing_slot == WALLET_PAIRING_SLOT,
        "WALLET_PERMISSION_RANGE"
    );
    Ok((0x0fu32 & !(1 << pairing_slot)) << ((wallet_slot / 8) * 8))
}

pub fn validate_permissions(
    wallet_slot: u8,
    pairing_slot: u8,
    irreversible: u32,
    reversible: u32,
) -> Result<()> {
    let denied = forbidden_mask(wallet_slot, pairing_slot)?;
    let required = 1u32 << (((wallet_slot / 8) * 8) + pairing_slot);
    ensure!(
        irreversible & denied == 0,
        "WALLET_PERMISSION_NOT_PERMANENT"
    );
    ensure!(
        irreversible & reversible & required != 0,
        "WALLET_PAIRING_NOT_AUTHORIZED"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_other_pairing_is_denied_for_each_wallet_group() {
        for wallet in 0..32 {
            let mask = forbidden_mask(wallet, 1).unwrap();
            let permanent = !mask;
            validate_permissions(wallet, 1, permanent, u32::MAX).unwrap();
            for attacker in [0, 2, 3] {
                let bit = 1 << (((wallet / 8) * 8) + attacker);
                assert_eq!(permanent & bit, 0);
                assert!(validate_permissions(wallet, 1, permanent | bit, u32::MAX).is_err());
            }
            assert_eq!(mask.count_ones(), 3);
            // All other fields, including reserved bits, remain unchanged.
            let group = 0xf << ((wallet / 8) * 8);
            assert_eq!(permanent & !group, u32::MAX & !group);
            assert!(validate_permissions(wallet, 1, permanent, 0).is_err());
        }
    }
    #[test]
    fn reversible_restriction_alone_is_never_accepted() {
        assert!(validate_permissions(1, 1, u32::MAX, 2).is_err());
        assert!(forbidden_mask(32, 1).is_err());
        for slot in [0, 2, 3, 255] {
            assert!(forbidden_mask(1, slot).is_err());
        }
    }
}
