use std::error::Error;
use crate::relay_server::RelayServer;

mod packet_type;
mod room;
mod relay_server;
mod renet_connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut server = RelayServer::new("0.0.0.0:8080".parse()?)?;
    server.run().await
}