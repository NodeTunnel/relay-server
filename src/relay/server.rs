use std::error::Error;
use std::time::Duration;
use serde::Serialize;
use tracing::{debug, info, warn};
use crate::config::loader::Config;
use crate::protocol::packet::Packet;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::relay::handlers::auth::AuthHandler;
use crate::relay::handlers::disconnect::DisconnectHandler;
use crate::relay::handlers::game_data::GameDataHandler;
use crate::relay::handlers::room::RoomHandler;
use crate::udp::common::{TransferChannel, ServerEvent};
use crate::udp::paper_interface::PaperInterface;

pub struct RelayServer {
    udp: PaperInterface,
    http_client: reqwest::Client,

    config: Config,
    apps: Apps,
    clients: Clients,
}

// The relay blocks on this check.
const WHITELIST_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Serialize)]
struct Heartbeat<'a> {
    relay_id: &'a str,
    version: &'a str,
    players: usize,
    rooms: usize,
}

impl RelayServer {
    pub fn new(transport: PaperInterface, config: Config) -> Self {
        Self {
            udp: transport,
            http_client: reqwest::Client::builder()
                .timeout(WHITELIST_REQUEST_TIMEOUT)
                .build()
                .expect("failed to build the HTTP client"),
            config,
            apps: Apps::new(),
            clients: Clients::new(),
        }
    }

