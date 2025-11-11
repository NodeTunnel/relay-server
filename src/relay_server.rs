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

pub struct RelayServer {
    renet_connection: RenetConnection,
    rooms: HashMap<String, Room>,
    clients: HashMap<ClientId, ClientSession>,
}

impl RelayServer {
    pub fn new(addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            renet_connection: RenetConnection::new(addr)?,
            rooms: HashMap::new(),
            clients: HashMap::new(),
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

                for (room_id, room) in &mut self.rooms {
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
                    self.rooms.remove(&room_id);
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

        self.clients.insert(client_id, ClientSession {
            app_id: app_id.clone(),
            renet_id: client_id.to_string(),
        });

        self.renet_connection.send(
            client_id,
            PacketType::ClientAuthenticated().to_bytes(),
            DefaultChannel::ReliableOrdered,
        );
    }

    fn handle_create_room(&mut self, client_id: ClientId) {
        println!("Client {} creating room", client_id);

        let mut room = Room::new(client_id.to_string(), client_id);
        room.add_peer(client_id);

        self.renet_connection.send(
            client_id,
            PacketType::ConnectedToRoom(room.id.clone(), 1).to_bytes(),
            DefaultChannel::ReliableOrdered
        );

        self.rooms.insert(client_id.to_string(), room);
    }

    fn handle_join_room(&mut self, client_id: ClientId, room_id: String) {
        println!("Client {} joining room: {}", client_id, room_id);

        if let Some(room) = self.rooms.get_mut(&room_id) {
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
        for (_room_id, room) in &self.rooms {
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