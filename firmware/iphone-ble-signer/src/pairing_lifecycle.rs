//! A protected credential is durable before any one-time pairing write.
use anyhow::{anyhow, ensure, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingState {
    Empty,
    Matching,
    Different,
    Invalid,
}

pub trait PairingStore {
    type Credential;
    fn load(&mut self) -> Result<Option<Self::Credential>>;
    fn prepare(&mut self) -> Result<Self::Credential>;
    /// Must verify both PIN-authorized readability and anonymous read denial.
    fn persist(&mut self, credential: &Self::Credential) -> Result<()>;
    fn slot_state(&mut self, credential: &Self::Credential) -> Result<PairingState>;
    fn program(&mut self, credential: &Self::Credential) -> Result<()>;
}

pub fn resume_pairing<S: PairingStore>(store: &mut S) -> Result<S::Credential> {
    let credential = match store.load()? {
        Some(existing) => existing,
        None => {
            let prepared = store.prepare()?;
            store.persist(&prepared)?;
            // Re-read the durable record rather than rely on a RAM-only copy.
            store
                .load()?
                .ok_or_else(|| anyhow!("PAIRING_JOURNAL_MISSING"))?
        }
    };
    match store.slot_state(&credential)? {
        PairingState::Matching => {}
        PairingState::Empty => {
            store.program(&credential)?;
            ensure!(
                store.slot_state(&credential)? == PairingState::Matching,
                "PAIRING_WRITE_NOT_VERIFIED"
            );
        }
        PairingState::Different => return Err(anyhow!("PAIRING_CREDENTIAL_MISMATCH")),
        PairingState::Invalid => return Err(anyhow!("PAIRING_SLOT_INVALIDATED")),
    }
    Ok(credential)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Store {
        durable: Option<u32>,
        slot: Option<u32>,
        invalid: bool,
        generated: u32,
        writes: u32,
        step: usize,
        cut: Option<usize>,
    }
    impl Store {
        fn checkpoint(&mut self) -> Result<()> {
            self.step += 1;
            ensure!(self.cut != Some(self.step), "POWER_LOSS");
            Ok(())
        }
    }
    impl PairingStore for Store {
        type Credential = u32;
        fn load(&mut self) -> Result<Option<u32>> {
            self.checkpoint()?;
            Ok(self.durable)
        }
        fn prepare(&mut self) -> Result<u32> {
            self.checkpoint()?;
            self.generated += 1;
            Ok(self.generated)
        }
        fn persist(&mut self, value: &u32) -> Result<()> {
            self.checkpoint()?;
            self.durable = Some(*value);
            self.checkpoint() // Includes a lost acknowledgement after persistence.
        }
        fn slot_state(&mut self, value: &u32) -> Result<PairingState> {
            self.checkpoint()?;
            Ok(if self.invalid {
                PairingState::Invalid
            } else {
                match self.slot {
                    None => PairingState::Empty,
                    Some(existing) if existing == *value => PairingState::Matching,
                    Some(_) => PairingState::Different,
                }
            })
        }
        fn program(&mut self, value: &u32) -> Result<()> {
            self.checkpoint()?;
            assert_eq!(self.durable, Some(*value));
            assert!(self.slot.is_none());
            self.slot = Some(*value);
            self.writes += 1;
            self.checkpoint() // Includes a successful OTP write with a lost reply.
        }
    }

    #[test]
    fn every_interrupted_transition_resumes_without_orphaning_or_rewriting_slot() {
        for cut in 1..=10 {
            let mut store = Store {
                cut: Some(cut),
                ..Store::default()
            };
            let _ = resume_pairing(&mut store);
            if store.slot.is_some() {
                assert_eq!(store.slot, store.durable);
            }
            store.cut = None;
            let credential = resume_pairing(&mut store).unwrap();
            assert_eq!(store.slot, Some(credential));
            assert_eq!(store.durable, Some(credential));
            assert_eq!(store.writes, 1);
            for _ in 0..5 {
                assert_eq!(resume_pairing(&mut store).unwrap(), credential);
            }
            assert_eq!(store.writes, 1);
        }
    }

    #[test]
    fn mismatched_or_invalid_slot_never_allocates_a_replacement() {
        for invalid in [false, true] {
            let mut store = Store {
                durable: Some(8),
                slot: Some(9),
                invalid,
                ..Store::default()
            };
            assert!(resume_pairing(&mut store).is_err());
            assert_eq!(store.generated, 0);
            assert_eq!(store.writes, 0);
            assert_eq!(store.durable, Some(8));
        }
    }

    #[test]
    fn failed_journal_read_does_not_generate_or_program_anything() {
        let mut store = Store {
            cut: Some(1),
            ..Store::default()
        };
        assert!(resume_pairing(&mut store).is_err());
        assert_eq!(store.generated, 0);
        assert_eq!(store.writes, 0);
    }
}
