use std::io::ErrorKind;
use std::mem::take;
use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use paperudp::channel::DecodeResult;
use paperudp::packet::PacketType;
use tracing::{debug, info, warn};
use crate::udp::error::UdpError;
use crate::udp::sessions::ConnectionManager;
use super::common::{ServerEvent, TransferChannel};

pub struct PaperInterface {
    pub(crate) socket: UdpSocket,
    connection_manager: ConnectionManager,
    pending_events: Vec<ServerEvent>,
}

impl PaperInterface {
    pub async fn new(addr: SocketAddr) -> Result<Self, UdpError> {
        let socket = UdpSocket::bind(addr).await
            .map_err(|e| UdpError::BindError(e))?;

        Ok(Self {
            socket,
            connection_manager: ConnectionManager::new(),
            pending_events: Vec::new(),
        })
    }

    pub async fn recv_events(&mut self) -> Result<Vec<ServerEvent>, UdpError> {
        let mut buf = [0u8; 65535];

        loop {
            match self.socket.try_recv_from(&mut buf) {
                Ok((len, addr)) => {
                    if len == 0 { continue; }

                    let (session_id, session_addr, res) = {
                        let session = self.connection_manager.get_or_create(addr);
                        session.last_heard_from = Instant::now();
                        let res = session.channel.decode(&buf[..len]);
                        (session.id, session.addr, res)
                    };

                    match res {
                        DecodeResult::Unreliable { payload } => {
                            for p in payload {
                                // heartbeat
                                if p == [3u8] {
                                    continue;
                                }

                                self.pending_events.push(ServerEvent::PacketReceived {
                                    client_id: session_id,
                                    data: p,
                                    channel: TransferChannel::Unreliable,
                                });
                            }
                        }
                        DecodeResult::Reliable { payload, ack_packet, .. } => {
                            for p in payload {
                                self.pending_events.push(ServerEvent::PacketReceived {
                                    client_id: session_id,
                                    data: p,
                                    channel: TransferChannel::Reliable,
                                });
                            }

                            if let Some(ack) = ack_packet {
                                if let Err(e) = self.socket
                                    .send_to(ack.as_slice(), session_addr)
                                    .await
                                {
                                    warn!("failed to send ack to {}: {}", session_addr, e);
                                }
                            }
                        }
                        DecodeResult::Ack { .. } => {}
                        DecodeResult::None => {
                            debug!("unknown packet: {:?}", &buf[..len]);
                            self.remove_client(&session_id);
                        }
                    }
                }
                Err(e) => match e.kind() {
                    ErrorKind::WouldBlock => break,

                    ErrorKind::Interrupted
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionAborted => {
                        continue;
                    }

                    _ => {
                        return Err(UdpError::RecvError(e));
                    }
                },
            }
        }

        Ok(take(&mut self.pending_events))
    }

    pub async fn send(&mut self, target: u64, data: Vec<u8>, channel: TransferChannel) -> Result<(), std::io::Error> {
        if let Some(session) = self.connection_manager.get_by_id(&target) {
            match channel {
                TransferChannel::Reliable => {
                    let pkt = session.channel.encode(
                        &*data,
                        PacketType::ReliableOrdered
                    );
                    self.socket.send_to(&pkt, session.addr).await?;
                }
                TransferChannel::Unreliable => {
                    let pkt = session.channel.encode(
                        &data,
                        PacketType::Unreliable
                    );
                    self.socket.send_to(&pkt, session.addr).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn do_resends(&mut self, interval: Duration) {
        for (addr, pkt) in self.connection_manager.get_resends(interval) {
            if let Err(e) = self.socket.send_to(&pkt, addr).await {
                warn!("failed to resend pkt {}", e);
                continue;
            }
        }
    }

    pub async fn cleanup_sessions(&mut self, timeout: Duration) {
        for client_id in self.connection_manager.cleanup_sessions(timeout) {
            self.pending_events.push(ServerEvent::ClientDisconnected { client_id });
        }
    }

    pub fn remove_client(&mut self, id: &u64) {
        self.connection_manager.remove_session(id);
    }
}