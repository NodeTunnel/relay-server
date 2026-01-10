use tracing::warn;
use crate::protocol::packet::Packet;
use crate::relay::apps::Apps;
use crate::udp::common::TransferChannel;
use crate::udp::paper_interface::PaperInterface;

pub struct GameDataHandler<'a> {
    udp: &'a mut PaperInterface,
    apps: &'a mut Apps,
}

impl<'a> GameDataHandler<'a> {
    pub fn new(
        udp: &'a mut PaperInterface,
        apps: &'a mut Apps
    ) -> Self {
        Self {
            udp,
            apps,
        }
    }

    pub async fn route_game_data(&mut self, sender_id: u64, client_app_id: u64, client_room_id: u64, target_peer: i32, data: &[u8], channel: &TransferChannel) {
        let Some(app) = self.apps.get_mut(client_app_id) else {
            warn!("{} has invalid app_id in index", sender_id);
            return;
        };

        let Some(room) = app.rooms.get(client_room_id) else {
            warn!("{} has invalid room_id in index", sender_id);
            return;
        };

        let Some(sender_godot_id) = room.client_to_gd(sender_id) else {
            warn!("{} not found in their own room", sender_id);
            return;
        };

        let Some(target_renet_id) = room.gd_to_client(target_peer) else {
            return;
        };

        self.send_packet(
            target_renet_id,
            &Packet::GameData {
                from_peer: sender_godot_id,
                data: data.to_vec(),
            },
            *channel,
        ).await;
    }

    // TODO: get rid of duplicates
    async fn send_packet(&mut self, target: u64, packet: &Packet, channel: TransferChannel) {
        if let Err(e) = self.udp.send(target, packet.to_bytes(), channel).await {
            warn!("failed to send packet: {}", e);
        }
    }
}
