use std::collections::{HashMap, HashSet};
use renet::ClientId;
use crate::transport::common::Channel;

#[derive(Debug)]
struct GamePacket {
    from_peer: ClientId,
    data: Vec<u8>,
    channel: Channel
}

#[derive(Debug)]
pub struct Room {
    pub id: String,
    host_id: ClientId,
    godot_to_client: HashMap<i32, ClientId>,
    client_to_godot: HashMap<ClientId, i32>,
    pending_clients: HashSet<ClientId>,
    pending_packets: HashMap<ClientId, Vec<GamePacket>>,
    next_godot_id: i32,
}

impl Room {
    pub fn new(id: String, host_id: ClientId) -> Self {
        Self {
            id,
            host_id,
            client_to_godot: HashMap::new(),
            godot_to_client: HashMap::new(),
            pending_clients: HashSet::new(),
            pending_packets: HashMap::new(),
            next_godot_id: 1,
        }
    }

    pub fn add_peer(&mut self, client_id: ClientId) -> i32 {
        let godot_pid = self.next_godot_id;
        self.client_to_godot.insert(client_id, godot_pid);
        self.godot_to_client.insert(godot_pid, client_id);
        self.next_godot_id += 1;

        godot_pid
    }

    pub fn get_peers(&self) -> &HashMap<ClientId, i32> {
        &self.client_to_godot
    }

    pub fn get_renet_ids(&self) -> Vec<ClientId> {
        self.client_to_godot.keys().copied().collect()
    }

    pub fn get_godot_ids(&self) -> Vec<i32> {
        self.godot_to_client.keys().copied().collect()
    }

    pub fn get_godot_id(&self, client_id: ClientId) -> Option<i32> {
        self.client_to_godot.get(&client_id).copied()
    }

    pub fn get_renet_id(&self, godot_id: i32) -> Option<ClientId> {
        self.godot_to_client.get(&godot_id).copied()
    }

    pub fn get_host(&self) -> ClientId {
        self.host_id
    }

    pub fn mark_pending(&mut self, renet_id: ClientId) {
        self.pending_clients.insert(renet_id);
    }

    pub fn mark_ready(&mut self, renet_id: ClientId) -> bool {
        self.pending_clients.remove(&renet_id)
    }

    pub fn is_pending(&self, renet_id: ClientId) -> bool {
        self.pending_clients.contains(&renet_id)
    }

    pub fn remove_peer(&mut self, renet_id: ClientId) {
        let Some(peer_id) = self.client_to_godot.remove(&renet_id) else {
            return;
        };

        self.godot_to_client.remove(&peer_id);
    }

    pub fn is_empty(&self) -> bool {
        self.client_to_godot.is_empty()
    }
}