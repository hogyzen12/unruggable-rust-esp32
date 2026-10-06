use pinocchio::error::ProgramError;
use solana_address::Address;

#[inline(always)]
pub fn take_bytes<'a>(payload: &mut &'a [u8], len: usize) -> Result<&'a [u8], ProgramError> {
    if payload.len() < len {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (chunk, rest) = payload.split_at(len);
    *payload = rest;
    Ok(chunk)
}

#[inline(always)]
pub fn read_u8(payload: &mut &[u8]) -> Result<u8, ProgramError> {
    let bytes = take_bytes(payload, 1)?;
    Ok(bytes[0])
}

#[inline(always)]
pub fn read_u16(payload: &mut &[u8]) -> Result<u16, ProgramError> {
    let mut out = [0u8; 2];
    out.copy_from_slice(take_bytes(payload, 2)?);
    Ok(u16::from_le_bytes(out))
}

#[inline(always)]
pub fn read_u32(payload: &mut &[u8]) -> Result<u32, ProgramError> {
    let mut out = [0u8; 4];
    out.copy_from_slice(take_bytes(payload, 4)?);
    Ok(u32::from_le_bytes(out))
}

#[inline(always)]
pub fn read_u64(payload: &mut &[u8]) -> Result<u64, ProgramError> {
    let mut out = [0u8; 8];
    out.copy_from_slice(take_bytes(payload, 8)?);
    Ok(u64::from_le_bytes(out))
}

#[inline(always)]
pub fn read_address(payload: &mut &[u8]) -> Result<Address, ProgramError> {
    let mut out = [0u8; 32];
    out.copy_from_slice(take_bytes(payload, 32)?);
    Ok(Address::new_from_array(out))
}

#[inline(always)]
pub fn read_array<const N: usize>(payload: &mut &[u8]) -> Result<[u8; N], ProgramError> {
    let mut out = [0u8; N];
    out.copy_from_slice(take_bytes(payload, N)?);
    Ok(out)
}

#[inline(always)]
pub fn write_u64(dst: &mut [u8], offset: &mut usize, value: u64) -> Result<(), ProgramError> {
    write_bytes(dst, offset, &value.to_le_bytes())
}

#[inline(always)]
pub fn write_u32(dst: &mut [u8], offset: &mut usize, value: u32) -> Result<(), ProgramError> {
    write_bytes(dst, offset, &value.to_le_bytes())
}

#[inline(always)]
pub fn write_u8(dst: &mut [u8], offset: &mut usize, value: u8) -> Result<(), ProgramError> {
    write_bytes(dst, offset, &[value])
}

#[inline(always)]
pub fn write_address(dst: &mut [u8], offset: &mut usize, address: &Address) -> Result<(), ProgramError> {
    write_bytes(dst, offset, address.as_ref())
}

#[inline(always)]
pub fn write_bytes(dst: &mut [u8], offset: &mut usize, src: &[u8]) -> Result<(), ProgramError> {
    let end = offset
        .checked_add(src.len())
        .ok_or(ProgramError::InvalidAccountData)?;
    let slot = dst
        .get_mut(*offset..end)
        .ok_or(ProgramError::AccountDataTooSmall)?;
    slot.copy_from_slice(src);
    *offset = end;
    Ok(())
}
