//! SE050 AN12413 sections 3.7 and 4.7.1.5. Policies are fixed at creation.
const READ: u32 = 0x0020_0000;
const WRITE: u32 = 0x0010_0000;
const DELETE: u32 = 0x0004_0000;

fn policy(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (auth, permissions) in entries {
        bytes.push(8);
        bytes.extend_from_slice(&auth.to_be_bytes());
        bytes.extend_from_slice(&permissions.to_be_bytes());
    }
    bytes
}

pub fn pin_object_policy(pin_object: u32) -> Vec<u8> {
    policy(&[(pin_object, DELETE)])
}

pub fn protected_binary_policy(pin_object: u32) -> Vec<u8> {
    // Do not allow anonymous PIN-object deletion/recreation: reusing its ID
    // could otherwise grant access to a surviving protected pairing secret.
    policy(&[(pin_object, READ | WRITE | DELETE)])
}

pub fn userid_payload(
    id: u32,
    user_id: &[u8],
    attempts: u16,
    policy: &[u8],
) -> Result<Vec<u8>, &'static str> {
    if !(4..=16).contains(&user_id.len()) || policy.len() > 127 {
        return Err("INVALID_USERID_PAYLOAD");
    }
    let mut payload = Vec::new();
    // AN12413 4.1.5: TLVs MUST follow the specified command order.
    if !policy.is_empty() {
        payload.extend_from_slice(&[0x11, policy.len() as u8]);
        payload.extend_from_slice(policy);
    }
    if attempts != 0 {
        payload.extend_from_slice(&[0x12, 2]);
        payload.extend_from_slice(&attempts.to_be_bytes());
    }
    payload.extend_from_slice(&[0x41, 4]);
    payload.extend_from_slice(&id.to_be_bytes());
    payload.extend_from_slice(&[0x42, user_id.len() as u8]);
    payload.extend_from_slice(user_id);
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userid_creation_matches_apdu_spec_field_order() {
        assert_eq!(
            userid_payload(0x01020304, b"123456", 10, &pin_object_policy(0x01020304)).unwrap(),
            [
                0x11, 9, 8, 1, 2, 3, 4, 0, 4, 0, 0, 0x12, 2, 0, 10, 0x41, 4, 1, 2, 3, 4, 0x42, 6,
                b'1', b'2', b'3', b'4', b'5', b'6'
            ]
        );
    }

    #[test]
    fn reset_policy_does_not_expose_or_allow_replacing_pairing_secret() {
        let entries = protected_binary_policy(0x12345678);
        assert_eq!(&entries[..9], &[8, 0x12, 0x34, 0x56, 0x78, 0, 0x34, 0, 0]);
        assert_eq!(entries.len(), 9);
    }
}
