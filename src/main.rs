use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;
use crate::game::server::GameServer;
use crate::transport::RenetTransport;

mod config;
mod transport;
mod protocol;
mod game;
mod registry;
mod health;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = config::load_config("config.toml")?;

    let addr: SocketAddr = config.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = RenetTransport::new(addr, 100)?;

    let health_addr: SocketAddr = config.http_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve http host name")?;
    tokio::spawn(health::run_health_server(health_addr));

    let mut server = GameServer::new(transport, config);
    server.run(Duration::from_millis(4)).await
}
