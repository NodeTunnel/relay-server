use std::error::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    #[serde(rename = "collectionId")]
    pub collection_id: Option<String>,
    #[serde(rename = "collectionName")]
    pub collection_name: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub room_id: String,
    pub relay_address: String,
    pub game: Game,
}

#[derive(Debug, Deserialize)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub page: i32,
    #[serde(rename = "perPage")]
    pub per_page: i32,
    #[serde(rename = "totalItems")]
    pub total_items: i32,
    #[serde(rename = "totalPages")]
    pub total_pages: i32,
}

pub struct PocketBaseClient {
    pub base_url: String,
    client: Client
}

impl PocketBaseClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: Client::new(),
        }
    }

    pub async fn register_room(
        &self,
        room_id: &str,
        relay_address: &str,
        game_id: &str
    ) -> Result<String, Box<dyn Error>> {
        let url = format!(
            "{}/api/collections/rooms/records",
            self.base_url
        );

        let body = serde_json::json!({
        "room_id": room_id,
        "relay_address": relay_address,
        "game": game_id
        });

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(format!("Failed to register room: {}", error_text).into());
        }

        let result: serde_json::Value = response.json().await?;
        let record_id = result["id"].as_str()
            .ok_or("No id in response")?
            .to_string();

        Ok(record_id)
    }
}