use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;
use reqwest::{Client, StatusCode};
use tracing::{debug, info, warn};
use crate::config::loader::Config;
use crate::relay::App;
use crate::relay::room::{Room, RoomIds};
use crate::protocol::packet::Packet;
use crate::udp::common::{TransferChannel, ServerEvent};
use crate::udp::paper_interface::PaperInterface;

// TODO: make into enum
struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

#[derive(Clone)]
enum GodotClientState {
    Connected,
    Authenticated { app_id: String },
    InRoom { app_id: String, room_id: String }
}

struct GodotClient {
    state: GodotClientState,
}

impl GodotClient {
    pub fn new() -> Self {
        Self {
            state: GodotClientState::Connected
        }
    }
}

pub struct RelayServer {
    transport: PaperInterface,
    http_client: Client,

    pub config: Config,

    /// App ID -> App
    pub apps: HashMap<String, App>,
    /// ClientId -> GodotClient
    clients: HashMap<u64, GodotClient>,
    /// Room ID generator
    room_ids: RoomIds,
}

impl RelayServer {
    pub fn new(transport: PaperInterface, config: Config) -> Self {
        Self {
            transport,
            http_client: Client::new(),
            config,
            apps: HashMap::new(),
            clients: HashMap::new(),
            room_ids: RoomIds::new(),
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

    /// Handles incoming packets from a client.
    /// Will result in warnings if the packet is invalid or the client is not in the correct state.
    /// Routes to different handlers depending on the client's state.
    async fn handle_packet(&mut self, from_client_id: u64, data: Vec<u8>, channel: TransferChannel) {
        let client_state = {
            let Some(client) = self.clients.get(&from_client_id) else {
                // This means that the client is not in the list of connected clients.
                // Likely a bug in the client or a malicious client.
                warn!("received a packet from an invalid peer");
                return;
            };

            client.state.clone()
        };

        let Ok(packet) = Packet::from_bytes(&data) else {
            warn!("received an invalid packet from {}", from_client_id);
            return;
        };

        match client_state {
            GodotClientState::Connected => self.handle_unauth_packet(from_client_id, &packet).await,
            GodotClientState::Authenticated { app_id } => self.handle_auth_packet(from_client_id, &app_id, &packet).await,
            GodotClientState::InRoom { app_id, room_id } => self.handle_room_packet(from_client_id, &app_id, &room_id, &packet, &channel).await
        };
    }

    async fn handle_unauth_packet(&mut self, from_client_id: u64, packet: &Packet) {
        match packet {
            Packet::Authenticate { app_id, version } =>
                self.authenticate_client(from_client_id, app_id, version).await,
            _ => {
                // TODO: should probably alert the client that they need to authenticate first!
                warn!("unexpected packet type from {} in un-authenticated state: {:?}.", from_client_id, packet)
            }
        }
    }

    async fn handle_auth_packet(&mut self, from_client_id: u64, client_app_id: &str, packet: &Packet) {
        match packet {
            Packet::CreateRoom { is_public, metadata } =>
                self.create_room(from_client_id, client_app_id, *is_public, metadata).await,
            Packet::ReqJoin { room_id, metadata } =>
                self.recv_join_req(from_client_id, client_app_id, room_id, metadata).await,
            Packet::ReqRooms =>
                self.send_rooms(from_client_id, client_app_id).await,
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in authenticated state: {:?}.", from_client_id, packet)
            }
        }
    }

    async fn handle_room_packet(&mut self, from_client_id: u64, client_app_id: &str, client_room_id: &str, packet: &Packet, channel: &TransferChannel) {
        match packet {
            Packet::UpdateRoom { room_id, metadata } =>
                self.update_room(from_client_id, client_app_id, client_room_id, metadata).await,
            Packet::JoinRes { target_id, room_id, allowed } =>
                self.recv_join_res(client_app_id, *target_id, client_room_id, allowed).await,
            Packet::GameData { from_peer, data } =>
                self.route_game_data(from_client_id, client_app_id, client_room_id, *from_peer, data, channel).await,
            _ => {
                // TODO: should probably alert the client that they are in an unexpected state?
                warn!("unexpected packet type from {} in room state: {:?}.", from_client_id, packet)
            }
        }
    }

    async fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                self.handle_connect(client_id);
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

    fn handle_connect(&mut self, client_id: u64) {
        self.clients.insert(client_id, GodotClient::new());
    }

    async fn handle_disconnect(&mut self, client_id: u64) {
        let Some(client) = self.clients.remove(&client_id) else {
            warn!("unregistered client disconnected");
            return;
        };

        match client.state {
            GodotClientState::InRoom { app_id, room_id } => {
                self.handle_room_disconnect(client_id, &app_id, &room_id).await;
            }
            _ => {}
        }
    }

    pub async fn send_packet(&mut self, target_client: u64, packet: &Packet, channel: TransferChannel) {
        match self.transport.send(
            target_client,
            packet.to_bytes(),
            channel,
        ).await {
            Ok(_) => {},
            Err(e) => warn!("failed to send packet: {}", e)
        }
    }

    pub async fn force_disconnect(&mut self, target_client: u64) {
        self.send_packet(
            target_client,
            &Packet::ForceDisconnect,
            TransferChannel::Reliable
        ).await;
        self.transport.remove_client(&target_client);
    }

    async fn authenticate_client(&mut self, sender_id: u64, app_id: &str, version: &str) {
        if !self.is_version_allowed(version) {
            let msg = format!("Version {} is not allowed", version);

            self.send_packet(
                sender_id,
                &Packet::Error {
                    error_code: 401,
                    error_message: msg.into(),
                },
                TransferChannel::Reliable,
            ).await;

            self.force_disconnect(sender_id).await;
            return;
        }

        let Some(client) = self.clients.get_mut(&sender_id) else {
            warn!("attempted to authenticate a missing client {}", sender_id);
            return;
        };

        let app_id_owned = app_id.to_string();

        client.state = GodotClientState::Authenticated { app_id: app_id_owned.clone() };

        self.apps.entry(app_id_owned.clone()).or_insert(App::new(app_id_owned.clone()));

        self.send_packet(
            sender_id,
            &Packet::ClientAuthenticated,
            TransferChannel::Reliable,
        ).await;
    }

    async fn create_room(&mut self, sender_id: u64, app_id: &str, is_public: bool, metadata: &str) {
        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("attempted to create a room for a missing app: {}", app_id);
            return;
        };

        let Some(client) = self.clients.get_mut(&sender_id) else {
            warn!("attempted to create a room for a missing client: {}", sender_id);
            return;
        };

        let room_id = format!("{}{}", self.config.relay_id, self.room_ids.generate());

        let mut room = Room::new(room_id.clone(), sender_id, is_public, metadata.to_string());
        let peer_id = room.add_peer(sender_id);

        client.state = GodotClientState::InRoom { app_id: app_id.to_string(), room_id: room_id.clone() };
        app.add_room(room);

        self.send_packet(
            sender_id,
            &Packet::ConnectedToRoom {
                room_id,
                peer_id,
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn send_rooms(&mut self, target: u64, app_id: &str) {
        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("attempted to list rooms for a missing app: {}", app_id);
            return;
        };

        let mut available_rooms = vec![];

        for (_, room) in app.get_rooms() {
            if room.is_public {
                available_rooms.push(room.to_info());
            }
        }

        self.send_packet(
            target,
            &Packet::GetRooms {
                rooms: available_rooms
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn recv_join_req(&mut self, sender_id: u64, app_id: &str, room_id: &str, metadata: &str) {
        let host_id = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("attempted to handle join request for a missing app: {}", app_id);
                return;
            };

            let Some(room) = app.get_room(&room_id) else {
                self.send_packet(
                    sender_id.clone(),
                    &Packet::Error {
                        error_code: 404,
                        error_message: "Room not found".into(),
                    },
                    TransferChannel::Reliable,
                ).await;
                return;
            };

            room.get_host()
        };

        self.send_packet(
            host_id,
            &Packet::PeerJoinAttempt {
                target_id: sender_id.clone(),
                metadata: metadata.to_string()
            },
            TransferChannel::Reliable
        ).await;
    }

    async fn recv_join_res(&mut self, app_id: &str, target_id: u64, room_id: &str, allowed: &bool) {
        if *allowed {
            let Some(client) = self.clients.get_mut(&target_id) else {
                warn!("attempted to handle join response for a missing client: {}", target_id);
                return;
            };

            let (peer_id, host_id) = {
                let app = self.apps.get_mut(app_id).expect("App exists");
                let Some(room) = app.get_room(&room_id) else {
                    self.send_packet(
                        target_id,
                        &Packet::Error {
                            error_code: 404,
                            error_message: "Room not found".into(),
                        },
                        TransferChannel::Reliable,
                    ).await;
                    return;
                };

                let peer_id = room.add_peer(target_id);
                let host_id = room.get_host();

                (peer_id, host_id)
            };

            client.state = GodotClientState::InRoom { app_id: app_id.to_string(), room_id: room_id.to_string() };

            self.send_packet(
                target_id,
                &Packet::ConnectedToRoom {
                    room_id: room_id.to_string(),
                    peer_id,
                },
                TransferChannel::Reliable,
            ).await;

            self.send_packet(
                host_id,
                &Packet::PeerJoinedRoom {
                    peer_id,
                },
                TransferChannel::Reliable
            ).await;

            return;
        }

        self.send_packet(
            target_id,
            &Packet::Error {
                error_code: 401,
                error_message: "Room host denied entry".into(),
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn update_room(&mut self, sender_id: u64, app_id: &str, room_id: &str, metadata: &str) {
        let app = self.apps.get_mut(app_id).expect("App exists");
        let Some(room) = app.get_room(&room_id) else {
            self.send_packet(
                sender_id,
                &Packet::Error {
                    error_code: 404,
                    error_message: "Room not found".into(),
                },
                TransferChannel::Reliable,
            ).await;
            return;
        };

        room.metadata = metadata.to_string();
    }

    async fn route_game_data(&mut self, sender_id: u64, client_app_id: &str, client_room_id: &str, target_peer: i32, data: &Vec<u8>, channel: &TransferChannel) {
        let Some(app) = self.apps.get_mut(client_app_id) else {
            warn!("{} has invalid app_id in index", sender_id);
            return;
        };

        let Some(room) = app.get_room(client_room_id) else {
            warn!("{} has invalid room_id in index", sender_id);
            return;
        };

        let Some(sender_godot_id) = room.get_godot_id(sender_id) else {
            warn!("{} not found in their own room", sender_id);
            return;
        };

        let Some(target_renet_id) = room.get_renet_id(target_peer) else {
            return;
        };

        self.send_packet(
            target_renet_id,
            &Packet::GameData {
                from_peer: sender_godot_id,
                data: data.clone(),
            },
            *channel,
        ).await;
    }

    async fn handle_room_disconnect(&mut self, sender_id: u64, app_id: &str, room_id: &str) {
        let disconnect_info = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("{} had invalid app_id on disconnect", sender_id);
                return;
            };

            let Some(room) = app.get_room(room_id) else {
                warn!("{} had invalid room_id on disconnect", sender_id);
                return;
            };

            let godot_id = match room.get_godot_id(sender_id) {
                Some(id) => id,
                None => {
                    warn!("{} not found in their room on disconnect", sender_id);
                    return;
                }
            };

            DisconnectInfo {
                is_host: room.get_host() == sender_id,
                godot_id,
                other_peers: room.get_renet_ids()
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

    async fn handle_host_disconnect(&mut self, app_id: &str, room_id: &str, peers_to_kick: Vec<u64>) {
        info!("host disconnected");
        self.remove_room(&app_id, &room_id).await;

        for peer_id in &peers_to_kick {
            self.send_packet(*peer_id, &Packet::ForceDisconnect, TransferChannel::Reliable).await;
        }

        for peer_id in peers_to_kick {
            self.clients.remove(&peer_id);
            self.force_disconnect(peer_id).await;
        }
    }

    async fn handle_peer_disconnect(&mut self, app_id: &str, room_id: &str, client_id: u64, peer_godot_id: i32, other_peers: Vec<u64>) {
        info!("peer disconnected");
        if let Some(app) = self.apps.get_mut(app_id) {
            if let Some(room) = app.get_room(&room_id) {
                room.remove_peer(client_id);
            }
        }

        for peer_id in other_peers {
            self.send_packet(peer_id, &Packet::PeerLeftRoom { peer_id: peer_godot_id }, TransferChannel::Reliable).await;
        }
    }

    async fn remove_room(&mut self, app_id: &str, room_id: &str) {
        if let Some(app) = self.apps.get_mut(app_id) {
            self.room_ids.free(&room_id);
            app.remove_room(&room_id);
        }
    }

    async fn app_allowed(&mut self, app: &str) -> bool {
        let remote = &self.config.remote_whitelist_endpoint;
        let token = &self.config.remote_whitelist_token;

        if remote.is_empty() || token.is_empty() {
            self.check_local_whitelist(app)
        } else {
            match self.check_remote_whitelist(remote, app, token).await {
                Ok(res) => res,
                Err(e) => {
                    warn!("failed to check remote whitelist, defaulting to local: {}", e);
                    self.check_local_whitelist(app)
                }
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
        let url = format!("{}/{}", endpoint, app);

        let res = self.http_client
            .get(&url)
            .header("X-Relay-Token", relay_token)
            .send()
            .await?;

        match res.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => Err(format!("unexpected status from endpoint: {}", s).into()),
        }
    }

    fn is_version_allowed(&self, version: &str) -> bool {
        let versions = &self.config.allowed_versions;
        versions.contains(&version.to_string())
    }

    pub async fn cleanup(&mut self) {
        let mut disconnects: Vec<u64> = Vec::new();
        let mut to_remove: Vec<(String, String)> = Vec::new();

        for (app_id, app) in self.apps.iter_mut() {
            for (room_id, room) in app.get_rooms() {
                disconnects.extend(room.get_renet_ids().iter().copied());
                to_remove.push((app_id.clone(), room_id.clone()));
            }
        }

        info!("disconnecting {} peers", disconnects.len());

        for id in disconnects {
            self.send_packet(id, &Packet::ForceDisconnect, TransferChannel::Reliable)
                .await;
        }

        for (app_id, room_id) in to_remove {
            self.remove_room(&app_id, &room_id).await;
        }
    }
}
