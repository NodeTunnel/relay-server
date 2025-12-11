use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;
use tokio::time::{Instant};
use tracing::{debug, error, info, warn};
use crate::config::loader::Config;
use crate::http::wrapper::HttpWrapper;
use crate::relay::{App, ClientSession};
use crate::relay::room::{Room, RoomIds};
use crate::protocol::packet::PacketType;
use crate::udp::common::{TransferChannel, ServerEvent};
use crate::udp::paper_interface::PaperInterface;

struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

pub struct RelayServer {
    transport: PaperInterface,
    http: Option<HttpWrapper>,
    pub config: Config,

    /// App ID -> App
    pub apps: HashMap<String, App>,
    /// ClientId -> ClientSession
    pub sessions: HashMap<u64, ClientSession>,
    /// ClientId -> (App ID, Room ID)
    pub(crate) client_to_room: HashMap<u64, (String, String)>,
    /// Room ID generator
    room_ids: RoomIds,
}

impl RelayServer {
    pub fn new(transport: PaperInterface, http: Option<HttpWrapper>, config: Config) -> Self {
        Self {
            transport,
            http,
            config,
            apps: HashMap::new(),
            sessions: HashMap::new(),
            client_to_room: HashMap::new(),
            room_ids: RoomIds::new(),
        }
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let mut last_resend = Instant::now();
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(1));
        cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Ok(_) = self.transport.socket.readable() => {
                    match self.transport.recv_events().await {
                        Ok(events) => {
                            for event in events {
                                self.handle_event(event).await;
                            }
                        }
                        Err(e) => {
                            error!("recv_events error: {}", e);
                            return Err(e.into());
                        }
                    }
                }

