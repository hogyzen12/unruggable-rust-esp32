//! Tropic CI_TR01_ERR_2025091800: reboot can truncate the final CRC bits.
//! Apply the vendor's exception only to the acknowledgement of Startup_Req.
pub fn response_crc_valid(
    startup_request: bool,
    status: u8,
    length: usize,
    received: u16,
    calculated: u16,
) -> bool {
    received == calculated
        || (startup_request && status == 0x01 && length == 0 && received as u8 == 0x03)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorded_truncated_reboot_crc_is_accepted_only_for_reboot() {
        assert!(response_crc_valid(true, 1, 0, 0xbf03, 0x8603));
        assert!(!response_crc_valid(false, 1, 0, 0xbf03, 0x8603));
        for status in [0, 2, 3, 4, 0x7c, 0xff] {
            assert!(!response_crc_valid(true, status, 0, 0xbf03, 0x8603));
        }
        assert!(!response_crc_valid(true, 1, 1, 0xbf03, 0x8603));
        assert!(!response_crc_valid(true, 1, 0, 0xbf02, 0x8603));
    }
    #[test]
    fn normal_frames_still_require_the_complete_crc() {
        assert!(response_crc_valid(false, 2, 80, 0x1234, 0x1234));
        for bit in 0..16 {
            assert!(!response_crc_valid(
                false,
                2,
                80,
                0x1234 ^ (1 << bit),
                0x1234
            ));
        }
    }
}
