use std::collections::{HashMap, HashSet};
use rand::{rng, Rng};
use crate::protocol::packet::RoomInfo;

const ID_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ123456789";
const ID_LENGTH: usize = 5;

pub struct RoomIds {
    used: HashSet<String>
}

impl RoomIds {
    pub fn new() -> Self {
        Self { used: HashSet::new() }
    }

    pub fn generate(&mut self) -> String {
        loop {
            let mut rng = rng();
            let id: String = (0..ID_LENGTH)
                .map(|_| {
                    let idx = rng.random_range(0..ID_CHARS.len());
                    ID_CHARS[idx] as char
                })
                .collect();

            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }

    pub fn free(&mut self, id: &str) {
        self.used.remove(id);
    }
}

#[derive(Debug)]
pub struct Room {
    pub id: String,
    pub is_public: bool,
    pub name: String,
    pub max_players: i32,
    host_id: u64,
    godot_to_client: HashMap<i32, u64>,
    client_to_godot: HashMap<u64, i32>,
    next_godot_id: i32,
}

impl Room {
    pub fn new(id: String, host_id: u64, is_public: bool, name: String, max_players: i32) -> Self {
        Self {
            id,
            is_public,
            name,
            max_players,
            host_id,
            client_to_godot: HashMap::new(),
            godot_to_client: HashMap::new(),
            next_godot_id: 1,
        }
    }

    pub fn to_info(&self) -> RoomInfo {
        RoomInfo {
            id: self.id.clone(),
            players: self.get_renet_ids().len() as i32,
            name: self.name.clone(),
            max_players: self.max_players.clone(),
        }
    }

    pub fn add_peer(&mut self, client_id: u64) -> i32 {
        let godot_pid = self.next_godot_id;
        self.client_to_godot.insert(client_id, godot_pid);
        self.godot_to_client.insert(godot_pid, client_id);
        self.next_godot_id += 1;

        godot_pid
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

    pub fn get_player_count(&self) -> i32 {
        self.godot_to_client.len() as i32
    }

    pub fn is_empty(&self) -> bool {
        self.client_to_godot.is_empty()
    }

    pub fn is_full(&self) -> bool {
        if self.max_players == -1 {
            return false;
        }

        self.get_player_count() >= self.max_players
    }
}