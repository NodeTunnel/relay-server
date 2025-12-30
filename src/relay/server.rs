use std::error::Error;
use std::time::Duration;
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

    async fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                self.clients.create(client_id);
            }
            ServerEvent::ClientDisconnected { client_id } => {
                DisconnectHandler::new(
                    &mut self.transport,
                    &mut self.clients,
                    &mut self.apps,
                ).handle_disconnect(client_id).await;
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
                warn!("unexpected packet type from {} in authenticated state: {:?}.", from_client_id, packet);
            }
        }
    }

    async fn handle_in_room_packet(&mut self, from_client_id: u64, client_app_id: u64, client_room_id: u64, packet: &Packet, channel: &TransferChannel) {
        match packet {
            Packet::UpdateRoom { metadata, room_id: _room_id } => {
                RoomHandler::new(
                    &mut self.transport,
                    &mut self.apps,
                    &mut self.clients,
                ).update_room(from_client_id, client_app_id, client_room_id, metadata).await;
            }
            Packet::JoinRes { target_id, allowed, room_id: _room_id } =>
                RoomHandler::new(
                    &mut self.transport,
                    &mut self.apps,
                    &mut self.clients,
                ).recv_join_res(client_app_id, *target_id, client_room_id, allowed).await,
            Packet::GameData { from_peer, data } => {
                GameDataHandler::new(
                    &mut self.transport,
                    &mut self.apps,
                ).route_game_data(from_client_id, client_app_id, client_room_id, *from_peer, data, channel).await;
            }
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in room state: {:?}.", from_client_id, packet);
            }
        }
    }

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

        let mut dh = DisconnectHandler::new(
            &mut self.transport,
            &mut self.clients,
            &mut self.apps
        );

        for id in disconnects {
            dh.force_disconnect(id).await;
        }

        let mut rh = RoomHandler::new(
            &mut self.transport,
            &mut self.apps,
            &mut self.clients,
        );

        for (app_id, room_id) in to_remove {
            rh.remove_room(app_id, room_id);
        }
    }
}
