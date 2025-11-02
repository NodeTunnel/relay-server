use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime};
use renet::{ConnectionConfig, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let socket = UdpSocket::bind(addr)?;

    let server_config = ServerConfig {
        current_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?,
        max_clients: 100,
        protocol_id: 0,
        public_addresses: vec![addr],
        authentication: ServerAuthentication::Unsecure,
    };

    let mut transport = NetcodeServerTransport::new(server_config, socket)?;
    let mut server = RenetServer::new(ConnectionConfig::default());

    println!("Server listening on {}", addr);
    let mut last_update = Instant::now();

    loop {
        let now = Instant::now();
        let delta = now - last_update;
        last_update = now;

        server.update(delta);
        transport.update(delta, &mut server)?;

        // Process events
        while let Some(event) = server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    println!("Client {} has connected!", client_id);
                    // Send welcome message or initial data
                    server.broadcast_message(0, b"Welcome to the server!".to_vec());
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    println!("Client {} disconnected: {:?}", client_id, reason);
                }
            }
        }

        // Process received messages
        for client_id in server.clients_id() {
            while let Some(message) = server.receive_message(client_id, 0) {
                println!("Received from client {}: {:?}", client_id, message);
                server.broadcast_message(0, message);
            }
        }

        sleep(Duration::from_millis(10)).await;
    }
}