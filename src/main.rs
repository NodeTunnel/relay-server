use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use crate::game::server::RelayServer;
use crate::transport::server::PaperUDP;

mod config;
mod transport;
mod protocol;
mod game;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = config::loader::load_config("config.toml")?;

    let addr: SocketAddr = config.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = PaperUDP::new(addr).await?;
    let mut server = RelayServer::new(transport, config);
    server.run().await
}
