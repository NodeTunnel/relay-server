use tracing::warn;
use crate::protocol::packet::{Packet, RoomInfo};
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::protocol::serialize::MIN_ROOM_INFO_BYTES;
use crate::relay::rooms::Room;
use crate::udp::common::TransferChannel;
use crate::udp::paper_interface::PaperInterface;

// Fits the most rooms the player cap allows, with a short name in each.
const ROOM_LIST_BUDGET: usize = 4500;

/// Picks the rooms that fit in `ROOM_LIST_BUDGET`, and counts the ones that did not.
fn fit_rooms(rooms: impl Iterator<Item = RoomInfo>) -> (Vec<RoomInfo>, usize) {
    let mut fitted = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;

    for info in rooms {
        let cost = MIN_ROOM_INFO_BYTES + info.join_code.len() + info.metadata.len();

        if used + cost > ROOM_LIST_BUDGET {
            omitted += 1;
            continue;
        }

        used += cost;
        fitted.push(info);
    }

    (fitted, omitted)
}

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

        // Sorted so the same rooms drop off each time, not a random few.
        let mut infos: Vec<RoomInfo> = app.rooms.iter()
            .filter(|room| room.is_public)
            .map(Room::to_info)
            .collect();
        infos.sort_by(|a, b| a.join_code.cmp(&b.join_code));

        let (public_rooms, omitted) = fit_rooms(infos.into_iter());

        if omitted > 0 {
            warn!(
                "room list for app {} truncated: {} rooms sent, {} omitted",
                app_id, public_rooms.len(), omitted
            );
        }

        self.send_packet(
            target,
            &Packet::GetRooms {
                rooms: public_rooms
            },
            TransferChannel::Reliable,
        ).await;
    }

    pub async fn update_room(&mut self, sender_id: u64, app_id: u64, room_id: u64, metadata: &str) {
        enum Outcome {
            Updated,
            NoRoom,
            NotHost,
        }

        let outcome = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("attempted to update a room for a missing app: {}", app_id);
                return;
            };

            match app.rooms.get_mut(room_id) {
                None => Outcome::NoRoom,
                Some(room) if room.get_host() != sender_id => Outcome::NotHost,
                Some(room) => {
                    room.metadata = metadata.to_string();
                    Outcome::Updated
                }
            }
        };

        match outcome {
            Outcome::Updated => {}
            Outcome::NoRoom => self.send_err(sender_id, "Room not found").await,
            Outcome::NotHost => {
                warn!("non-host {} attempted to update room {}", sender_id, room_id);
                self.send_err(sender_id, "Only the room host can update the room").await;
            }
        }
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

            let Some(room) = app.rooms.get_by_jc_mut(room_id) else {
                self.send_err(sender_id, "Room not found").await;
                return;
            };

            if !room.add_pending_join(sender_id) {
                warn!(
                    "room {} has too many pending joins; dropping request from {}",
                    room_id, sender_id
                );
                self.send_err(sender_id, "Room is busy, please retry").await;
                return;
            }

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

    pub(crate) async fn recv_join_res(&mut self, sender_id: u64, app_id: u64, target_id: u64, room_id: u64, allowed: &bool) {
        let admitted = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("join response for a missing app: {}", app_id);
                return;
            };

            let Some(room) = app.rooms.get_mut(room_id) else {
                warn!("join response for a missing room: {}", room_id);
                return;
            };

            if room.get_host() != sender_id {
                warn!(
                    "non-host {} attempted to answer a join request for room {}",
                    sender_id, room_id
                );
                return;
            }

            if !room.take_pending_join(target_id) {
                warn!(
                    "host {} answered an unsolicited join request for {}",
                    sender_id, target_id
                );
                return;
            }

            if !*allowed {
                None
            } else if !matches!(
                self.clients.get(target_id).map(|c| &c.state),
                Some(ClientState::Authenticated { app_id: a }) if *a == app_id
            ) {
                warn!("join response for client {} in an ineligible state", target_id);
                return;
            } else {
                let peer_id = room.add_peer(target_id);
                Some((peer_id, room.get_host(), room.join_code.clone()))
            }
        };

        let Some((peer_id, host_id, join_code)) = admitted else {
            self.send_err(target_id, "Room host denied entry").await;
            return;
        };

        if let Some(client) = self.clients.get_mut(target_id) {
            client.state = ClientState::InRoom { app_id, room_id };
        }

        self.send_packet(
            target_id,
            &Packet::ConnectedToRoom {
                room_id: join_code,
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
