use tokio::net::UdpSocket;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use paperudp::channel::{Channel, DecodeResult};
use paperudp::packet::PacketType;
use super::common::{ServerEvent, TransferChannels};

pub struct ClientSession {
    channel: Channel,
    last_heard_from: Instant,
}

pub struct PaperTransport {
    pub socket: UdpSocket,
    clients: HashMap<SocketAddr, u64>,
    client_sessions: HashMap<u64, ClientSession>,
    client_addrs: HashMap<u64, SocketAddr>,
    next_client_id: u64,
    pending_events: Vec<ServerEvent>,
}

impl PaperTransport {
    pub async fn new(addr: SocketAddr) -> Result<Self, std::io::Error> {
        let socket = UdpSocket::bind(addr).await?;

        Ok(Self {
            socket,
            clients: HashMap::new(),
            client_sessions: HashMap::new(),
            client_addrs: HashMap::new(),
            next_client_id: 1,
            pending_events: Vec::new(),
        })
    }

    pub async fn recv_events(&mut self) -> Vec<ServerEvent> {
        let mut buf = [0u8; 65535];

        loop {
            match self.socket.try_recv_from(&mut buf) {
                Ok((len, addr)) => {
                    if len == 0 { continue; }

                    // session management
                    let client_id = if let Some(&id) = self.clients.get(&addr) {
                        id
                    } else {
                        let id = self.next_client_id;
                        self.next_client_id += 1;
                        self.clients.insert(addr, id);
                        self.client_addrs.insert(id, addr);

                        self.client_sessions.insert(id, ClientSession {
                            channel: Channel::new(),
                            last_heard_from: Instant::now(),
                        });

                        self.pending_events.push(ServerEvent::ClientConnected { client_id: id });
                        id
                    };

                    let Some(session) = self.client_sessions.get_mut(&client_id) else {
                        continue;
                    };
                    // end session management

                    session.last_heard_from = Instant::now();
                    let res = session.channel.decode(&buf[..len]);

                    match res {
                        DecodeResult::Data { payload, ack_packet } => {
                            for p in payload {
                                self.pending_events.push(ServerEvent::PacketReceived {
                                    client_id,
                                    data: p,
                                    channel: TransferChannels::Reliable,
                                });
                            }

                            if let Some(ack) = ack_packet {
                                self.socket.send_to(
                                    ack.as_slice(),
                                    self.client_addrs.get(&client_id).unwrap()
                                ).await.expect("TODO: panic message");
                            }
                        }
                        DecodeResult::Ack { .. } => {}
                        DecodeResult::None => {}
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        std::mem::take(&mut self.pending_events)
    }

    pub async fn send(&mut self, target: u64, data: Vec<u8>, channel: TransferChannels) -> Result<(), std::io::Error> {
        if let Some(&addr) = self.client_addrs.get(&target) {
            match channel {
                TransferChannels::Reliable => {
                    if let Some(session) = self.client_sessions.get_mut(&target) {
                        let pkt = session.channel.encode(
                            &*data,
                            PacketType::ReliableOrdered
                        );
                        self.socket.send_to(&pkt, addr).await?;
                    }
                }
                TransferChannels::Unreliable => {
                    let mut packet = vec![0u8];
                    packet.extend(data);
                    self.socket.send_to(&packet, addr).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn do_resends(&mut self, interval: Duration) {
        for (id, session) in self.client_sessions.iter_mut() {
            let addr = self.client_addrs.get(id).expect("exists");
            let resends = session.channel.collect_resends(interval);

            for pkt in resends {
                self.socket.send_to(&*pkt, addr).await.unwrap();
            }
        }
    }

    pub async fn cleanup_sessions(&mut self, timeout: Duration) {
        let now = Instant::now();
        let mut clients_to_disconnect = Vec::new();

        for (&client_id, session) in &self.client_sessions {
            if now.duration_since(session.last_heard_from) > timeout {
                clients_to_disconnect.push(client_id);
            }
        }

        if clients_to_disconnect.is_empty() {
            return;
        }

        for client_id in clients_to_disconnect {
            self.disconnect_client(client_id);
        }
    }

    pub fn disconnect_client(&mut self, target: u64) {
        if let Some(addr) = self.client_addrs.remove(&target) {
            println!("client disconnected: {}", target);
            self.clients.remove(&addr);
            self.client_sessions.remove(&target);
            self.pending_events.push(ServerEvent::ClientDisconnected { client_id: target });
        }
    }
}