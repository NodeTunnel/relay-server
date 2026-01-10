use tracing::warn;
use crate::protocol::packet::{Packet, RoomInfo};
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::udp::common::TransferChannel;
use crate::udp::paper_interface::PaperInterface;

pub struct RoomHandler<'a> {
    udp: &'a mut PaperInterface,
    apps: &'a mut Apps,
    clients: &'a mut Clients,
}

impl<'a> RoomHandler<'a> {
    pub fn new(
        udp: &'a mut PaperInterface,
        apps: &'a mut Apps,
        clients: &'a mut Clients,
    ) -> Self {
        Self {
            udp,
            apps,
            clients
        }
    }

    pub async fn create_room(&mut self, sender_id: u64, app_id: u64, is_public: bool, metadata: &str) {
        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("attempted to create a room for a missing app: {}", app_id);
            return;
        };

        let Some(client) = self.clients.get_mut(sender_id) else {
            warn!("attempted to create a room for a missing client: {}", sender_id);
            return;
        };

        let room = app.rooms.create(sender_id, is_public, metadata.to_string());
        let join_code = room.join_code.clone();
        let peer_id = room.add_peer(sender_id);

        client.state = ClientState::InRoom { app_id, room_id: room.id };

        self.send_packet(
            sender_id,
            &Packet::ConnectedToRoom {
                room_id: join_code,
                peer_id,
            },
            TransferChannel::Reliable,
        ).await;
    }

    pub async fn send_rooms(&mut self, target: u64, app_id: u64) {
        let Some(app) = self.apps.get_mut(app_id) else {
            warn!("attempted to list rooms for a missing app: {}", app_id);
            return;
        };

        let public_rooms: Vec<RoomInfo> = app.rooms.iter_mut()
            .filter(|room| room.is_public)
            .map(|room| room.to_info())
            .collect();

        self.send_packet(
            target,
            &Packet::GetRooms {
                rooms: public_rooms
            },
            TransferChannel::Reliable,
        ).await;
    }

    pub async fn update_room(&mut self, sender_id: u64, app_id: u64, room_id: u64, metadata: &str) {
        let app = self.apps.get_mut(app_id).expect("App exists");
        let Some(room) = app.rooms.get_mut(room_id) else {
            self.send_err(sender_id, "Room not found").await;
            return;
        };

        room.metadata = metadata.to_string();
    }

    pub fn remove_room(&mut self, app_id: u64, room_id: u64) {
        if let Some(app) = self.apps.get_mut(app_id) {
            app.rooms.remove(room_id);
        }
    }

    pub(crate) async fn recv_join_req(&mut self, sender_id: u64, app_id: u64, room_id: &str, metadata: &str) {
        let host_id = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("attempted to handle join request for a missing app: {}", app_id);
                return;
            };

            let Some(room) = app.rooms.get_by_jc(room_id) else {
                self.send_err(sender_id, "Room not found").await;
                return;
            };

            room.get_host()
        };

        self.send_packet(
            host_id,
            &Packet::PeerJoinAttempt {
                target_id: sender_id,
                metadata: metadata.to_string()
            },
            TransferChannel::Reliable
        ).await;
    }

    pub(crate) async fn recv_join_res(&mut self, app_id: u64, target_id: u64, room_id: u64, allowed: &bool) {
        if *allowed {
            let Some(client) = self.clients.get_mut(target_id) else {
                warn!("attempted to handle join response for a missing client: {}", target_id);
                return;
            };

            let (peer_id, host_id) = {
                let app = self.apps.get_mut(app_id).expect("App exists");
                let Some(room) = app.rooms.get_mut(room_id) else {
                    self.send_err(target_id, "Room not found").await;
                    return;
                };

                let peer_id = room.add_peer(target_id);
                let host_id = room.get_host();

                (peer_id, host_id)
            };

            client.state = ClientState::InRoom { app_id, room_id };

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

        self.send_err(target_id, "Room host denied entry").await;
    }

    async fn send_packet(&mut self, target: u64, packet: &Packet, channel: TransferChannel) {
        if let Err(e) = self.udp.send(target, packet.to_bytes(), channel).await {
            warn!("failed to send packet: {}", e);
        }
    }

    async fn send_err(&mut self, target: u64, msg: &str) {
        self.send_packet(
            target,
            &Packet::Error {
                error_code: 401,
                error_message: msg.to_string(),
            },
            TransferChannel::Reliable,
        )
            .await;
    }
}
