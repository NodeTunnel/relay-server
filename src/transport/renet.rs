use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};
use renet::{ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use crate::protocol::version::PROTOCOL_VERSION;
use crate::transport::error::TransportError;
use crate::transport::types::{Channel, Packet};

/// A wrapper for the Renet server and Netcode transport
pub struct RenetTransport {
    server: RenetServer,
    transport: NetcodeServerTransport,
}

impl RenetTransport {
    pub fn new(addr: SocketAddr, max_clients: usize) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr).map_err(TransportError::BindError)?;

        let server_config = ServerConfig {
            current_time: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?,
            max_clients,
            protocol_id: PROTOCOL_VERSION,
            public_addresses: vec![addr],
            authentication: ServerAuthentication::Unsecure,
        };

        let transport = NetcodeServerTransport::new(server_config, socket)
            .map_err(TransportError::NetcodeCreationFailed)?;

        let server = RenetServer::new(ConnectionConfig::default());

        Ok(Self {
            server,
            transport,
        })
    }

    pub fn update(&mut self, delta_time: Duration) -> Result<(), TransportError> {
        self.server.update(delta_time);
        self.transport.update(delta_time, &mut self.server)
            .map_err(TransportError::NetcodeUpdateFailed)?;

        self.transport.send_packets(&mut self.server);
        
        Ok(())
    }

    pub fn recv_packets(&mut self) -> Vec<Packet> {
        let mut received_packets = Vec::new();

        let channels = [Channel::Reliable, Channel::Unreliable];

        for client_id in self.server.clients_id() {
            for channel in &channels {
                while let Some(message) = self.server.receive_message(client_id, DefaultChannel::from(*channel)) {
                    let packet = Packet {
                        client_id,
                        data: Vec::from(message),
                        channel: *channel,
                    };

                    received_packets.push(packet);
                }
            }
        }

        received_packets
    }

    pub fn recv_events(&mut self) -> Vec<ServerEvent> {
        let mut received_events = Vec::new();

        while let Some(event) = self.server.get_event() {
            received_events.push(event);
        }

        received_events
    }

    pub fn send(&mut self, target: ClientId, data: Vec<u8>, channel: Channel) {
        self.server.send_message(target, DefaultChannel::from(channel), data);
        self.transport.send_packets(&mut self.server);
    }

    pub fn disconnect_client(&mut self, target: ClientId) {
        self.server.disconnect(target);
    }
}
