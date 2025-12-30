use std::error::Error;
use std::time::Duration;
use tracing::{debug, info, warn};
use crate::config::loader::Config;
use crate::protocol::packet::Packet;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::relay::handlers::auth::AuthHandler;
use crate::relay::handlers::game_data::GameDataHandler;
use crate::relay::handlers::room::RoomHandler;
use crate::udp::common::{TransferChannel, ServerEvent};
use crate::udp::paper_interface::PaperInterface;

struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

pub struct RelayServer {
    transport: PaperInterface,
    http_client: reqwest::Client,

    config: Config,
    apps: Apps,
    clients: Clients,
}

impl RelayServer {
    pub fn new(transport: PaperInterface, config: Config) -> Self {
        Self {
            transport,
            http_client: reqwest::Client::new(),
            config,
            apps: Apps::new(),
            clients: Clients::new(),
        }
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        // TODO: remove magic numbers
        let mut cleanup = tokio::time::interval(Duration::from_secs(1));
        // TODO: remove magic numbers
        let mut resend  = tokio::time::interval(Duration::from_millis(50));

        cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        resend.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = self.transport.recv_events() => {
                    let events = result?;
                    for event in events {
                        self.handle_event(event).await;
                    }
                }

                _ = cleanup.tick() => {
                    // TODO: remove magic numbers
                    for client_id in self.transport.connection_manager.cleanup_sessions(Duration::from_secs(5)) {
                        self.handle_event(ServerEvent::ClientDisconnected { client_id }).await;
                    }
                }

                _ = resend.tick() => {
                    // TODO: remove magic numbers
                    self.transport.do_resends(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// --------------
    /// Event Handling
    /// --------------

    async fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                self.clients.create(client_id);
            }
            ServerEvent::ClientDisconnected { client_id } => {
                self.handle_disconnect(client_id).await;
            }
            ServerEvent::PacketReceived { client_id, data, channel } => {
                debug!("got packet: {:?}", data);
                self.handle_packet(client_id, data, channel).await;
            }
        }
    }

    async fn handle_packet(&mut self, from_client_id: u64, data: Vec<u8>, channel: TransferChannel) {
        let Some(client) = self.clients.get(from_client_id) else {
            // This means that the client is not in the list of connected clients.
            // Likely a bug in the client or a malicious client.
            warn!("received a packet from an invalid peer");
            return;
        };

        let Ok(packet) = Packet::from_bytes(&data) else {
            warn!("received an invalid packet from {}", from_client_id);
            return;
        };

        match client.state {
            ClientState::Connected => self.handle_unauthenticated_packet(from_client_id, &packet).await,
            ClientState::Authenticated { app_id } => self.handle_authenticated_packet(from_client_id, app_id, &packet).await,
            ClientState::InRoom { app_id, room_id } => self.handle_in_room_packet(from_client_id, app_id, room_id, &packet, &channel).await
        }
    }

