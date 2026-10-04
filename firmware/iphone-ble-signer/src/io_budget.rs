//! Whole-operation bounds in addition to each individual bus-read timeout.
use anyhow::{ensure, Result};

pub struct IoBudget {
    deadline_ms: u64,
    steps_left: usize,
    response_bytes_left: usize,
}

impl IoBudget {
    pub fn new(now_ms: u64, timeout_ms: u64, steps: usize, response_bytes: usize) -> Self {
        Self {
            deadline_ms: now_ms.saturating_add(timeout_ms),
            steps_left: steps,
            response_bytes_left: response_bytes,
        }
    }

    pub fn check_time(&self, now_ms: u64) -> Result<()> {
        ensure!(now_ms < self.deadline_ms, "CHIP_OPERATION_TIMEOUT");
        Ok(())
    }

    pub fn step(&mut self, now_ms: u64) -> Result<()> {
        self.check_time(now_ms)?;
        ensure!(self.steps_left > 0, "CHIP_RESPONSE_LIMIT");
        self.steps_left -= 1;
        Ok(())
    }

    pub fn append(&mut self, output: &mut Vec<u8>, bytes: &[u8], now_ms: u64) -> Result<()> {
        self.step(now_ms)?;
        ensure!(
            bytes.len() <= self.response_bytes_left,
            "CHIP_RESPONSE_TOO_LARGE"
        );
        self.response_bytes_left -= bytes.len();
        output.extend_from_slice(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endless_retries_and_empty_fragments_terminate_even_without_clock_progress() {
        let mut budget = IoBudget::new(10, 5000, 3, 512);
        let mut reply = vec![];
        for _ in 0..3 {
            budget.append(&mut reply, &[], 10).unwrap();
        }
        assert!(budget.append(&mut reply, &[], 10).is_err());
        assert!(reply.is_empty());
    }

    #[test]
    fn oversized_fragment_is_rejected_before_allocation() {
        let mut budget = IoBudget::new(0, 100, 10, 4);
        let mut reply = vec![];
        budget.append(&mut reply, &[1, 2], 1).unwrap();
        assert!(budget.append(&mut reply, &[3, 4, 5], 2).is_err());
        assert_eq!(reply, [1, 2]);
    }

    #[test]
    fn a_fast_peer_cannot_reset_the_overall_deadline() {
        let mut budget = IoBudget::new(100, 50, 1000, 512);
        budget.step(149).unwrap();
        assert!(budget.step(150).is_err());
    }
}
