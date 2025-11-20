use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::Instant;
use crate::transport::common::Channel;
use crate::transport::error::TransportError;
use crate::transport::reliability::{ReliableReceiver, ReliableSender, SequenceNumber};

pub struct ClientTransport {
    socket: Arc<UdpSocket>,
    server_addr: SocketAddr,
    reliable_sender: Mutex<ReliableSender>,
    reliable_receiver: Mutex<ReliableReceiver>,
    pending_events: Vec<ClientEvent>,
    last_resend_check: Instant,
    last_ack_send: Instant,
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connected,
    Disconnected,
    PacketReceived { data: Vec<u8>, channel: Channel },
}

impl ClientTransport {
    pub async fn new(server_addr: SocketAddr) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind("0.0.0.0:0").await
            .map_err(|e| TransportError::BindError(e))?;

        Ok(Self {
            socket: Arc::new(socket),
            server_addr,
            reliable_sender: Mutex::new(ReliableSender::new()),
            reliable_receiver: Mutex::new(ReliableReceiver::new()),
            pending_events: Vec::new(),
            last_resend_check: Instant::now(),
            last_ack_send: Instant::now(),
        })
    }

    pub async fn recv_packets(&mut self) -> Vec<ClientEvent> {
        let mut buf = [0u8; 65535];
        let now = Instant::now();

        // Check resends
        if now.duration_since(self.last_resend_check) > std::time::Duration::from_millis(50) {
            self.do_resends().await;
            self.last_resend_check = now;
        }

        // Check ACKs
        if now.duration_since(self.last_ack_send) > std::time::Duration::from_millis(10) {
            self.send_acks().await.ok();
            self.last_ack_send = now;
        }

        loop {
            match self.socket.try_recv_from(&mut buf) {
                Ok((len, _addr)) => {
                    if len == 0 { continue; }

                    let packet_type = buf[0];

                    match packet_type {
                        0 => { // Reliable packet
                            if buf.len() < 5 { continue; }
                            let seq = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
                            let data = buf[5..len].to_vec();

                            let mut receiver = self.reliable_receiver.lock().await;
                            let acks = receiver.receive(SequenceNumber::new(seq), data);

                            // Queue acks
                            let mut sender = self.reliable_sender.lock().await;
                            for ack in acks {
                                sender.queue_ack(ack);
                            }

                            // Extract received packets
                            let packets = receiver.take_all_packets();
                            for packet in packets {
                                self.pending_events.push(ClientEvent::PacketReceived {
                                    data: packet,
                                    channel: Channel::Reliable,
                                });
                            }
                        }
                        1 => { // Unreliable packet
                            self.pending_events.push(ClientEvent::PacketReceived {
                                data: buf[1..len].to_vec(),
                                channel: Channel::Unreliable,
                            });
                        }
                        2 => { // ACK packet
                            if buf.len() < 5 { continue; }
                            let seq = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
                            let mut sender = self.reliable_sender.lock().await;
                            sender.ack_received(SequenceNumber::new(seq));
                        }
                        _ => {}
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }

        std::mem::take(&mut self.pending_events)
    }

    pub async fn send(&self, data: Vec<u8>, channel: Channel) -> Result<(), std::io::Error> {
        match channel {
            Channel::Reliable => {
                let mut sender = self.reliable_sender.lock().await;
                let seq = sender.send(data.clone());

                let mut packet = vec![0u8];
                packet.extend(seq.0.to_be_bytes());
                packet.extend(data);
                self.socket.send_to(&packet, self.server_addr).await?;
            }
            Channel::Unreliable => {
                let mut packet = vec![1u8];
                packet.extend(data);
                self.socket.send_to(&packet, self.server_addr).await?;
            }
        }
        Ok(())
    }

    async fn send_acks(&self) -> Result<(), std::io::Error> {
        let mut sender = self.reliable_sender.lock().await;
        let pending_acks = sender.get_pending_acks();

        for ack in pending_acks {
            let mut packet = vec![2u8]; // ACK packet type
            packet.extend(ack.0.to_be_bytes());
            self.socket.send_to(&packet, self.server_addr).await?;
        }
        Ok(())
    }

    async fn do_resends(&self) {
        let mut sender = self.reliable_sender.lock().await;
        let resends = sender.get_resends();

        for (seq, data) in resends {
            let mut packet = vec![0u8];
            packet.extend(seq.0.to_be_bytes());
            packet.extend(data);

            let _ = self.socket.send_to(&packet, self.server_addr).await;
        }
    }
}