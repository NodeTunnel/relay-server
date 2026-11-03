use tracing::{info, warn};
use crate::protocol::packet::Packet;
use crate::relay::apps::Apps;
use crate::relay::clients::{ClientState, Clients};
use crate::relay::handlers::room::RoomHandler;
use crate::udp::common::{TrafficClass, TransferChannel};
use crate::udp::paper_interface::PaperInterface;

struct DisconnectInfo {
    is_host: bool,
    godot_id: i32,
    other_peers: Vec<u64>,
}

pub struct DisconnectHandler<'a> {
    udp: &'a mut PaperInterface,
    clients: &'a mut Clients,
    apps: &'a mut Apps,
}

impl<'a> DisconnectHandler<'a> {
    pub fn new(
        udp: &'a mut PaperInterface,
        clients: &'a mut Clients,
        apps: &'a mut Apps,
    ) -> Self {
        Self {
            udp,
            clients,
            apps,
        }
    }

    pub async fn handle_disconnect(&mut self, client_id: u64) {
        let Some(client) = self.clients.remove(client_id) else {
            warn!("unregistered client disconnected");
            return;
        };

        if let ClientState::InRoom { app_id, room_id } = client.state {
            self.handle_room_disconnect(client_id, app_id, room_id).await;
        }
    }

    pub async fn handle_leave(&mut self, client_id: u64) {
        self.udp.remove_client(&client_id);
        self.handle_disconnect(client_id).await;
    }

    pub async fn kick_peer(&mut self, sender_id: u64, app_id: u64, room_id: u64, peer_id: i32) {
        let Some(room) = self.apps.get_mut(app_id).and_then(|app| app.rooms.get(room_id)) else {
            warn!("{} had invalid room on kick", sender_id);
            return;
        };

        if room.get_host() != sender_id {
            warn!("{} tried to kick a peer without being the host", sender_id);
            return;
        }

        let Some(target) = room.gd_to_client(peer_id) else {
            return;
        };

        if target == sender_id {
            return;
        }

        self.drop_client(target, "Kicked by host").await;
    }

    pub async fn drop_client(&mut self, client_id: u64, reason: &str) {
        self.handle_disconnect(client_id).await;
        self.notify_and_disconnect(client_id, reason).await;
    }

    pub async fn notify_and_disconnect(&mut self, client_id: u64, reason: &str) {
        self.send_err(client_id, reason).await;
        self.force_disconnect(client_id).await;
    }

