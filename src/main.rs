use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::OnceLock;
use crate::config::{load_config, Config};
use crate::relay_server::RelayServer;

mod packet_type;
mod room;
mod relay_server;
mod renet_connection;
mod version;
mod config;
mod app;

static CONFIG: OnceLock<Config> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    CONFIG.set(config).expect("Failed to set config");

    let cfg = CONFIG.get().unwrap();

    let addr: SocketAddr = cfg.server.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let mut server = RelayServer::new(addr)?;
    server.run().await
}