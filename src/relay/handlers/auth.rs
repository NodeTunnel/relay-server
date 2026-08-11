use std::error::Error;
use reqwest::StatusCode;
use tracing::warn;
use crate::config::loader::Config;
use crate::protocol::packet::Packet;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::udp::common::{TrafficClass, TransferChannel};
use crate::udp::paper_interface::PaperInterface;

// Counts authenticated clients, so random traffic cannot fill the slots.
const MAX_PLAYERS: usize = 100;

// Error messages quote client input and must fit the client's MAX_STRING_LEN.
// 64 chars of up to 4 bytes each always fits in 256 bytes.
const MAX_ERROR_MESSAGE_CHARS: usize = 64;

fn client_safe_error(msg: &str) -> String {
    msg.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

pub struct AuthHandler<'a> {
    udp: &'a mut PaperInterface,
    http: &'a reqwest::Client,

    clients: &'a mut Clients,
    apps: &'a mut Apps,
    config: &'a Config,
}

impl<'a> AuthHandler<'a> {
    pub fn new(udp: &'a mut PaperInterface,
               http: &'a reqwest::Client,
               clients: &'a mut Clients,
               apps: &'a mut Apps,
               config: &'a Config
    ) -> Self {
        Self {
            udp,
            http,
            clients,
            apps,
            config
        }
    }

    pub async fn authenticate_client(&mut self, sender_id: u64, app_token: &str, version: &str) {
        if !self.is_version_allowed(version) {
            let msg = format!("Version {version} is not allowed.");
            self.send_err(sender_id, &msg).await;
            self.force_disconnect(sender_id).await;
            return;
        }

        if self.clients.player_count() >= MAX_PLAYERS {
            warn!("rejecting {}: already at the {} player cap", sender_id, MAX_PLAYERS);
            self.send_err(sender_id, "Server is full").await;
            self.force_disconnect(sender_id).await;
            return;
        }

        if !self.app_allowed(app_token).await {
            let msg = format!("App token {app_token} is not allowed.");
            self.send_err(sender_id, &msg).await;
            self.force_disconnect(sender_id).await;
            return;
        }

        let Some(client) = self.clients.get_mut(sender_id) else {
            warn!("attempted to authenticate a missing client {}", sender_id);
            return;
        };

        let app_id = match self.apps.get_by_token(app_token) {
            Some(app) => app.id,
            None => self.apps.create(app_token.to_string())
        };

        client.state = ClientState::Authenticated { app_id };
        self.send_packet(sender_id, &Packet::ClientAuthenticated, TransferChannel::Reliable, ).await;
    }

    fn is_version_allowed(&self, version: &str) -> bool {
        let versions = &self.config.allowed_versions;
        versions.contains(&version.to_string())
    }

    async fn app_allowed(&mut self, app: &str) -> bool {
        let remote = &self.config.remote_whitelist_endpoint;
        let token = &self.config.remote_whitelist_token;

        if remote.is_empty() || token.is_empty() {
            return self.check_local_whitelist(app);
        }

        match self.check_remote_whitelist(remote, app, token).await {
            Ok(res) => res,
            Err(e) => {
                warn!("failed to check remote whitelist, defaulting to local: {}", e);
                self.check_local_whitelist(app)
            }
        }
    }

    fn check_local_whitelist(&self, app: &str) -> bool {
        let whitelist = &self.config.whitelist;

        if whitelist.is_empty() {
            true
        } else {
            whitelist.contains(&app.to_string())
        }
    }

    async fn check_remote_whitelist(
        &self,
        endpoint: &str,
        app: &str,
        relay_token: &str,
    ) -> Result<bool, Box<dyn Error>> {
        let url = format!("{endpoint}/{app}");

        let res = self.http
            .get(&url)
            .header("X-Relay-Token", relay_token)
            .send()
            .await?;

        match res.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => Err(format!("unexpected status from endpoint: {s}").into()),
        }
    }

    async fn send_packet(&mut self, target: u64, packet: &Packet, channel: TransferChannel) {
        if let Err(e) = self.udp.send(target, packet.to_bytes(), channel, TrafficClass::Control).await {
            warn!("failed to send packet: {}", e);
        }
    }

    async fn send_err(&mut self, target: u64, msg: &str) {
        let msg = client_safe_error(msg);

        self.send_packet(
            target,
            &Packet::Error {
                error_code: 401,
                error_message: msg,
            },
            TransferChannel::Reliable,
        )
            .await;
    }

    async fn force_disconnect(&mut self, target: u64) {
        self.send_packet(target, &Packet::ForceDisconnect, TransferChannel::Reliable)
            .await;
        self.udp.remove_client(&target);
        // No disconnect event fires for a removed session, so clean up here.
        self.clients.remove(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::serialize::{push_string, read_string, MAX_STRING_LEN};

    fn survives_the_client(msg: &str) -> bool {
        let mut buf = Vec::new();
        push_string(&mut buf, &client_safe_error(msg));
        read_string(&buf).is_ok()
    }

    #[test]
    fn untruncated_messages_would_overflow_the_client_cap() {
        let token = "a".repeat(MAX_STRING_LEN);
        let raw = format!("App token {token} is not allowed.");

        let mut buf = Vec::new();
        push_string(&mut buf, &raw);
        assert!(read_string(&buf).is_err(), "the untruncated message should not fit");
    }

    #[test]
    fn truncated_messages_reach_the_client() {
        let token = "a".repeat(MAX_STRING_LEN);
        assert!(survives_the_client(&format!("App token {token} is not allowed.")));

        let version = "9".repeat(MAX_STRING_LEN);
        assert!(survives_the_client(&format!("Version {version} is not allowed.")));
    }

    #[test]
    fn multibyte_input_cannot_overflow_the_cap() {
        let token = "\u{1F600}".repeat(MAX_STRING_LEN);
        let truncated = client_safe_error(&format!("App token {token} is not allowed."));

        assert!(truncated.len() <= MAX_STRING_LEN, "{} bytes", truncated.len());
        assert!(survives_the_client(&format!("App token {token} is not allowed.")));
    }

    #[test]
    fn short_messages_are_left_alone() {
        assert_eq!(client_safe_error("Server is full"), "Server is full");
    }
}
