use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use paperudp::channel::Channel;
use tracing::warn;
use crate::limits::{BandwidthLimiter, TrafficLimits, MAX_TRAFFIC_VIOLATIONS};

const MAX_RESEND_ROUNDS: u32 = 20;

const MAX_SESSIONS: usize = 8192;

pub struct ClientSession {
    pub id: u64,
    pub addr: SocketAddr,
    pub channel: Channel,
    pub last_heard_from: Instant,

    pub(crate) traffic: BandwidthLimiter,
    pub(crate) bytes_sent_total: u64,
    /// Reliable sends refused by the rate limit.
    pub(crate) traffic_violations: u32,
    created: Instant,

    /// Consecutive resends without the channel clearing.
    resend_rounds: u32,
    last_resend_at: Option<Instant>,
}

impl ClientSession {
    fn resend_budget_spent(&self) -> bool {
        self.resend_rounds > MAX_RESEND_ROUNDS
    }

    fn traffic_budget_spent(&self, limits: &TrafficLimits, now: Instant) -> bool {
        self.bytes_sent_total > limits.max_session_bytes
            || now.duration_since(self.created) > limits.max_session_duration
            || self.traffic_violations > MAX_TRAFFIC_VIOLATIONS
    }
}

pub struct ConnectionManager {
    id_to_session: HashMap<u64, ClientSession>,
    addr_to_id: HashMap<SocketAddr, u64>,
    next_client_id: u64,
    limits: TrafficLimits,
}

impl ConnectionManager {
    pub fn new(limits: TrafficLimits) -> Self {
        Self {
            id_to_session: HashMap::new(),
            addr_to_id: HashMap::new(),
            next_client_id: 1,
            limits,
        }
    }

    /// Returns the session and whether it was just created. `None` at the session cap.
    pub fn get_or_create(&mut self, addr: SocketAddr) -> Option<(&mut ClientSession, bool)> {
        if let Some(id) = self.addr_to_id.get(&addr) {
            // TODO: get rid of expect
            let s = self.id_to_session.get_mut(id).expect("session exists in both maps");
            return Some((s, false));
        }

        if self.id_to_session.len() >= MAX_SESSIONS {
            warn!("refusing a session for {}: at the {} session cap", addr, MAX_SESSIONS);
            return None;
        }

        Some((self.create_session(addr), true))
    }

    pub fn create_session(&mut self, addr: SocketAddr) -> &mut ClientSession {
        let id = self.next_client_id;
        self.next_client_id += 1;

        let now = Instant::now();
        let session = ClientSession {
            id,
            addr,
            channel: Channel::new(),
            last_heard_from: now,
            traffic: BandwidthLimiter::per_second(
                self.limits.client_bytes_per_sec,
                self.limits.interval,
                now,
            ),
            bytes_sent_total: 0,
            traffic_violations: 0,
            created: now,
            resend_rounds: 0,
            last_resend_at: None,
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
        let now = Instant::now();

        for session in self.id_to_session.values_mut() {
            // Otherwise a spoofed address turns the relay into a flood.
            if session.resend_budget_spent() {
                continue;
            }

            let packets = session.channel.collect_resends(interval);

            if packets.is_empty() {
                // Runs more often than `interval`; an empty round may not mean clear.
                if session.last_resend_at.is_none_or(|t| now.duration_since(t) > interval) {
                    session.resend_rounds = 0;
                }
                continue;
            }

            session.last_resend_at = Some(now);
            session.resend_rounds += 1;

            for pkt in packets {
                out.push((session.addr, pkt));
            }
        }

        out
    }

    /// Drops idle and over-budget sessions. The server loop turns the returned
    /// IDs into `ClientDisconnected` events.
    pub fn cleanup_sessions(&mut self, timeout: Duration) -> Vec<u64> {
        let now = Instant::now();
        let mut expired = Vec::new();

        for (&id, session) in &mut self.id_to_session {
            if session.traffic_budget_spent(&self.limits, now) {
                warn!(
                    "dropping session {} ({}): {} bytes sent over {}s, {} reliable violations",
                    id,
                    session.addr,
                    session.bytes_sent_total,
                    now.duration_since(session.created).as_secs(),
                    session.traffic_violations
                );
                expired.push(id);
                continue;
            }

            if now.duration_since(session.last_heard_from) > timeout {
                expired.push(id);
                continue;
            }

            session.traffic_violations = session.traffic_violations.saturating_sub(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> ConnectionManager {
        ConnectionManager::new(TrafficLimits {
            client_bytes_per_sec: 65536,
            global_bytes_per_sec: 2_000_000,
            daily_bytes: 5_000_000_000,
            daily_control_bytes: 250_000_000,
            interval: Duration::from_millis(250),
            max_session_bytes: 2_000_000_000,
            max_session_duration: Duration::from_secs(4 * 60 * 60),
        })
    }

    const TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn a_spent_resend_budget_does_not_disconnect_a_live_session() {
        let mut cm = manager();
        let id = cm.create_session("1.2.3.4:5000".parse().unwrap()).id;

        let session = cm.get_by_id(&id).unwrap();
        session.resend_rounds = MAX_RESEND_ROUNDS + 1;
        session.last_heard_from = Instant::now();

        // Resends stop, but the session survives until the idle timeout.
        assert!(cm.get_resends(Duration::from_millis(100)).is_empty());
        assert!(cm.cleanup_sessions(TIMEOUT).is_empty());
        assert!(cm.get_by_id(&id).is_some());
    }

    #[test]
    fn violations_decay_one_per_cleanup_pass() {
        let mut cm = manager();
        let id = cm.create_session("1.2.3.4:5000".parse().unwrap()).id;

        cm.get_by_id(&id).unwrap().traffic_violations = 5;

        cm.cleanup_sessions(TIMEOUT);
        assert_eq!(cm.get_by_id(&id).unwrap().traffic_violations, 4);

        cm.cleanup_sessions(TIMEOUT);
        assert_eq!(cm.get_by_id(&id).unwrap().traffic_violations, 3);
    }

    #[test]
    fn a_sustained_overrun_still_expires_the_session() {
        let mut cm = manager();
        let id = cm.create_session("1.2.3.4:5000".parse().unwrap()).id;

        cm.get_by_id(&id).unwrap().traffic_violations = MAX_TRAFFIC_VIOLATIONS + 1;

        assert_eq!(cm.cleanup_sessions(TIMEOUT), vec![id]);
        assert!(cm.get_by_id(&id).is_none());
    }
}
