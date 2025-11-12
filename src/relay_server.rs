use crate::packet_type::PacketType;
use crate::renet_connection::{Packet, RenetConnection};
use crate::room::Room;
use renet::{ClientId, DefaultChannel, ServerEvent};
use std::collections::HashMap;
use std::error::Error;
use std::net::SocketAddr;
use std::time::{Duration};
use tokio::time::sleep;
use crate::CONFIG;

struct ClientSession {
    app_id: String,
    renet_id: String,
}

struct App {
    id: String,
    rooms: HashMap<String, Room>,
}

impl App {
    fn new(id: String) -> Self {
        Self {
            id,
            rooms: HashMap::new(),
        }
    }

    fn add_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    fn get_room(&mut self, id: &str) -> Option<&mut Room> {
        self.rooms.get_mut(id)
    }

    fn get_rooms(&mut self) -> &mut HashMap<String, Room> {
        &mut self.rooms
    }

    fn remove_room(&mut self, id: &str) -> Option<Room> {
        self.rooms.remove(id)
    }
}

pub struct RelayServer {
    renet_connection: RenetConnection,
    app_sessions: HashMap<String, App>,
    client_sessions: HashMap<ClientId, ClientSession>,
}

impl RelayServer {
    pub fn new(addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            renet_connection: RenetConnection::new(addr)?,
            client_sessions: HashMap::new(),
            app_sessions: HashMap::new(),
        })
    }

    pub async fn run(&mut self) -> Result<(), Box<dyn Error>> {
        loop {
            self.update().await?;
            sleep(Duration::from_millis(16)).await;
        }
    }

    async fn update(&mut self) -> Result<(), Box<dyn Error>> {
        let packets = self.renet_connection.receive_packets()?;
        let events = self.renet_connection.receive_events()?;

        for packet in packets {
            self.process_packet(packet);
        }
        
        for event in events {
            self.process_event(event)
        }

        Ok(())
    }

    fn process_packet(&mut self, packet: Packet) {
        if let Ok(packet_type) = PacketType::from_bytes(packet.data) {
            match packet_type {
                PacketType::CreateRoom => {
                    self.handle_create_room(packet.renet_id);
                }
                PacketType::JoinRoom(room_id) => {
                    self.handle_join_room(packet.renet_id, room_id);
                }
                PacketType::GameData(target_id, data) => {
                    self.handle_game_data(packet.renet_id, target_id, data, packet.channel);
                }
                PacketType::Authenticate(app_id) => {
                    self.handle_authenticate(packet.renet_id, app_id);
                }
                _ => {}
            }
        } else {
            println!("Invalid packet from client {}", packet.renet_id);
        }
    }

    fn process_event(&mut self, server_event: ServerEvent) {
        match server_event {
            ServerEvent::ClientDisconnected { client_id, reason } => {
                println!("{} disconnected: {}", client_id, reason);
                let mut rooms_to_remove = Vec::new();

                let Some(client_session) = self.client_sessions.get_mut(&client_id) else {
                    println!("Client {} attempted to disconnect without authenticating", client_id);
                    return;
                };

                let Some(app) = self.app_sessions.get_mut(&client_session.app_id) else {
                    println!("Client {} attempted to disconnect with an invalid app id", client_id);
                    return;
                };

                for (room_id, room) in app.get_rooms() {
                    if !room.contains_renet_id(client_id) {
                        continue;
                    }

                    let godot_id = room.get_godot_id(client_id).unwrap();
                    let is_host = room.get_host() == client_id;

                    if is_host {
                        let peer_ids: Vec<ClientId> = room.get_renet_ids()
                            .filter(|&renet_id| renet_id != client_id)
                            .collect();

                        for other_renet_id in peer_ids {
                            self.renet_connection.send(
                                other_renet_id,
                                PacketType::ForceDisconnect().to_bytes(),
                                DefaultChannel::ReliableOrdered,
                            );
                        }

                        rooms_to_remove.push(room_id.clone());
                    } else {
                        self.renet_connection.send(
                            room.get_host(),
                            PacketType::PeerLeftRoom(godot_id).to_bytes(),
                            DefaultChannel::ReliableOrdered,
                        );

                        room.remove_peer(client_id);

                        if room.is_empty() {
                            rooms_to_remove.push(room_id.clone());
                        }
                    }

                    break;
                }

                for room_id in rooms_to_remove {
                    println!("Destroying room {}", room_id);
                    app.remove_room(&room_id);
                }
            }
            _ => {}
        }
    }

    fn handle_authenticate(&mut self, client_id: ClientId, app_id: String) {
        let cfg = CONFIG.get().unwrap();

        if !cfg.server.app_whitelist.is_empty() && !cfg.server.app_whitelist.contains(&app_id) {
            self.renet_connection.send(
                client_id,
                PacketType::ForceDisconnect().to_bytes(),
                DefaultChannel::ReliableOrdered,
            );
            return;
        }

        if !self.app_sessions.contains_key(&app_id) {
            self.app_sessions.insert(app_id.clone(), App::new(app_id.clone()));
        }

        self.client_sessions.insert(client_id, ClientSession {
            app_id: app_id.clone(),
            renet_id: client_id.to_string(),
        });

        self.renet_connection.send(
            client_id,
            PacketType::ClientAuthenticated().to_bytes(),
            DefaultChannel::ReliableOrdered,
        );
    }

    // TODO: Unauthorized packet
    fn handle_create_room(&mut self, client_id: ClientId) {
        println!("Client {} creating room", client_id);

        let Some(client_session) = self.client_sessions.get_mut(&client_id) else {
            println!("Client {} attempted to create a room without authenticating", client_id);
            return;
        };

        let Some(app) = self.app_sessions.get_mut(&client_session.app_id) else {
            println!("Client {} attempted to create a room with an invalid app id", client_id);
            return;
        };

        let mut room = Room::new(client_id.to_string(), client_id);
        room.add_peer(client_id);

        self.renet_connection.send(
            client_id,
            PacketType::ConnectedToRoom(room.id.clone(), 1).to_bytes(),
            DefaultChannel::ReliableOrdered
        );

        app.add_room(room);
    }

    fn handle_join_room(&mut self, client_id: ClientId, room_id: String) {
        println!("Client {} joining room: {}", client_id, room_id);

        let Some(client_session) = self.client_sessions.get_mut(&client_id) else {
            println!("Client {} attempted to join a room without authenticating", client_id);
            return;
        };

        let Some(app) = self.app_sessions.get_mut(&client_session.app_id) else {
            println!("Client {} attempted to join a room with an invalid app id", client_id);
            return;
        };

        if let Some(room) = app.get_room(&room_id) {
            let godot_pid = room.add_peer(client_id);

            self.renet_connection.send(
                client_id,
                PacketType::ConnectedToRoom(room_id, godot_pid).to_bytes(),
                DefaultChannel::ReliableOrdered
            );

            self.renet_connection.send(
                room.get_host(),
                PacketType::PeerJoinedRoom(godot_pid).to_bytes(),
                DefaultChannel::ReliableOrdered
            );
        } else {
            println!("Client attempted to join an invalid room")
        }
    }

    fn handle_game_data(&mut self, client_id: ClientId, target_id: i32, original_data: Vec<u8>, channel: DefaultChannel) {
        let Some(client_session) = self.client_sessions.get_mut(&client_id) else {
            println!("Client {} attempted to send game data without authenticating", client_id);
            return;
        };

        let Some(app) = self.app_sessions.get_mut(&client_session.app_id) else {
            println!("Client {} attempted to send game data with an invalid app id", client_id);
            return;
        };

        for (_room_id, room) in &app.rooms {
            if let Some(sender_godot_id) = room.get_godot_id(client_id) {
                if let Some(target_renet_id) = room.get_renet_id(target_id) {
                    let packet = PacketType::GameData(sender_godot_id, original_data).to_bytes();

                    self.renet_connection.send(
                        target_renet_id,
                        packet,
                        channel
                    );
                }
                break;
            }
        }
    }
}