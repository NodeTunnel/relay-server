use std::net::SocketAddr;
use axum::{Router, routing::get};

async fn health_check() -> &'static str {
    "OK"
}

pub async fn run_health_server(addr: SocketAddr) {
    let app = Router::new().route("/health", get(health_check));

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}