    /// Starts the server loop.
    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        // TODO: remove magic numbers
        let mut cleanup = tokio::time::interval(Duration::from_secs(2));
        // TODO: remove magic numbers
        let mut resend  = tokio::time::interval(Duration::from_millis(50));
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);

        cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        resend.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = self.udp.recv_events() => {
                    let events = result?;
                    for event in events {
                        self.handle_event(event).await;
                    }
                }

                _ = cleanup.tick() => {
                    // TODO: remove magic numbers
                    for (client_id, reason) in self.udp.connection_manager.expired_sessions(Duration::from_secs(8)) {
                        DisconnectHandler::new(
                            &mut self.udp,
                            &mut self.clients,
                            &mut self.apps,
                        ).drop_client(client_id, reason).await;
                    }
                }

                _ = resend.tick() => {
                    // TODO: remove magic numbers
                    self.udp.do_resends(Duration::from_millis(100)).await;
                }

                _ = heartbeat.tick() => {
                    self.send_heartbeat();
                }
            }
        }
    }

    fn send_heartbeat(&self) {
        let endpoint = &self.config.heartbeat_endpoint;
        if endpoint.is_empty() {
            return;
        }

        let body = Heartbeat {
            relay_id: &self.config.relay_id,
            version: env!("CARGO_PKG_VERSION"),
            players: self.clients.player_count(),
            rooms: self.apps.iter().map(|app| app.rooms.iter().count()).sum(),
        };

        let request = self.http_client
            .post(endpoint)
            .header("X-Relay-Token", &self.config.remote_whitelist_token)
            .json(&body);

        tokio::spawn(async move {
            match request.send().await {
                Ok(res) if res.status().is_success() => debug!("heartbeat sent"),
                Ok(res) => warn!("heartbeat rejected: {}", res.status()),
                Err(e) => warn!("heartbeat failed: {}", e),
            }
        });
    }

    /// Handles an event from the UDP layer.
    async fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                self.clients.create(client_id);
            }
            ServerEvent::PacketReceived { client_id, data, channel } => {
                debug!("got packet: {:?}", data);
                self.handle_packet(client_id, data, channel).await;
            }
        }
    }

    /// Handles a packet received from `PaperUDP`.
    /// This checks the state of the client and routes packets based on the state.
    async fn handle_packet(&mut self, from_client_id: u64, data: Vec<u8>, channel: TransferChannel) {
        let Some(client) = self.clients.get(from_client_id) else {
            // This means that the client is not in the list of connected clients.
            // Likely a bug in the client or a malicious client.
            warn!("received a packet from an invalid peer");
            return;
        };

        let Ok(packet) = Packet::from_client_bytes(&data) else {
            warn!("received an invalid packet from {}", from_client_id);
            return;
        };

        if matches!(packet, Packet::Leave) {
            DisconnectHandler::new(
                &mut self.udp,
                &mut self.clients,
                &mut self.apps,
            ).handle_leave(from_client_id).await;
            return;
        }

        match client.state {
            ClientState::Connected => self.handle_unauthenticated_packet(from_client_id, &packet).await,
            ClientState::Authenticated { app_id } => self.handle_authenticated_packet(from_client_id, app_id, &packet).await,
            ClientState::InRoom { app_id, room_id } => self.handle_in_room_packet(from_client_id, app_id, room_id, &packet, &channel).await
        }
    }

    /// Delegates packets to various handlers when the client has yet to authenticate.
    async fn handle_unauthenticated_packet(&mut self, from_client_id: u64, packet: &Packet) {
        match packet {
            Packet::Authenticate { app_id, version } => {
                AuthHandler::new(
                    &mut self.udp,
                    &self.http_client,
                    &mut self.clients,
                    &mut self.apps,
                    &self.config
                ).authenticate_client(from_client_id, app_id, version).await;
            }
            _ => {
                // TODO: should probably alert the client that they need to authenticate first!
                warn!("unexpected packet type from {} in un-authenticated state: {:?}.", from_client_id, packet);
            }
        }
    }

    /// Delegates packets to various handlers when the client is authenticated, but not in a room.
    async fn handle_authenticated_packet(&mut self, from_client_id: u64, client_app_id: u64, packet: &Packet) {
        let mut rh = RoomHandler::new(
            &mut self.udp,
            &mut self.apps,
            &mut self.clients,
        );

        match packet {
            Packet::CreateRoom { is_public, metadata } =>
                rh.create_room(from_client_id, client_app_id, *is_public, metadata).await,
            Packet::ReqJoin { room_id, metadata } =>
                rh.recv_join_req(from_client_id, client_app_id, room_id, metadata).await,
            Packet::ReqRooms =>
                rh.send_rooms(from_client_id, client_app_id).await,
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in authenticated state: {:?}.", from_client_id, packet);
            }
        }
    }

    /// Delegates packets to various handlers when the client is in a room.
    async fn handle_in_room_packet(&mut self, from_client_id: u64, client_app_id: u64, client_room_id: u64, packet: &Packet, channel: &TransferChannel) {
        match packet {
            Packet::UpdateRoom { metadata, room_id: _room_id } => {
                RoomHandler::new(
                    &mut self.udp,
                    &mut self.apps,
                    &mut self.clients,
                ).update_room(from_client_id, client_app_id, client_room_id, metadata).await;
            }
            Packet::JoinRes { target_id, allowed, room_id: _room_id } =>
                RoomHandler::new(
                    &mut self.udp,
                    &mut self.apps,
                    &mut self.clients,
                ).recv_join_res(client_app_id, *target_id, client_room_id, allowed).await,
            Packet::KickPeer { peer_id } => {
                DisconnectHandler::new(
                    &mut self.udp,
                    &mut self.clients,
                    &mut self.apps,
                ).kick_peer(from_client_id, client_app_id, client_room_id, *peer_id).await;
            }
            Packet::GameData { from_peer, data } => {
                GameDataHandler::new(
                    &mut self.udp,
                    &mut self.apps,
                ).route_game_data(from_client_id, client_app_id, client_room_id, *from_peer, data, channel).await;
            }
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in room state: {:?}.", from_client_id, packet);
            }
        }
    }

    /// Forcefully disconnects all clients from the server.
    /// Should be called when the server shuts down.
    pub async fn cleanup(&mut self) {
        let disconnects = self.clients.ids();
        let mut to_remove: Vec<(u64, u64)> = Vec::new();

        for app in self.apps.iter() {
            for room in app.rooms.iter() {
                to_remove.push((app.id, room.id));
            }
        }

        info!("disconnecting {} peers", disconnects.len());

        let mut dh = DisconnectHandler::new(
            &mut self.udp,
            &mut self.clients,
            &mut self.apps
        );

        for id in disconnects {
            dh.notify_and_disconnect(id, "Relay is shutting down").await;
        }

        let mut rh = RoomHandler::new(
            &mut self.udp,
            &mut self.apps,
            &mut self.clients,
        );

        for (app_id, room_id) in to_remove {
            rh.remove_room(app_id, room_id);
        }
    }
}
