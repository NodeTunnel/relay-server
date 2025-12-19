use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::http::error::HttpError;

#[derive(Deserialize)]
struct AuthResponse {
    token: String,
}

#[derive(Deserialize)]
struct ListResponse {
    items: Vec<serde_json::Value>,
}

pub struct HttpWrapper {
    client: Client,
    base_url: String,
    email: String,
    password: String,
    token: String,
}

impl HttpWrapper {
    pub async fn new(base_url: &str, email: &str, password: &str) -> Result<Self, HttpError> {
        let client = Client::new();
        let mut wrapper = Self {
            client,
            base_url: base_url.to_string(),
            email: email.to_string(),
            password: password.to_string(),
            token: String::new(),
        };
        wrapper.refresh_token().await?;
        Ok(wrapper)
    }

    async fn refresh_token(&mut self) -> Result<(), HttpError> {
        let res = self.client
            .post(format!("{}/api/collections/users/auth-with-password", self.base_url))
            .json(&serde_json::json!({
                "identity": self.email,
                "password": self.password,
            }))
            .send()
            .await?
            .error_for_status()?;

        self.token = res.json::<AuthResponse>().await?.token;
        Ok(())
    }

    async fn get(&mut self, path: &str) -> Result<reqwest::Response, HttpError> {
        let url = format!("{}{}", self.base_url, path);

        let res = self.client.get(&url).bearer_auth(&self.token).send().await?;

        if res.status().as_u16() == 401 {
            self.refresh_token().await?;
            return Ok(self.client.get(&url).bearer_auth(&self.token).send().await?);
        }

        Ok(res)
    }

    async fn delete(&mut self, path: &str) -> Result<reqwest::Response, HttpError> {
        let url = format!("{}{}", self.base_url, path);

        let res = self.client.delete(&url).bearer_auth(&self.token).send().await?;

        if res.status().as_u16() == 401 {
            self.refresh_token().await?;
            return Ok(self.client.delete(&url).bearer_auth(&self.token).send().await?);
        }

        Ok(res)
    }

    async fn post_json<B: Serialize + ?Sized>(
        &mut self,
        path: &str,
        body: &B,
    ) -> Result<reqwest::Response, HttpError> {
        let url = format!("{}{}", self.base_url, path);

        let res = self.client
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?;

        if res.status().as_u16() == 401 {
            self.refresh_token().await?;
            return Ok(
                self.client
                    .post(&url)
                    .bearer_auth(&self.token)
                    .json(body)
                    .send()
                    .await?
            );
        }

        Ok(res)
    }

    pub async fn app_exists(&mut self, app_id: &str) -> Result<bool, HttpError> {
        let res = self.get(&format!(
            "/api/collections/apps/records?filter=(app_id='{}')&limit=1",
            app_id
        )).await?;

        if !res.status().is_success() {
            return Err(HttpError::UnexpectedStatus(res.status()));
        }

        let list: ListResponse = res.json().await?;
        Ok(!list.items.is_empty())
    }

    pub async fn create_room(&mut self, region: &str, app_id: &str, room_id: &str, metadata: &str) -> Result<(), HttpError> {
        let body = serde_json::json!({
            "id": room_id,
            "region": region,
            "metadata": metadata,
            "app_id": app_id,
        });

        let res = self
            .post_json("/api/collections/rooms/records", &body)
            .await?;

        if !res.status().is_success() {
            return Err(HttpError::UnexpectedStatus(res.status()));
        }

        Ok(())
    }

    pub async fn delete_room(&mut self, room_id: &str) -> Result<(), HttpError> {
        let res = self
            .delete(&format!("/api/collections/rooms/records/{}", room_id))
            .await?;

        if !res.status().is_success() {
            return Err(HttpError::UnexpectedStatus(res.status()));
        }

        Ok(())
    }
}