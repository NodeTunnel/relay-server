use crate::packet_type::PacketType;
use crate::renet_connection::{Packet, RenetConnection};
use crate::room::Room;
use renet::{ClientId, DefaultChannel, ServerEvent};
use std::collections::HashMap;
use std::error::Error;
use std::net::SocketAddr;
use std::time::{Duration};
use tokio::time::sleep;

pub struct RelayServer {
    renet_connection: RenetConnection,
    rooms: HashMap<String, Room>,
}

impl RelayServer {
    pub fn new(addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            renet_connection: RenetConnection::new(addr)?,
            rooms: HashMap::new(),
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
                    self.handle_game_data_raw(packet.renet_id, target_id, data, packet.channel);
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
                for (_room_id, room) in &mut self.rooms {
                    println!("{} disconnected: {}", client_id, reason);
                    room.remove_peer(client_id);
                }
            }
            _ => {}
        }
    }

    fn handle_create_room(&mut self, client_id: ClientId) {
        println!("Client {} creating room", client_id);

        let mut room = Room::new(client_id.to_string(), client_id);
        room.add_peer(1, client_id);

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
            room.add_peer(2, client_id);

            self.renet_connection.send(
                client_id,
                PacketType::ConnectedToRoom(room_id, 2).to_bytes(),
                DefaultChannel::ReliableOrdered
            );

            self.renet_connection.send(
                room.get_host(),
                PacketType::PeerJoinedRoom(2).to_bytes(),
                DefaultChannel::ReliableOrdered
            );
        } else {
            println!("Client attempted to join an invalid room")
        }
    }

    fn handle_game_data_raw(&mut self, client_id: ClientId, target_id: i32, original_data: Vec<u8>, channel: DefaultChannel) {
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