    async fn handle_unauthenticated_packet(&mut self, from_client_id: u64, packet: &Packet) {
        match packet {
            Packet::Authenticate { app_id, version } => {
                AuthHandler::new(
                    &mut self.transport,
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

    async fn handle_authenticated_packet(&mut self, from_client_id: u64, client_app_id: u64, packet: &Packet) {
        let mut rh = RoomHandler::new(
            &mut self.transport,
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
                warn!("unexpected packet type from {} in authenticated state: {:?}.", from_client_id, packet)
            }
        }
    }

    async fn handle_in_room_packet(&mut self, from_client_id: u64, client_app_id: u64, client_room_id: u64, packet: &Packet, channel: &TransferChannel) {
        let mut rh = RoomHandler::new(
            &mut self.transport,
            &mut self.apps,
            &mut self.clients,
        );

        match packet {
            Packet::UpdateRoom { room_id, metadata } => {
                rh.update_room(from_client_id, client_app_id, client_room_id, metadata).await;
            }
            Packet::JoinRes { target_id, room_id, allowed } =>
                rh.recv_join_res(client_app_id, *target_id, client_room_id, allowed).await,
            Packet::GameData { from_peer, data } => {
                GameDataHandler::new(
                    &mut self.transport,
                    &mut self.apps,
                ).route_game_data(from_client_id, client_app_id, client_room_id, *from_peer, data, channel).await;
            }
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in room state: {:?}.", from_client_id, packet)
            }
        }
    }

    /// -------------------
    /// Disconnect Handling
    /// -------------------

    async fn handle_disconnect(&mut self, client_id: u64) {
        let Some(client) = self.clients.remove(client_id) else {
            warn!("unregistered client disconnected");
            return;
        };

        match client.state {
            ClientState::InRoom { app_id, room_id } => {
                self.handle_room_disconnect(client_id, app_id, room_id).await;
            }
            _ => {}
        }
    }

    async fn handle_room_disconnect(&mut self, sender_id: u64, app_id: u64, room_id: u64) {
        let disconnect_info = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("{} had invalid app_id on disconnect", sender_id);
                return;
            };

            let Some(room) = app.rooms.get(room_id) else {
                warn!("{} had invalid room_id on disconnect", sender_id);
                return;
            };

            let Some(godot_id) = room.client_to_gd(sender_id) else {
                warn!("{} not found in their room on disconnect", sender_id);
                return;
            };

            DisconnectInfo {
                is_host: room.get_host() == sender_id,
                godot_id,
                other_peers: room.get_clients()
                    .into_iter()
                    .filter(|&id| id != sender_id)
                    .collect(),
            }
        };

        if disconnect_info.is_host {
            self.handle_host_disconnect(app_id, room_id, disconnect_info.other_peers).await;
        } else {
            self.handle_peer_disconnect(app_id, room_id, sender_id, disconnect_info.godot_id, disconnect_info.other_peers).await;
        }
    }

    async fn handle_host_disconnect(&mut self, app_id: u64, room_id: u64, peers_to_kick: Vec<u64>) {
        info!("host disconnected");
        RoomHandler::new(
            &mut self.transport,
            &mut self.apps,
            &mut self.clients,
        ).remove_room(app_id, room_id);

        for peer_id in peers_to_kick {
            self.clients.remove(peer_id);
            self.force_disconnect(peer_id).await;
        }
    }

    async fn handle_peer_disconnect(&mut self, app_id: u64, room_id: u64, client_id: u64, peer_godot_id: i32, other_peers: Vec<u64>) {
        info!("peer disconnected");
        if let Some(app) = self.apps.get_mut(app_id) {
            if let Some(room) = app.rooms.get_mut(room_id) {
                room.remove_peer(client_id);
            }
        }

        for peer_id in other_peers {
            self.send_packet(peer_id, &Packet::PeerLeftRoom { peer_id: peer_godot_id }, TransferChannel::Reliable).await;
        }
    }

    /// --------------
    /// Packet Helpers
    /// --------------

    async fn send_packet(&mut self, target_client: u64, packet: &Packet, channel: TransferChannel) {
        match self.transport.send(
            target_client,
            packet.to_bytes(),
            channel,
        ).await {
            Ok(_) => {},
            Err(e) => warn!("failed to send packet: {}", e)
        }
    }

    async fn send_err(&mut self, target_client: u64, err_msg: &str) {
        self.send_packet(
            target_client,
            &Packet::Error {
                error_code: 401,
                error_message: err_msg.to_string(),
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn force_disconnect(&mut self, target_client: u64) {
        self.send_packet(
            target_client,
            &Packet::ForceDisconnect,
            TransferChannel::Reliable
        ).await;
        self.transport.remove_client(&target_client);
    }

    /// ---------
    /// Utilities
    /// ---------

    pub async fn cleanup(&mut self) {
        let mut disconnects: Vec<u64> = Vec::new();
        let mut to_remove: Vec<(u64, u64)> = Vec::new();

        for app in self.apps.iter() {
            for room in app.rooms.iter() {
                disconnects.extend(room.get_clients().iter().copied());
                to_remove.push((app.id, room.id));
            }
        }

        info!("disconnecting {} peers", disconnects.len());

        for id in disconnects {
            self.send_packet(id, &Packet::ForceDisconnect, TransferChannel::Reliable)
                .await;
        }

        for (app_id, room_id) in to_remove {
            RoomHandler::new(
                &mut self.transport,
                &mut self.apps,
                &mut self.clients,
            ).remove_room(app_id, room_id);
        }
    }
}
