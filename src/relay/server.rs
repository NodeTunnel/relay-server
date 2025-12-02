use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;
use tokio::time::{Instant};
use log::warn;
use crate::config::loader::Config;
use crate::relay::{App, ClientSession};
use crate::relay::room::Room;
use crate::protocol::packet::PacketType;
use crate::transport::common::{Channel, ServerEvent};
use crate::transport::server::PaperUDP;

struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

pub struct RelayServer {
    transport: PaperUDP,
    pub config: Config,

    /// App ID -> App
    pub apps: HashMap<String, App>,
    /// ClientId -> ClientSession
    pub sessions: HashMap<u64, ClientSession>,
    /// ClientId -> (App ID, Room ID)
    pub(crate) client_to_room: HashMap<u64, (String, String)>,
}

impl RelayServer {
    pub fn new(transport: PaperUDP, config: Config) -> Self {
        Self {
            transport,
            config,
            apps: HashMap::new(),
            sessions: HashMap::new(),
            client_to_room: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        let mut last_resend = Instant::now();
        let mut last_ack = Instant::now();
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(1));
        cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Ok(_) = self.transport.socket.readable() => {
                    let events = self.transport.recv_events().await;
                    for event in events {
                        self.handle_event(event).await;
                    }
                }

                _ = cleanup_interval.tick() => {
                    self.transport.cleanup_sessions(Duration::from_secs(5)).await;
                }
            }

            let now = Instant::now();

            if now.duration_since(last_resend) > Duration::from_millis(50) {
                self.transport.resend_unacked().await.ok();
                last_resend = now;
            }

            if now.duration_since(last_ack) > Duration::from_millis(10) {
                self.transport.send_acks().await.ok();
                last_ack = now;
            }

            tokio::task::yield_now().await;
        }
    }

    async fn handle_packet(&mut self, client: u64, data: Vec<u8>, channel: Channel) {
        match PacketType::from_bytes(&data) {
            Ok(PacketType::Authenticate { app_id, version }) => {
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
                        Channel::Reliable,
                    ).await;
                    return;
                }
                self.handle_authorized_packet(client, packet_type, channel).await;
            }
            _ => {
                warn!("Unexpected packet type from {}", client);
            }
        }
    }

    async fn handle_authorized_packet(&mut self, client_id: u64, packet_type: PacketType, channel: Channel) {
        let session_app_id = match self.sessions.get(&client_id) {
            Some(s) => s.app_id.clone(),
            None => {
                self.send_packet(
                    client_id,
                    PacketType::Error {
                        error_code: 401,
                        error_message: "Unauthorized".into(),
                    },
                    Channel::Reliable,
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
            ServerEvent::ClientConnected { client_id } => {
                println!("Client connected: {}", client_id);
            }
            ServerEvent::ClientDisconnected { client_id } => {
                println!("Client disconnected: {}", client_id);
                self.handle_disconnect(client_id).await;
            }
            ServerEvent::PacketReceived { client_id, data, channel } => {
                self.handle_packet(client_id, data, channel).await;
            }
        }
    }

    pub async fn send_packet(&self, target_client: u64, packet_type: PacketType, channel: Channel) {
        match self.transport.send(
            target_client,
            packet_type.to_bytes(),
            channel,
        ).await {
            Ok(_) => {},
            Err(e) => println!("Failed to send packet: {}", e)
        }
    }

    pub fn force_disconnect(&mut self, target_client: u64) {
        self.transport.disconnect_client(target_client);
    }

    /// Handlers

    async fn authenticate_client(&mut self, sender_id: u64, app_id: String, version: String) {
        if !self.is_app_allowed(app_id.as_str()) {
            self.send_packet(
                sender_id,
                PacketType::Error {
                    error_code: 401,
                    error_message: "Unauthorized".into(),
                },
                Channel::Reliable,
            ).await;

            self.force_disconnect(sender_id);

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
                Channel::Reliable,
            ).await;

            self.force_disconnect(sender_id);
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
            Channel::Reliable,
        ).await;
    }

    async fn create_room(&mut self, sender_id: u64, app_id: String, is_public: bool, name: String, max_players: i32) {
        let app = self.apps.get_mut(&app_id).expect("App exists");

        let room_id = match &self.config.relay_id {
            Some(relay_id) => format!("{}_{}", relay_id, sender_id),
            None => sender_id.to_string(),
        };

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
            Channel::Reliable,
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
            Channel::Reliable,
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
                    Channel::Reliable,
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
                    Channel::Reliable,
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
            Channel::Reliable,
        ).await;

        self.send_packet(
            host_id,
            PacketType::PeerJoinedRoom {
                peer_id,
            },
            Channel::Reliable
        ).await;
    }

    async fn route_game_data(&mut self, sender_id: u64, target_peer: i32, data: Vec<u8>, channel: Channel) {
        let Some((app_id, room_id)) = self.client_to_room.get(&sender_id) else {
            println!("Client {} tried to send relay data but is not in a room", sender_id);
            return;
        };

        let Some(app) = self.apps.get_mut(app_id) else {
            println!("Client {} has invalid app_id in index", sender_id);
            return;
        };

        let Some(room) = app.get_room(room_id) else {
            println!("Client {} has invalid room_id in index", sender_id);
            return;
        };

        let Some(sender_godot_id) = room.get_godot_id(sender_id) else {
            println!("Client {} not found in their own room", sender_id);
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
            return;
        };

        let disconnect_info = {
            let Some(app) = self.apps.get_mut(&app_id) else {
                warn!("Client {} had invalid app_id on disconnect", client_id);
                return;
            };

            let Some(room) = app.get_room(&room_id) else {
                warn!("Client {} had invalid room_id on disconnect", client_id);
                return;
            };

            let godot_id = match room.get_godot_id(client_id) {
                Some(id) => id,
                None => {
                    warn!("Client {} not found in their room on disconnect", client_id);
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
            app.remove_room(&room_id);
        }

        for peer_id in &peers_to_kick {
            self.send_packet(*peer_id, PacketType::ForceDisconnect, Channel::Reliable).await;
        }

        for peer_id in peers_to_kick {
            self.sessions.remove(&peer_id);
            self.client_to_room.remove(&peer_id);
            self.force_disconnect(peer_id);
        }
    }

    async fn handle_peer_disconnect(&mut self, app_id: String, room_id: String, client_id: u64, peer_godot_id: i32, other_peers: Vec<u64>) {
        if let Some(app) = self.apps.get_mut(&app_id) {
            if let Some(room) = app.get_room(&room_id) {
                room.remove_peer(client_id);

                if room.is_empty() {
                    app.remove_room(&room_id);
                    return;
                }
            }
        }

        for peer_id in other_peers {
            self.send_packet(peer_id, PacketType::PeerLeftRoom { peer_id: peer_godot_id }, Channel::Reliable).await;
        }
    }

    // TODO: Handle error
    fn is_app_allowed(&self, app: &str) -> bool {
        let whitelist = &self.config.app_whitelist;

        if whitelist.is_empty() {
            true
        } else {
            whitelist.contains(&app.to_string())
        }
    }

    // TODO: Handle error
    fn is_version_allowed(&self, version: &str) -> bool {
        let versions = &self.config.allowed_versions;
        versions.contains(&version.to_string())
    }
}
