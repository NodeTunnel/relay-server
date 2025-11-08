use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use warp::Filter;
use crate::relay_server::RelayServer;

mod packet_type;
mod room;
mod relay_server;
mod renet_connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let ready = warp::path!("ready").map(|| {
        warp::reply::json(&serde_json::json!({
            "status": "ready",
            "udp_port": 8080
        }))
    });

    tokio::spawn(warp::serve(ready).run(([0, 0, 0, 0], 8081)));

    let addr: SocketAddr = "fly-global-services:8080"
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let mut server = RelayServer::new(addr)?;
    server.run().await
}