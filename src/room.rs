use std::collections::HashMap;
use renet::ClientId;

#[derive(Debug)]
pub struct Room {
    pub id: String,
    host_id: ClientId,
    godot_to_client: HashMap<i32, ClientId>,
    client_to_godot: HashMap<ClientId, i32>,
}

impl Room {
    pub fn new(id: String, host_id: ClientId) -> Self {
        Self {
            id,
            host_id,
            godot_to_client: HashMap::new(),
            client_to_godot: HashMap::new(),
        }
    }

    pub fn add_peer(&mut self, godot_pid: i32, client_id: ClientId) {
        self.godot_to_client.insert(godot_pid, client_id);
        self.client_to_godot.insert(client_id, godot_pid);
    }
    
    pub fn get_host(&self) -> ClientId {
        self.host_id
    }

    pub fn get_godot_id(&self, client_id: ClientId) -> Option<i32> {
        self.client_to_godot.get(&client_id).copied()
    }

    pub fn get_renet_id(&self, godot_id: i32) -> Option<ClientId> {
        self.godot_to_client.get(&godot_id).copied()
    }

    pub fn contains_renet_id(&self, renet_id: ClientId) -> bool {
        self.client_to_godot.contains_key(&renet_id)
    }

    pub fn remove_peer(&mut self, renet_id: ClientId) -> Option<i32> {
        if let Some(godot_id) = self.client_to_godot.remove(&renet_id) {
            self.godot_to_client.remove(&godot_id);
            Some(godot_id)
        } else {
            None
        }
    }
}