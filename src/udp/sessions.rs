use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use paperudp::channel::Channel;

pub struct ClientSession {
    pub id: u64,
    pub addr: SocketAddr,
    pub channel: Channel,
    pub last_heard_from: Instant,
}

pub struct ConnectionManager {
    id_to_session: HashMap<u64, ClientSession>,
    addr_to_id: HashMap<SocketAddr, u64>,
    next_client_id: u64,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            id_to_session: HashMap::new(),
            addr_to_id: HashMap::new(),
            next_client_id: 1
        }
    }

    /// Returns a ClientSession and a bool.
    /// If the session already existed, the bool will be false.
    /// If it had to be created, it will return true.
    pub fn get_or_create(&mut self, addr: SocketAddr) -> (&mut ClientSession, bool) {
        if let Some(id) = self.addr_to_id.get(&addr) {
            // TODO: get rid of expect
            let s = self.id_to_session.get_mut(id).expect("session exists in both maps");
            return (s, false);
        }

        (self.create_session(addr), true)
    }

    pub fn create_session(&mut self, addr: SocketAddr) -> &mut ClientSession {
        let id = self.next_client_id;
        self.next_client_id += 1;

        let session = ClientSession {
            id,
            addr,
            channel: Channel::new(),
            last_heard_from: Instant::now(),
        };

        self.id_to_session.insert(id, session);
        self.addr_to_id.insert(addr, id);

        self.id_to_session.get_mut(&id).expect("session exists")
    }

    pub fn get_by_id(&mut self, id: &u64) -> Option<&mut ClientSession> {
        self.id_to_session.get_mut(id)
    }

    pub fn get_resends(
        &mut self,
        interval: Duration,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();

        for session in self.id_to_session.values_mut() {
            let packets = session.channel.collect_resends(interval);

            for pkt in packets {
                out.push((session.addr, pkt));
            }
        }

        out
    }

    pub fn cleanup_sessions(&mut self, timeout: Duration) -> Vec<u64> {
        let now = Instant::now();
        let mut expired = Vec::new();

        for (&id, session) in &self.id_to_session {
            if now.duration_since(session.last_heard_from) > timeout {
                expired.push(id);
            }
        }

        for id in &expired {
            if let Some(session) = self.id_to_session.remove(id) {
                self.addr_to_id.remove(&session.addr);
            }
        }

        expired
    }

    pub fn remove_session(&mut self, id: &u64) {
        if let Some(session) = self.id_to_session.remove(id) {
            self.addr_to_id.remove(&session.addr);
        }
    }
}
