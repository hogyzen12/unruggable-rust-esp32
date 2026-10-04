//! Bounded notification transmission. Advance only after the stack accepts bytes.
pub const TX_TIMEOUT_MS: u64 = 2_000;
pub const TX_RETRY_MS: u32 = 10;
pub const MAX_NOTIFICATION_BYTES: usize = 512;
const MAX_ATTEMPTS: usize = 512;

pub fn notification_payload_len(mtu: u16) -> usize {
    // Three ATT header bytes; never exceed the 512-byte attribute-value limit.
    usize::from(mtu.max(23) - 3).min(MAX_NOTIFICATION_BYTES)
}

#[derive(Debug, PartialEq, Eq)]
pub enum TxError<E> {
    Timeout,
    InvalidProgress,
    Link(E),
}

/// `enqueue` returns accepted bytes, or zero when congestion prevented enqueue.
/// It must recheck connection ownership on EVERY call, including retries.
/// `pause` runs outside the connection lock so disconnect callbacks can proceed.
pub fn transmit<E>(
    bytes: &[u8],
    mut now: impl FnMut() -> u64,
    mut enqueue: impl FnMut(&[u8]) -> Result<usize, E>,
    mut pause: impl FnMut(),
) -> Result<(), TxError<E>> {
    let deadline = now().saturating_add(TX_TIMEOUT_MS);
    let mut offset = 0;
    for _ in 0..MAX_ATTEMPTS {
        if offset == bytes.len() {
            return Ok(());
        }
        if now() >= deadline {
            return Err(TxError::Timeout);
        }
        let remaining = &bytes[offset..];
        match enqueue(remaining).map_err(TxError::Link)? {
            0 => pause(),
            accepted if accepted <= remaining.len() => offset += accepted,
            _ => return Err(TxError::InvalidProgress),
        }
    }
    if offset == bytes.len() {
        Ok(())
    } else {
        Err(TxError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn mtu_limits_include_att_header_and_attribute_maximum() {
        for (mtu, length) in [
            (0, 20),
            (23, 20),
            (185, 182),
            (247, 244),
            (517, 512),
            (65535, 512),
        ] {
            assert_eq!(notification_payload_len(mtu), length);
        }
    }

    #[test]
    fn expanded_status_is_complete_at_old_and_negotiated_packet_sizes() {
        let status = format!(
            "INFO;{};SIGN_REVIEW=PAGED;BLIND_SIGN=EXPLICIT\n",
            "x".repeat(900)
        );
        for mtu in [23, 185, 517] {
            let mut received = vec![];
            let mut packets = 0;
            transmit(
                status.as_bytes(),
                || 0,
                |remaining| {
                    let n = remaining.len().min(notification_payload_len(mtu));
                    received.extend_from_slice(&remaining[..n]);
                    packets += 1;
                    Ok::<_, ()>(n)
                },
                || panic!("no congestion"),
            )
            .unwrap();
            assert_eq!(received, status.as_bytes());
            if mtu == 517 {
                assert_eq!(packets, 2);
            }
        }
    }

    #[test]
    fn temporary_congestion_never_drops_or_repeats_accepted_bytes() {
        let bytes = vec![b'a'; 731];
        let mut received = vec![];
        let mut calls = 0;
        let time = Cell::new(0);
        transmit(
            &bytes,
            || time.get(),
            |remaining| {
                calls += 1;
                if calls % 3 != 1 {
                    return Ok::<_, ()>(0);
                }
                let n = remaining.len().min(20);
                received.extend_from_slice(&remaining[..n]);
                Ok(n)
            },
            || time.set(time.get() + u64::from(TX_RETRY_MS)),
        )
        .unwrap();
        assert_eq!(received, bytes);
        assert!(calls > bytes.len().div_ceil(20));
    }

    #[test]
    fn congestion_cannot_extend_the_whole_response_deadline() {
        let time = Cell::new(0);
        let result = transmit(
            b"status\n",
            || time.get(),
            |_| Ok::<_, ()>(0),
            || time.set(time.get() + u64::from(TX_RETRY_MS)),
        );
        assert_eq!(result, Err(TxError::Timeout));
        assert_eq!(time.get(), TX_TIMEOUT_MS);
    }

    #[test]
    fn stalled_clock_still_has_a_finite_attempt_budget() {
        let mut pauses = 0;
        let result = transmit(b"status\n", || 0, |_| Ok::<_, ()>(0), || pauses += 1);
        assert_eq!(result, Err(TxError::Timeout));
        assert_eq!(pauses, MAX_ATTEMPTS);
    }

    #[test]
    fn disconnect_during_congestion_never_sends_into_a_new_session() {
        use crate::ble_session::BleSession;
        let mut session = BleSession::default();
        let old = session.connect();
        session.authenticate(old);
        session.subscribe(true);
        let mut calls = 0;
        let result = transmit(
            b"status\n",
            || 0,
            |_| {
                calls += 1;
                if !session.is_active(old) {
                    return Err("closed");
                }
                let next = session.connect();
                session.authenticate(next);
                session.subscribe(true);
                Ok(0)
            },
            || {},
        );
        assert_eq!(result, Err(TxError::Link("closed")));
        assert_eq!(calls, 2);
    }

    #[test]
    fn permanent_failure_and_invalid_progress_stop_immediately() {
        let mut calls = 0;
        let result = transmit(
            b"status\n",
            || 0,
            |_| {
                calls += 1;
                if calls == 1 {
                    Ok(3)
                } else {
                    Err("failed")
                }
            },
            || panic!("fatal errors are not retried"),
        );
        assert_eq!(result, Err(TxError::Link("failed")));
        assert_eq!(calls, 2);
        assert_eq!(
            transmit(b"x", || 0, |_| Ok::<_, ()>(2), || {}),
            Err(TxError::InvalidProgress)
        );
    }
}
