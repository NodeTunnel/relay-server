use std::error::Error;
use crate::relay_server::GameServer;

mod packet_type;
mod room;
mod relay_server;
mod renet_connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut server = GameServer::new("127.0.0.1:8080".parse()?)?;
    server.run().await
}