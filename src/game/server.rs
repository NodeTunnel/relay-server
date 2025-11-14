use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;
use renet::{ClientId, ServerEvent};
use tokio::time::{sleep, Instant};
use log::warn;
use crate::config::Config;
use crate::game::{App, ClientSession};
use crate::game::room::Room;
use crate::protocol::packet::PacketType;
use crate::transport::{Channel, Packet, RenetTransport};

pub struct GameServer {
    transport: RenetTransport,
    pub config: Config,

    /// App ID -> App
    pub apps: HashMap<String, App>,
    /// ClientId -> ClientSession
    pub sessions: HashMap<ClientId, ClientSession>,
    /// ClientId -> (App ID, Room ID)
    pub(crate) client_to_room: HashMap<ClientId, (String, String)>,
}

impl GameServer {
    pub fn new(transport: RenetTransport, config: Config) -> Self {
        Self {
            transport,
            config,
            apps: HashMap::new(),
            sessions: HashMap::new(),
            client_to_room: HashMap::new(),
        }
    }

    pub async fn run(&mut self, tick_rate: Duration) -> Result<(), Box<dyn Error>> {
        loop {
            self.transport.update(tick_rate)?;

            let packets = self.transport.recv_packets();
            let events = self.transport.recv_events();

            for packet in packets {
                self.handle_packet(packet);
            }
            for event in events {
                self.handle_event(event)
            }

            sleep(tick_rate).await;
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        match PacketType::from_bytes(&packet.data) {
            Ok(PacketType::Authenticate { app_id }) => {
                self.authenticate_client(packet.client_id, app_id);
            }
            Ok(packet_type) => {
                if !self.sessions.contains_key(&packet.client_id) {
                    self.send_packet(
                        packet.client_id,
                        PacketType::Error {
                            error_code: 0,
                            error_message: "Unauthorized".to_string(),
                        },
                        Channel::Reliable,
                    );
                    return;
                }
                self.handle_authorized_packet(packet.client_id, packet_type, packet.channel);
            }
            _ => {
                warn!("Unexpected packet type from {}", packet.client_id);
            }
        }
    }

    fn handle_authorized_packet(&mut self, client_id: ClientId, packet_type: PacketType, channel: Channel) {
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
                );
                return;
            }
        };

        match packet_type {
            PacketType::CreateRoom => {
                self.create_room(client_id, session_app_id.clone());
            }
            PacketType::JoinRoom { room_id } => {
                self.join_room(client_id, session_app_id.clone(), room_id)
            }
            PacketType::GameData { data, from_peer } => {
                self.route_game_data(client_id, from_peer, data, channel);
            }
            _ => warn!("Unexpected authorized packet"),
        }
    }

    fn handle_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                println!("Client connected: {}", client_id);
            }
            ServerEvent::ClientDisconnected { client_id, reason } => {
                println!("Client disconnected: {} ({:?})", client_id, reason);
                self.handle_disconnect(client_id);
            }
        }
    }

    pub fn send_packet(&mut self, target_client: ClientId, packet_type: PacketType, channel: Channel) {
        self.transport.send(
            target_client,
            packet_type.to_bytes(),
            channel,
        )
    }

    pub fn force_disconnect(&mut self, target_client: ClientId) {
        self.transport.disconnect_client(target_client);
    }

    /// Handlers

    fn authenticate_client(&mut self, sender_id: ClientId, app_id: String) {
        if !self.config.app_whitelist.is_empty() && !self.config.app_whitelist.contains(&app_id) {
            self.send_packet(
                sender_id,
                PacketType::Error {
                    error_code: 401,
                    error_message: "Unauthorized".into(),
                },
                Channel::Reliable,
            );

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
        );
    }

    fn create_room(&mut self, sender_id: ClientId, app_id: String) {
        let app = self.apps.get_mut(&app_id).expect("App exists");
        let mut room = Room::new(sender_id.to_string(), sender_id);
        let peer_id = room.add_peer(sender_id);
        let room_id = room.id.clone();

        app.add_room(room);
        self.client_to_room.insert(sender_id, (app_id, room_id.clone()));

        self.send_packet(
            sender_id,
            PacketType::ConnectedToRoom {
                room_id,
                peer_id
            },
            Channel::Reliable,
        )
    }

    fn join_room(&mut self, sender_id: ClientId, app_id: String, room_id: String) {
        let app = self.apps.get_mut(&app_id).expect("App exists");
        let Some(room) = app.get_room(&room_id) else {
            self.send_packet(
                sender_id,
                PacketType::Error {
                    error_code: 404,
                    error_message: "Room not found".into(),
                },
                Channel::Reliable,
            );
            return;
        };

        let peer_id = room.add_peer(sender_id);
        let room_host = room.get_host();
        self.client_to_room.insert(sender_id, (app_id, room_id.clone()));

        // Tell the sender that they have connected
        self.send_packet(
            sender_id,
            PacketType::ConnectedToRoom {
                room_id: room_id.clone(),
                peer_id
            },
            Channel::Reliable,
        );

        // Alert the host
        self.send_packet(
            room_host,
            PacketType::PeerJoinedRoom {
                peer_id
            },
            Channel::Reliable,
        );
    }

    fn route_game_data(&mut self, sender_id: ClientId, target_peer: i32, data: Vec<u8>, channel: Channel) {
        let Some((app_id, room_id)) = self.client_to_room.get(&sender_id) else {
            warn!("Client {} tried to send game data but is not in a room", sender_id);
            return;
        };

        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("Client {} has invalid app_id in index", sender_id);
            return;
        };

        let Some(room) = app.get_room(room_id) else {
            warn!("Client {} has invalid room_id in index", sender_id);
            return;
        };

        let Some(sender_godot_id) = room.get_godot_id(sender_id) else {
            warn!("Client {} not found in their own room", sender_id);
            return;
        };

        let Some(target_renet_id) = room.get_renet_id(target_peer) else {
            warn!("Client {} not found in room {}", target_peer, room_id);
            return;
        };

        self.send_packet(
            target_renet_id,
            PacketType::GameData {
                from_peer: sender_godot_id,
                data,
            },
            channel,
        );
    }

    fn handle_disconnect(&mut self, client_id: ClientId) {
        let Some(session) = self.sessions.remove(&client_id) else {
            warn!("Client {} disconnected without authenticating", client_id);
            return;
        };

        let room_info = self.client_to_room.remove(&client_id);

        let Some((app_id, room_id)) = room_info else {
            return;
        };

        let app_opt = self.apps.get_mut(&app_id);
        if app_opt.is_none() {
            warn!("Client {} had invalid app_id on disconnect", client_id);
            return;
        }

        let app = app_opt.unwrap();

        let room_opt = app.get_room(&room_id);
        if room_opt.is_none() {
            warn!("Client {} had invalid room_id on disconnect", client_id);
            return;
        }

        let room = room_opt.unwrap();

        let host_id = room.get_host();
        let is_host = host_id == client_id;
        let peer_godot_id = match room.get_godot_id(client_id) {
            Some(id) => id,
            None => {
                warn!("Client {} not found in their room on disconnect", client_id);
                return;
            }
        };

        let other_peers: Vec<ClientId> = room
            .get_renet_ids()
            .into_iter()
            .filter(|&id| id != client_id)
            .collect();

        if is_host {
            for peer_id in &other_peers {
                self.send_packet(
                    *peer_id,
                    PacketType::ForceDisconnect,
                    Channel::Reliable,
                );
                self.client_to_room.remove(peer_id);
            }

            if let Some(app) = self.apps.get_mut(&app_id) {
                app.remove_room(&room_id);
            }
        } else {
            self.send_packet(
                host_id,
                PacketType::PeerLeftRoom { peer_id: peer_godot_id },
                Channel::Reliable,
            );

            if let Some(app) = self.apps.get_mut(&app_id) {
                if let Some(room) = app.get_room(&room_id) {
                    room.remove_peer(client_id);

                    if room.is_empty() {
                        app.remove_room(&room_id);
                    }
                }
            }
        }

        if let Some(app) = self.apps.get_mut(&app_id) {
            if app.get_rooms().is_empty() {
                self.apps.remove(&app_id);
            }
        }
    }
}
