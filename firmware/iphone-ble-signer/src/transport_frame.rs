use anyhow::{anyhow, Result};
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDecodeError {
    CommandTooLong,
    InvalidUtf8,
}

pub struct LineAccumulator {
    buffer: Vec<u8>,
    max_len: usize,
    discarding: bool,
}

impl LineAccumulator {
    pub fn new(max_len: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(max_len + 64),
            max_len,
            discarding: false,
        }
    }

    /// Drop a partial/overlong command at a transport session boundary.
    pub fn reset(&mut self) {
        self.buffer.zeroize();
        self.discarding = false;
    }

    pub fn push_byte(&mut self, byte: u8) -> std::result::Result<Option<String>, LineDecodeError> {
        if self.discarding {
            if byte == b'\n' {
                self.discarding = false;
            }
            return Ok(None);
        }
        if byte == b'\r' {
            return Ok(None);
        }

        if byte != b'\n' {
            if self.buffer.len() >= self.max_len {
                self.buffer.zeroize();
                self.discarding = true;
                return Err(LineDecodeError::CommandTooLong);
            }

            self.buffer.push(byte);
            return Ok(None);
        }

        if self.buffer.is_empty() {
            return Ok(None);
        }

        let line = match std::str::from_utf8(&self.buffer) {
            Ok(value) => value.trim().to_string(),
            Err(_) => {
                self.buffer.zeroize();
                return Err(LineDecodeError::InvalidUtf8);
            }
        };
        self.buffer.zeroize();

        Ok(Some(line))
    }
}

pub fn encode_line(value: &str) -> Vec<u8> {
    let mut line = value.as_bytes().to_vec();
    line.push(b'\n');
    line
}

pub fn write_all_line<F>(value: &str, mut write_chunk: F) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<usize>,
{
    let bytes = encode_line(value);
    let mut written = 0usize;
    let mut idle_tries = 0u32;

    while written < bytes.len() {
        let count = write_chunk(&bytes[written..])?;
        if count == 0 {
            idle_tries = idle_tries.saturating_add(1);
            if idle_tries > 200 {
                break;
            }
            continue;
        }

        idle_tries = 0;
        written += count;
    }

    if written != bytes.len() {
        return Err(anyhow!("transport short write: {written}/{}", bytes.len()));
    }

    Ok(())
}

/// USB console diagnostics can end mid-line when a host disappears. Start each
/// protocol reply on a fresh line so its first response survives reconnection.
pub fn write_usb_response<F>(value: &str, mut write_chunk: F) -> Result<()>
where
    F: FnMut(&[u8]) -> Result<usize>,
{
    write_all_line("", &mut write_chunk)?;
    write_all_line(value, write_chunk)
}

impl Drop for LineAccumulator {
    fn drop(&mut self) {
        self.buffer.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_usb_reply_is_separate_from_a_truncated_diagnostic() {
        let mut wire = b"Host command failed: HOST_SESSION_CLO".to_vec();
        write_usb_response("PONG", |bytes| {
            let count = bytes.len().min(2);
            wire.extend_from_slice(&bytes[..count]);
            Ok(count)
        })
        .unwrap();
        let mut parser = LineAccumulator::new(100);
        let lines: Vec<_> = wire
            .into_iter()
            .filter_map(|b| parser.push_byte(b).unwrap())
            .collect();
        assert_eq!(lines, ["Host command failed: HOST_SESSION_CLO", "PONG"]);
        assert!(write_usb_response("PONG", |_| Ok(0)).is_err());
    }
    #[test]
    fn session_boundary_discards_partial_pin_and_overflow_state() {
        let mut parser = LineAccumulator::new(32);
        for byte in b"UNLOCK:PIN:" {
            parser.push_byte(*byte).unwrap();
        }
        parser.reset();
        assert!(parser.push_byte(b'\n').unwrap().is_none());
        for byte in b"PING" {
            parser.push_byte(*byte).unwrap();
        }
        assert_eq!(parser.push_byte(b'\n').unwrap(), Some("PING".into()));
        for _ in 0..32 {
            parser.push_byte(b'x').unwrap();
        }
        assert_eq!(parser.push_byte(b'x'), Err(LineDecodeError::CommandTooLong));
        parser.reset();
        for byte in b"PING" {
            parser.push_byte(*byte).unwrap();
        }
        assert_eq!(parser.push_byte(b'\n').unwrap(), Some("PING".into()));
    }
    #[test]
    fn overflow_discards_the_entire_line_not_a_command_shaped_tail() {
        let mut parser = LineAccumulator::new(4);
        let mut lines = Vec::new();
        let mut errors = 0;
        for byte in b"12345PING\nPING\n" {
            match parser.push_byte(*byte) {
                Ok(Some(line)) => lines.push(line),
                Err(LineDecodeError::CommandTooLong) => errors += 1,
                other => assert!(other.is_ok()),
            }
        }
        assert_eq!(errors, 1);
        assert_eq!(lines, ["PING"]);
    }
    #[test]
    fn invalid_utf8_does_not_poison_the_following_line() {
        let mut parser = LineAccumulator::new(8);
        parser.push_byte(0xff).unwrap();
        assert_eq!(parser.push_byte(b'\n'), Err(LineDecodeError::InvalidUtf8));
        parser.push_byte(b'A').unwrap();
        assert_eq!(parser.push_byte(b'\n').unwrap(), Some("A".into()));
    }
    #[test]
    fn fragmented_commands_and_crlf_decode_at_exact_limit() {
        let mut parser = LineAccumulator::new(4);
        for byte in b"PING\r" {
            assert!(parser.push_byte(*byte).unwrap().is_none());
        }
        assert_eq!(parser.push_byte(b'\n').unwrap(), Some("PING".into()));
    }
}
