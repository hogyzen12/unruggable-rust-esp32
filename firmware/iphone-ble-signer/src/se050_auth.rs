//! Distinguish a rejected PIN from a broken link or an unavailable applet.
use anyhow::{ensure, Result};
use std::fmt;

#[derive(Debug)]
pub struct StatusWord(pub u16);

impl fmt::Display for StatusWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PROCESS_SESSION_SW_{:04X}", self.0)
    }
}
impl std::error::Error for StatusWord {}

#[derive(Debug)]
pub struct PinRejected;
impl fmt::Display for PinRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AUTH_FAILED")
    }
}
impl std::error::Error for PinRejected {}

pub fn check_pin_reply(reply: Result<Vec<u8>>) -> Result<()> {
    match reply {
        Ok(data) => {
            ensure!(data.is_empty(), "VERIFY_USERID_RDATA_{}", data.len());
            Ok(())
        }
        // AN12413 4.5.2.1: wrong UserID returns CONDITIONS_NOT_SATISFIED.
        // Interpret this only at VerifySessionUserID, never at session creation.
        Err(error)
            if error
                .downcast_ref::<StatusWord>()
                .is_some_and(|sw| sw.0 == 0x6985) =>
        {
            Err(PinRejected.into())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_wrong_userid_consumes_a_pin_attempt() {
        let error = check_pin_reply(Err(StatusWord(0x6985).into())).unwrap_err();
        assert!(error.is::<PinRejected>());
        for status in [0x6982, 0x6984, 0x6986, 0x6a80, 0x6f00] {
            assert!(!check_pin_reply(Err(StatusWord(status).into()))
                .unwrap_err()
                .is::<PinRejected>());
        }
    }

    #[test]
    fn transport_and_malformed_replies_are_faults_not_wrong_pins() {
        assert!(check_pin_reply(Ok(vec![])).is_ok());
        assert!(!check_pin_reply(Ok(vec![1]))
            .unwrap_err()
            .is::<PinRejected>());
        assert!(!check_pin_reply(Err(anyhow::anyhow!("I2C_TIMEOUT")))
            .unwrap_err()
            .is::<PinRejected>());
    }
}