                _ = cleanup_interval.tick() => {
                    self.transport.cleanup_sessions(Duration::from_secs(5)).await;
                }
            }

            let now = Instant::now();

            if now.duration_since(last_resend) > Duration::from_millis(50) {
                self.transport.do_resends(Duration::from_millis(100)).await;
                last_resend = now;
            }

            tokio::task::yield_now().await;
        }
    }

    async fn handle_packet(&mut self, client: u64, data: Vec<u8>, channel: TransferChannel) {
        match PacketType::from_bytes(&data) {
            Ok(PacketType::Authenticate { app_id, version }) => {
                debug!("authenticating: {}", client);
                self.authenticate_client(client, app_id, version).await;
            }
            Ok(packet_type) => {
                if !self.sessions.contains_key(&client) {
                    self.send_packet(
                        client,
                        PacketType::Error {
                            error_code: 0,
                            error_message: "Unauthorized".to_string(),
                        },
                        TransferChannel::Reliable,
                    ).await;
                    debug!("unauthorized: {}", client);
                    return;
                }
                debug!("handling authorized pkt: {:?} for: {}", packet_type, client);
                self.handle_authorized_packet(client, packet_type, channel).await;
            }
            _ => {
                warn!("unexpected packet type from {}: {:?}. forcing disconnect", client, data);
                self.force_disconnect(client).await;
            }
        }
    }

    async fn handle_authorized_packet(&mut self, client_id: u64, packet_type: PacketType, channel: TransferChannel) {
        let session_app_id = match self.sessions.get(&client_id) {
            Some(s) => s.app_id.clone(),
            None => {
                self.send_packet(
                    client_id,
                    PacketType::Error {
                        error_code: 401,
                        error_message: "Unauthorized".into(),
                    },
                    TransferChannel::Reliable,
                ).await;
                return;
            }
        };

        match packet_type {
            PacketType::CreateRoom { is_public, name, max_players } => {
                self.create_room(client_id, session_app_id.clone(), is_public, name, max_players).await;
            }
            PacketType::ReqRooms => {
                self.send_rooms(client_id, session_app_id.clone()).await;
            }
            PacketType::JoinRoom { room_id } => {
                self.join_room(client_id, session_app_id.clone(), room_id).await
            }
            PacketType::GameData { data, from_peer } => {
                self.route_game_data(client_id, from_peer, data, channel).await;
            }
            _ => warn!("Unexpected authorized packet"),
        }
    }

    async fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientDisconnected { client_id } => {
                self.handle_disconnect(client_id).await;
            }
            ServerEvent::PacketReceived { client_id, data, channel } => {
                debug!("got packet: {:?}", data);
                self.handle_packet(client_id, data, channel).await;
            }
        }
    }

    pub async fn send_packet(&mut self, target_client: u64, packet_type: PacketType, channel: TransferChannel) {
        match self.transport.send(
            target_client,
            packet_type.to_bytes(),
            channel,
        ).await {
            Ok(_) => {},
            Err(e) => warn!("failed to send packet: {}", e)
        }
    }

    pub async fn force_disconnect(&mut self, target_client: u64) {
        self.send_packet(
            target_client,
            PacketType::ForceDisconnect,
            TransferChannel::Reliable
        ).await;
        self.transport.remove_client(&target_client);
    }

    /// Handlers

    async fn authenticate_client(&mut self, sender_id: u64, app_id: String, version: String) {
        if !self.app_allowed(app_id.as_str()).await {
            self.send_packet(
                sender_id,
                PacketType::Error {
                    error_code: 401,
                    error_message: "Unauthorized".into(),
                },
                TransferChannel::Reliable,
            ).await;

            self.force_disconnect(sender_id).await;

            return;
        }

        if !self.is_version_allowed(version.as_str()) {
            let msg = format!("Version {} is not allowed", version);

            self.send_packet(
                sender_id,
                PacketType::Error {
                    error_code: 401,
                    error_message: msg.into(),
                },
                TransferChannel::Reliable,
            ).await;

            self.force_disconnect(sender_id).await;
            return;
        }

        self.sessions.insert(
            sender_id,
            ClientSession {
                app_id: app_id.clone(),
                connected_at: Instant::now(),
            }
        );

        self.apps.entry(app_id.clone()).or_insert(App::new(app_id));

        self.send_packet(
            sender_id,
            PacketType::ClientAuthenticated,
            TransferChannel::Reliable,
        ).await;
    }

    async fn create_room(&mut self, sender_id: u64, app_id: String, is_public: bool, name: String, max_players: i32) {
        let app = self.apps.get_mut(&app_id).expect("App exists");

        let room_id = format!("{}{}", self.config.relay_id, self.room_ids.generate());

        let mut room = Room::new(room_id.clone(), sender_id, is_public, name, max_players);
        let peer_id = room.add_peer(sender_id);

        app.add_room(room);
        self.client_to_room.insert(sender_id, (app_id.clone(), room_id.clone()));

        self.send_packet(
            sender_id,
            PacketType::ConnectedToRoom {
                room_id,
                peer_id,
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn send_rooms(&mut self, target: u64, app_id: String) {
        let app = self.apps.get_mut(&app_id).expect("App exists");
        let mut available_rooms = vec![];

        for (_, room) in app.get_rooms() {
            if room.is_public {
                available_rooms.push(room.to_info());
            }
        }

        self.send_packet(
            target,
            PacketType::GetRooms {
                rooms: available_rooms
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn join_room(&mut self, sender_id: u64, app_id: String, room_id: String) {
        let (peer_id, host_id) = {
            let app = self.apps.get_mut(&app_id).expect("App exists");
            let Some(room) = app.get_room(&room_id) else {
                self.send_packet(
                    sender_id,
                    PacketType::Error {
                        error_code: 404,
                        error_message: "Room not found".into(),
                    },
                    TransferChannel::Reliable,
                ).await;
                return;
            };

            if room.is_full() {
                self.send_packet(
                    sender_id,
                    PacketType::Error {
                        error_code: 422,
                        error_message: "Room full".into(),
                    },
                    TransferChannel::Reliable,
                ).await;
                return;
            }

            let peer_id = room.add_peer(sender_id);
            let host_id = room.get_host();

            (peer_id, host_id)
        };

        self.client_to_room.insert(sender_id, (app_id, room_id.clone()));

        self.send_packet(
            sender_id,
            PacketType::ConnectedToRoom {
                room_id: room_id.clone(),
                peer_id,
            },
            TransferChannel::Reliable,
        ).await;

        self.send_packet(
            host_id,
            PacketType::PeerJoinedRoom {
                peer_id,
            },
            TransferChannel::Reliable
        ).await;
    }

    async fn route_game_data(&mut self, sender_id: u64, target_peer: i32, data: Vec<u8>, channel: TransferChannel) {
        let Some((app_id, room_id)) = self.client_to_room.get(&sender_id) else {
            warn!("{} tried to send relay data but is not in a room", sender_id);
            return;
        };

        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("{} has invalid app_id in index", sender_id);
            return;
        };

        let Some(room) = app.get_room(room_id) else {
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
            PacketType::GameData {
                from_peer: sender_godot_id,
                data,
            },
            channel,
        ).await;
    }

    async fn handle_disconnect(&mut self, client_id: u64) {
        self.sessions.remove(&client_id);

        let Some((app_id, room_id)) = self.client_to_room.remove(&client_id) else {
            info!("attempted to remove non-existent client");
            return;
        };

        let disconnect_info = {
            let Some(app) = self.apps.get_mut(&app_id) else {
                warn!("{} had invalid app_id on disconnect", client_id);
                return;
            };

            let Some(room) = app.get_room(&room_id) else {
                warn!("{} had invalid room_id on disconnect", client_id);
                return;
            };

            let godot_id = match room.get_godot_id(client_id) {
                Some(id) => id,
                None => {
                    warn!("{} not found in their room on disconnect", client_id);
                    return;
                }
            };

            DisconnectInfo {
                is_host: room.get_host() == client_id,
                godot_id,
                other_peers: room.get_renet_ids()
                    .into_iter()
                    .filter(|&id| id != client_id)
                    .collect(),
            }
        };

        if disconnect_info.is_host {
            self.handle_host_disconnect(app_id.clone(), room_id, disconnect_info.other_peers).await;
        } else {
            self.handle_peer_disconnect(app_id.clone(), room_id, client_id, disconnect_info.godot_id, disconnect_info.other_peers).await;
        }
    }

    async fn handle_host_disconnect(&mut self, app_id: String, room_id: String, peers_to_kick: Vec<u64>) {
        if let Some(app) = self.apps.get_mut(&app_id) {
            self.room_ids.free(&room_id);
            app.remove_room(&room_id);
        }

        for peer_id in &peers_to_kick {
            self.send_packet(*peer_id, PacketType::ForceDisconnect, TransferChannel::Reliable).await;
        }

        for peer_id in peers_to_kick {
            self.sessions.remove(&peer_id);
            self.client_to_room.remove(&peer_id);
            self.force_disconnect(peer_id).await;
        }
    }

    async fn handle_peer_disconnect(&mut self, app_id: String, room_id: String, client_id: u64, peer_godot_id: i32, other_peers: Vec<u64>) {
        if let Some(app) = self.apps.get_mut(&app_id) {
            if let Some(room) = app.get_room(&room_id) {
                room.remove_peer(client_id);

                if room.is_empty() {
                    self.room_ids.free(&room_id);
                    app.remove_room(&room_id);
                    return;
                }
            }
        }

        for peer_id in other_peers {
            self.send_packet(peer_id, PacketType::PeerLeftRoom { peer_id: peer_godot_id }, TransferChannel::Reliable).await;
        }
    }

    async fn app_allowed(&mut self, app: &str) -> bool {
        match self.http.as_mut() {
            Some(http) => match http.app_exists(app).await {
                Ok(exists) => exists,
                Err(e) => {
                    warn!("failed to check app_exists: {}", e);
                    self.check_local_whitelist(app)
                }
            },
            None => self.check_local_whitelist(app),
        }
    }

    fn check_local_whitelist(&self, app: &str) -> bool {
        let whitelist = &self.config.app_whitelist;

        if whitelist.is_empty() {
            true
        } else {
            whitelist.contains(&app.to_string())
        }
    }

    fn is_version_allowed(&self, version: &str) -> bool {
        let versions = &self.config.allowed_versions;
        versions.contains(&version.to_string())
    }
}
