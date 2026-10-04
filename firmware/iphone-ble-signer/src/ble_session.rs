//! Connection-scoped authorization and bounded request ownership.
use std::collections::VecDeque;
use zeroize::Zeroizing;

pub struct BleCommand {
    pub generation: u64,
    pub line: Zeroizing<String>,
}

#[derive(Default)]
pub struct BleSession {
    generation: u64,
    connected: bool,
    authenticated: bool,
    subscribed: bool,
    processing: bool,
    commands: VecDeque<BleCommand>,
}

impl BleSession {
    pub fn connect(&mut self) -> u64 {
        self.disconnect();
        self.generation = self
            .generation
            .checked_add(1)
            .expect("BLE generation exhausted");
        self.connected = true;
        self.generation
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.authenticated = false;
        self.subscribed = false;
        self.processing = false;
        self.commands.clear();
    }
    pub fn authenticate(&mut self, generation: u64) {
        if self.connected && self.generation == generation {
            self.authenticated = true;
        }
    }
    pub fn subscribe(&mut self, enabled: bool) {
        self.subscribed = enabled;
        if !enabled {
            self.commands.clear();
        }
    }
    pub fn is_active(&self, generation: u64) -> bool {
        self.connected && self.authenticated && self.subscribed && self.generation == generation
    }
    pub fn enqueue(&mut self, generation: u64, line: String) -> bool {
        let line = Zeroizing::new(line);
        // The wire protocol has no request ids: prohibit command pipelining.
        if !self.is_active(generation) || self.processing || !self.commands.is_empty() {
            return false;
        }
        self.commands.push_back(BleCommand { generation, line });
        true
    }
    pub fn pop(&mut self) -> Option<BleCommand> {
        let command = self.commands.pop_front()?;
        if !self.is_active(command.generation) {
            return None;
        }
        self.processing = true;
        Some(command)
    }
    pub fn finish(&mut self, generation: u64) {
        if self.generation == generation {
            self.processing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready(session: &mut BleSession) -> u64 {
        let generation = session.connect();
        session.authenticate(generation);
        session.subscribe(true);
        generation
    }
    #[test]
    fn commands_need_authentication_and_subscription() {
        let mut session = BleSession::default();
        let generation = session.connect();
        assert!(!session.enqueue(generation, "UNLOCK:PIN:123456".into()));
        session.authenticate(generation);
        assert!(!session.enqueue(generation, "PING".into()));
        session.subscribe(true);
        assert!(session.enqueue(generation, "PING".into()));
    }
    #[test]
    fn disconnect_drops_pending_commands_and_rejects_old_replies() {
        let mut session = BleSession::default();
        let old = ready(&mut session);
        assert!(session.enqueue(old, "SIGN:data".into()));
        session.disconnect();
        let new = ready(&mut session);
        assert_ne!(old, new);
        assert!(session.pop().is_none());
        assert!(!session.is_active(old));
        assert!(session.is_active(new));
        assert!(!session.enqueue(old, "WIPE_KEYS".into()));
    }
    #[test]
    fn in_flight_request_cannot_be_replaced_or_pipelined() {
        let mut session = BleSession::default();
        let generation = ready(&mut session);
        assert!(session.enqueue(generation, "SIGN:first".into()));
        assert!(!session.enqueue(generation, "SIGN:second".into()));
        assert_eq!(&*session.pop().unwrap().line, "SIGN:first");
        assert!(!session.enqueue(generation, "WIPE_KEYS".into()));
        session.finish(generation);
        assert!(session.enqueue(generation, "PING".into()));
    }
    #[test]
    fn stale_auth_and_completion_do_not_authorize_a_new_connection() {
        let mut session = BleSession::default();
        let old = ready(&mut session);
        let new = session.connect();
        session.authenticate(old);
        session.subscribe(true);
        session.finish(old);
        assert!(!session.is_active(new));
    }
    #[test]
    fn unsubscribe_revokes_in_flight_response_authority() {
        let mut session = BleSession::default();
        let generation = ready(&mut session);
        session.subscribe(false);
        assert!(!session.is_active(generation));
    }
}
