use std::error::Error;
use reqwest::{Client, StatusCode};
use serde::Serialize;

pub struct RegistryClient {
    client: Client,
    base_url: String,
    relay_id: String,
    api_key: String,
}

#[derive(Serialize)]
struct CreateRoomRequest {
    room_id: String,
    app_id: String,
}

impl RegistryClient {
    pub fn new(base_url: String, relay_id: String, api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            relay_id,
            api_key,
        }
    }

    pub async fn register_room(&self, room_id: &str, app_id: &str) -> Result<(), reqwest::Error> {
        let url = format!("{}/api/collections/rooms/records", self.base_url);

        let body = CreateRoomRequest {
            room_id: room_id.to_string(),
            app_id: app_id.to_string(),
        };

        let response = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?;

        if response.status() != StatusCode::OK {
            println!("Failed to register room! Response: {} - {:?}", response.status(), response.text().await);
        }

        Ok(())
    }

    pub async fn deregister_room(&self, room_id: &str) -> Result<(), Box<dyn Error>> {
        let url = format!(
            "{}/api/collections/rooms/records?filter=(room_id='{}')",
            self.base_url, room_id
        );

        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        let status = response.status();

        if status != StatusCode::OK {
            println!("Failed to de-register room! Response: {} - {:?}", status, response.text().await);
            return Err("Failed to de-register room".into())
        }

        let json: serde_json::Value = response.json().await?;

        if let Some(items) = json["items"].as_array() {
            if let Some(first) = items.first() {
                if let Some(id) = first["id"].as_str() {
                    let delete_url = format!("{}/api/collections/rooms/records/{}", self.base_url, id);
                    self.client
                        .delete(&delete_url)
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .send()
                        .await?;
                }
            }
        }

        Ok(())
    }
}
