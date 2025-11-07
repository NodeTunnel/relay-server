use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};
use renet::{Bytes, ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};

pub struct Packet {
    pub renet_id: ClientId,
    pub data: Bytes,
    pub channel: DefaultChannel,
}

pub struct RenetConnection {
    server: RenetServer,
    transport: NetcodeServerTransport,
}

impl RenetConnection {
    pub fn new(bind_addr: SocketAddr) -> Result<Self, Box<dyn Error>> {
        let socket = UdpSocket::bind(bind_addr)?;

        let server_config = ServerConfig {
            current_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?,
            max_clients: 100,
            protocol_id: 0,
            public_addresses: vec![bind_addr],
            authentication: ServerAuthentication::Unsecure,
        };

        let transport = NetcodeServerTransport::new(server_config, socket)?;
        let server = RenetServer::new(ConnectionConfig::default());

        Ok(Self {
            server,
            transport,
        })
    }

    pub fn receive_packets(&mut self) -> Result<Vec<Packet>, Box<dyn Error>> {
        let delta_time = Duration::from_millis(16);
        let mut received_packets = Vec::new();

        self.server.update(delta_time);
        self.transport.update(delta_time, &mut self.server)?;

        for client_id in self.server.clients_id() {
            while let Some(message) = self.server.receive_message(client_id, DefaultChannel::ReliableOrdered) {
                let packet = Packet {
                    renet_id: client_id,
                    data: message,
                    channel: DefaultChannel::ReliableOrdered,
                };
                
                received_packets.push(packet);
            }

            while let Some(message) = self.server.receive_message(client_id, DefaultChannel::Unreliable) {
                let packet = Packet {
                    renet_id: client_id,
                    data: message,
                    channel: DefaultChannel::Unreliable,
                };

                received_packets.push(packet);
            }
        }
        
        Ok(received_packets)
    }
    
    pub fn receive_events(&mut self) -> Result<Vec<ServerEvent>, Box<dyn Error>> {
        let mut received_events = Vec::new();
        
        while let Some(event) = self.server.get_event() {
            received_events.push(event);
        }
        
        Ok(received_events)
    }
    
    pub fn send(&mut self, target: ClientId, data: Vec<u8>, channel: DefaultChannel) {
        self.server.send_message(target, channel, data);
        self.transport.send_packets(&mut self.server);
    }

    pub fn close(&mut self) {
        self.server.disconnect_all();
    }
}