    async fn handle_room_disconnect(&mut self, sender_id: u64, app_id: u64, room_id: u64) {
        let disconnect_info = {
            let Some(app) = self.apps.get_mut(app_id) else {
                warn!("{} had invalid app_id on disconnect", sender_id);
                return;
            };

            let Some(room) = app.rooms.get(room_id) else {
                warn!("{} had invalid room_id on disconnect", sender_id);
                return;
            };

            let Some(godot_id) = room.client_to_gd(sender_id) else {
                warn!("{} not found in their room on disconnect", sender_id);
                return;
            };

            DisconnectInfo {
                is_host: room.get_host() == sender_id,
                godot_id,
                other_peers: room.get_clients()
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

    async fn handle_host_disconnect(&mut self, app_id: u64, room_id: u64, peers_to_kick: Vec<u64>) {
        info!("host disconnected");
        RoomHandler::new(
            self.udp,
            self.apps,
            self.clients,
        ).remove_room(app_id, room_id);

        for peer_id in peers_to_kick {
            self.clients.remove(peer_id);
            self.notify_and_disconnect(peer_id, "Host left the room").await;
        }
    }

    async fn handle_peer_disconnect(&mut self, app_id: u64, room_id: u64, client_id: u64, peer_godot_id: i32, other_peers: Vec<u64>) {
        info!("peer disconnected");
        if let Some(app) = self.apps.get_mut(app_id) {
            if let Some(room) = app.rooms.get_mut(room_id) {
                room.remove_peer(client_id);
            }
        }

        for peer_id in other_peers {
            self.send_packet(peer_id, &Packet::PeerLeftRoom { peer_id: peer_godot_id }, TransferChannel::Reliable).await;
        }
    }

    pub async fn force_disconnect(&mut self, target_client: u64) {
        self.send_packet(
            target_client,
            &Packet::ForceDisconnect,
            TransferChannel::Reliable
        ).await;
        self.udp.remove_client(&target_client);
    }

    async fn send_err(&mut self, target_client: u64, msg: &str) {
        self.send_packet(
            target_client,
            &Packet::Error {
                error_code: 401,
                error_message: msg.to_string(),
            },
            TransferChannel::Reliable,
        ).await;
    }

    async fn send_packet(&mut self, target_client: u64, packet: &Packet, channel: TransferChannel) {
        match self.udp.send(
            target_client,
            packet.to_bytes(),
            channel,
            TrafficClass::Control,
        ).await {
            Ok(()) => {},
            Err(e) => warn!("failed to send packet: {}", e)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use crate::limits::TrafficLimits;
    use super::*;

    const HOST: u64 = 1;
    const PEER: u64 = 2;
    const OTHER: u64 = 3;
    const HOST_GD: i32 = 1;
    const PEER_GD: i32 = 2;
    const OTHER_GD: i32 = 3;

    struct Fixture {
        udp: PaperInterface,
        clients: Clients,
        apps: Apps,
        app_id: u64,
        room_id: u64,
    }

    impl Fixture {
        async fn new() -> Self {
            let udp = PaperInterface::new("127.0.0.1:0".parse().unwrap(), TrafficLimits {
                client_bytes_per_sec: 65536,
                global_bytes_per_sec: 2_000_000,
                daily_bytes: 5_000_000_000,
                daily_control_bytes: 250_000_000,
                interval: Duration::from_millis(250),
                max_session_bytes: 2_000_000_000,
                max_session_duration: Duration::from_secs(60),
            }).await.unwrap();

            let mut apps = Apps::new();
            let mut clients = Clients::new();
            let app_id = apps.create("app".to_string());
            let room = apps.get_mut(app_id).unwrap().rooms.create(HOST, false, String::new());
            let room_id = room.id;

            for id in [HOST, PEER, OTHER] {
                room.add_peer(id);
                clients.create(id);
                clients.get_mut(id).unwrap().state = ClientState::InRoom { app_id, room_id };
            }

            Self { udp, clients, apps, app_id, room_id }
        }

        fn handler(&mut self) -> DisconnectHandler<'_> {
            DisconnectHandler::new(&mut self.udp, &mut self.clients, &mut self.apps)
        }

        fn room_exists(&mut self) -> bool {
            let room_id = self.room_id;
            self.apps.get_mut(self.app_id).unwrap().rooms.get(room_id).is_some()
        }

        fn gd_in_room(&mut self, godot_id: i32) -> bool {
            let room_id = self.room_id;
            self.apps.get_mut(self.app_id).unwrap().rooms.get(room_id).unwrap()
                .gd_to_client(godot_id).is_some()
        }
    }

    #[tokio::test]
    async fn the_host_can_kick_a_peer() {
        let mut f = Fixture::new().await;
        let (app_id, room_id) = (f.app_id, f.room_id);

        f.handler().kick_peer(HOST, app_id, room_id, PEER_GD).await;

        assert!(!f.gd_in_room(PEER_GD));
        assert!(f.clients.get(PEER).is_none());
        assert!(f.gd_in_room(OTHER_GD));
    }

    #[tokio::test]
    async fn a_non_host_cannot_kick() {
        let mut f = Fixture::new().await;
        let (app_id, room_id) = (f.app_id, f.room_id);

        f.handler().kick_peer(PEER, app_id, room_id, OTHER_GD).await;

        assert!(f.gd_in_room(OTHER_GD));
        assert!(f.clients.get(OTHER).is_some());
    }

    #[tokio::test]
    async fn the_host_cannot_kick_itself() {
        let mut f = Fixture::new().await;
        let (app_id, room_id) = (f.app_id, f.room_id);

        f.handler().kick_peer(HOST, app_id, room_id, HOST_GD).await;

        assert!(f.room_exists());
        assert!(f.clients.get(HOST).is_some());
    }
}
