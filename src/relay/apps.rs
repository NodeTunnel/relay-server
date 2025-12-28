use std::collections::HashMap;
use crate::relay::rooms::Rooms;

pub struct App {
    pub id: u64,
    pub token: String,
    pub rooms: Rooms,
}

impl App {
    pub fn new(id: u64, token: String) -> Self {
        Self {
            id,
            token,
            rooms: Rooms::new()
        }
    }
}

#[derive(Default)]
pub struct Apps {
    by_id: HashMap<u64, App>,
    token_to_id: HashMap<String, u64>,
    next_id: u64,
}

impl Apps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&mut self, token: String) -> u64 {
        let app_id = self.next_id;
        self.next_id += 1;

        let app = App::new(app_id, token.clone());
        self.by_id.insert(app_id, app);
        self.token_to_id.insert(token, app_id);

        app_id
    }
    
    pub fn iter(&self) -> impl Iterator<Item = &App> {
        self.by_id.values()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut App> {
        self.by_id.values_mut()
    }

    pub fn get(&self, id: u64) -> Option<&App> {
        self.by_id.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut App> {
        self.by_id.get_mut(&id)
    }

    pub fn get_by_token(&self, token: &str) -> Option<&App> {
        let id = self.token_to_id.get(token)?;
        self.by_id.get(id)
    }

    pub fn get_by_token_mut(&mut self, token: &str) -> Option<&mut App> {
        let id = self.token_to_id.get(token)?;
        self.by_id.get_mut(id)
    }
}
