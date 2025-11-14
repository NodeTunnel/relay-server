use tokio::time::Instant;

pub struct ClientSession {
    pub app_id: String,
    pub connected_at: Instant,
}