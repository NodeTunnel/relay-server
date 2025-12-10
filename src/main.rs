use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use crate::relay::server::RelayServer;
use crate::udp::paper_interface::PaperInterface;

mod config;
mod udp;
mod protocol;
mod relay;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = config::loader::load_config("config.toml")?;

    let addr: SocketAddr = config.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = PaperInterface::new(addr).await?;
    let mut server = RelayServer::new(transport, config);
    server.run().await
}
