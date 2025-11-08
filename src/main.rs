use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::OnceLock;
use warp::Filter;
use crate::config::{load_config, Config};
use crate::relay_server::RelayServer;

mod packet_type;
mod room;
mod relay_server;
mod renet_connection;
mod version;
mod config;

static CONFIG: OnceLock<Config> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    CONFIG.set(config).expect("Failed to set config");

    let cfg = CONFIG.get().unwrap();

    if !cfg.server.http_bind_address.is_empty() {
        let ready = warp::path!("ready").map(|| {
            warp::reply::json(&serde_json::json!({
                "status": "ready",
            }))
        });

        let addr: SocketAddr = cfg.server.http_bind_address
            .to_socket_addrs()?
            .next()
            .ok_or("Failed to resolve host name")?;

        println!("HTTP server listening on {}", addr);

        tokio::spawn(warp::serve(ready).run(addr));
    }

    let addr: SocketAddr = cfg.server.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let mut server = RelayServer::new(addr)?;
    server.run().await
}