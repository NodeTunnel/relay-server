use std::collections::{HashMap, HashSet};
use rand::{rng, Rng};
use crate::protocol::packet::RoomInfo;

const ID_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ123456789";
const ID_LENGTH: usize = 5;

#[derive(Default)]
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
    pub id: u64,
    pub join_code: String,
    pub is_public: bool,
    pub metadata: String,
    host_id: u64,
    client_to_godot: HashMap<u64, i32>,
    godot_to_client: HashMap<i32, u64>,
    next_godot_id: i32,
}

impl Room {
    pub fn new(id: u64, join_code: String, host_id: u64, is_public: bool, metadata: String) -> Self {
        Self {
            id,
            join_code,
            is_public,
            metadata,
            host_id,
            client_to_godot: HashMap::new(),
            godot_to_client: HashMap::new(),
            next_godot_id: 1,
        }
    }

    pub fn to_info(&self) -> RoomInfo {
        RoomInfo {
            join_code: self.join_code.clone(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn add_peer(&mut self, client_id: u64) -> i32 {
        let godot_pid = self.next_godot_id;
        self.client_to_godot.insert(client_id, godot_pid);
        self.godot_to_client.insert(godot_pid, client_id);
        self.next_godot_id += 1;

        godot_pid
    }

    pub fn get_clients(&self) -> Vec<u64> {
        self.client_to_godot.keys().copied().collect()
    }

    pub fn client_to_gd(&self, client_id: u64) -> Option<i32> {
        self.client_to_godot.get(&client_id).copied()
    }

    pub fn gd_to_client(&self, godot_id: i32) -> Option<u64> {
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
}

#[derive(Default)]
pub struct Rooms {
    by_id: HashMap<u64, Room>,
    jc_to_id: HashMap<String, u64>,
    next_id: u64,
    join_codes: RoomIds,
}

impl Rooms {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new room based on the given parameters.
    /// Returns a mutable reference to the new `Room`.
    pub fn create(&mut self, host_id: u64, is_public: bool, metadata: String) -> &mut Room {
        let room_id = self.next_id;
        self.next_id += 1;

        let join_code = self.join_codes.generate();
        let room = Room::new(room_id, join_code.clone(), host_id, is_public, metadata);
        self.jc_to_id.insert(join_code, room_id);
        self.by_id.entry(room_id).or_insert(room)
    }

    /// Gets an iterator for all `Room`'s stored.
    pub fn iter(&self) -> impl Iterator<Item = &Room> {
        self.by_id.values()
    }

    /// Gets an iterator for all `Room`'s stored.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Room> {
        self.by_id.values_mut()
    }

    /// Gets a reference to a room by an ID
    pub fn get(&self, id: u64) -> Option<&Room> {
        self.by_id.get(&id)
    }

    /// Gets a mutable reference to a room by an ID
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Room> {
        self.by_id.get_mut(&id)
    }

    /// Gets a reference to a room by a join code.
    /// Prefer `get` whenever possible as this requires 2 lookups.
    pub fn get_by_jc(&self, jc: &str) -> Option<&Room> {
        let id = self.jc_to_id.get(jc)?;
        self.by_id.get(id)
    }

    /// Gets a mutable reference to a room by a join code.
    /// Prefer `get_mut` whenever possible as this requires 2 lookups.
    pub fn get_by_jc_mut(&mut self, jc: &str) -> Option<&mut Room> {
        let id = self.jc_to_id.get(jc)?;
        self.by_id.get_mut(id)
    }

    /// Removes a room under an ID.
    /// Also frees the join code from the generator.
    pub fn remove(&mut self, id: u64) -> Option<Room> {
        let r = self.by_id.remove(&id)?;
        self.jc_to_id.remove(&r.join_code);
        self.join_codes.free(&r.join_code);
        Some(r)
    }
}
