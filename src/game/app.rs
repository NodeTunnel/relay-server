use std::collections::HashMap;
use crate::game::room::Room;

pub struct App {
    pub(crate) id: String,
    rooms: HashMap<String, Room>,
}

impl App {
    pub fn new(id: String) -> Self {
        Self {
            id,
            rooms: HashMap::new(),
        }
    }

    pub fn add_room(&mut self, room: Room) {
        self.rooms.insert(room.id.clone(), room);
    }

    pub fn get_room(&mut self, room_id: &str) -> Option<&mut Room> {
        self.rooms.get_mut(room_id)
    }

    pub fn get_rooms(&mut self) -> &mut HashMap<String, Room> {
        &mut self.rooms
    }

    pub fn remove_room(&mut self, room_id: &str) -> Option<Room> {
        self.rooms.remove(room_id)
    }
}