use std::collections::{HashMap, HashSet};
use crate::transport::common::Channel;

#[derive(Debug)]
struct GamePacket {
    from_peer: u64,
    data: Vec<u8>,
    channel: Channel
}

#[derive(Debug)]
pub struct Room {
    pub id: String,
    host_id: u64,
    godot_to_client: HashMap<i32, u64>,
    client_to_godot: HashMap<u64, i32>,
    pending_clients: HashSet<u64>,
    pending_packets: HashMap<u64, Vec<GamePacket>>,
    next_godot_id: i32,
}

impl Room {
    pub fn new(id: String, host_id: u64) -> Self {
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

    pub fn add_peer(&mut self, client_id: u64) -> i32 {
        let godot_pid = self.next_godot_id;
        self.client_to_godot.insert(client_id, godot_pid);
        self.godot_to_client.insert(godot_pid, client_id);
        self.next_godot_id += 1;

        godot_pid
    }

    pub fn get_peers(&self) -> &HashMap<u64, i32> {
        &self.client_to_godot
    }

    pub fn get_renet_ids(&self) -> Vec<u64> {
        self.client_to_godot.keys().copied().collect()
    }

    pub fn get_godot_ids(&self) -> Vec<i32> {
        self.godot_to_client.keys().copied().collect()
    }

    pub fn get_godot_id(&self, client_id: u64) -> Option<i32> {
        self.client_to_godot.get(&client_id).copied()
    }

    pub fn get_renet_id(&self, godot_id: i32) -> Option<u64> {
        self.godot_to_client.get(&godot_id).copied()
    }

    pub fn get_host(&self) -> u64 {
        self.host_id
    }

    pub fn remove_peer(&mut self, renet_id: u64) {
        let Some(peer_id) = self.client_to_godot.remove(&renet_id) else {
            return;
        };

        self.godot_to_client.remove(&peer_id);
    }

    pub fn is_empty(&self) -> bool {
        self.client_to_godot.is_empty()
    }
}