use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use paperudp::channel::DecodeResult;
use paperudp::packet::PacketType;
use tracing::{debug, error, warn};
use crate::limits::{BandwidthLimiter, TrafficLimits, WIRE_OVERHEAD_ESTIMATE};
use crate::udp::error::UdpError;
use crate::udp::sessions::ConnectionManager;
use super::common::{ServerEvent, TrafficClass, TransferChannel};

const MAX_RELIABLE_SEND_BYTES: usize = 16 * 1024;

const DAY: Duration = Duration::from_hours(24);

pub struct PaperInterface {
    pub(crate) socket: UdpSocket,
    pub(crate) connection_manager: ConnectionManager,
    pending_events: Vec<ServerEvent>,

    global_traffic: BandwidthLimiter,
    // Daily egress is bounded by the sum of the two.
    daily_gameplay: BandwidthLimiter,
    daily_control: BandwidthLimiter,
    /// One warning per window, not per dropped packet.
    gameplay_budget_reported: bool,
    control_budget_reported: bool,
}

impl PaperInterface {
    pub async fn new(addr: SocketAddr, limits: TrafficLimits) -> Result<Self, UdpError> {
        let socket = UdpSocket::bind(addr).await
            .map_err(|e| UdpError::BindError(e))?;

        let now = Instant::now();

        Ok(Self {
            socket,
            connection_manager: ConnectionManager::new(limits),
            pending_events: Vec::new(),
            global_traffic: BandwidthLimiter::per_second(
                limits.global_bytes_per_sec,
                limits.interval,
                now,
            ),
            daily_gameplay: BandwidthLimiter::per_window(limits.daily_bytes, DAY, now),
            daily_control: BandwidthLimiter::per_window(limits.daily_control_bytes, DAY, now),
            gameplay_budget_reported: false,
            control_budget_reported: false,
        })
    }

    pub async fn recv_events(&mut self) -> Result<Vec<ServerEvent>, UdpError> {
        let mut buf = [0u8; 65535];

        loop {
            self.socket.readable().await.map_err(UdpError::RecvError)?;

            loop {
                match self.socket.try_recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        if len == 0 { continue; }

                        let (session_id, session_addr, res) = {
                            let Some((session, is_new)) = self.connection_manager.get_or_create(addr) else {
                                continue;
                            };

                            if is_new {
                                self.pending_events.push(ServerEvent::ClientConnected {
                                    client_id: session.id
                                })
                            }

                            session.last_heard_from = Instant::now();
                            let res = session.channel.decode(&buf[..len]);
                            (session.id, session.addr, res)
                        };

                        match res {
                            DecodeResult::Unreliable { payload } => {
                                for p in payload {
                                    if p == [3u8] { continue; }
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
                                    if self.charge_control(ack.len(), Instant::now())
                                        && let Err(e) = self.socket.send_to(ack.as_slice(), session_addr).await
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

                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted
                ) => continue,
                    Err(e) => return Err(UdpError::RecvError(e)),
                }
            }

            if !self.pending_events.is_empty() {
                return Ok(std::mem::take(&mut self.pending_events));
            }
        }
    }

    /// Limits are checked before encoding, because encoding a reliable packet
    /// queues it for resend. An over-limit packet is dropped and reported as `Ok`.
    pub async fn send(
        &mut self,
        target: u64,
        data: Vec<u8>,
        channel: TransferChannel,
        class: TrafficClass,
    ) -> Result<(), std::io::Error> {
        if matches!(channel, TransferChannel::Reliable) && data.len() > MAX_RELIABLE_SEND_BYTES {
            error!(
                "refusing to send {} byte reliable packet to {} (max {})",
                data.len(), target, MAX_RELIABLE_SEND_BYTES
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "packet exceeds maximum send size",
            ));
        }

        let now = Instant::now();
        let charge = data.len() as u64 + WIRE_OVERHEAD_ESTIMATE;
        let is_unreliable = matches!(channel, TransferChannel::Unreliable);

        if !self.global_traffic.would_fit(charge, now) {
            debug!("dropping {} byte packet to {}: global rate limit", charge, target);
            return Ok(());
        }

        let (budget, reported, label) = match class {
            TrafficClass::Gameplay => (
                &mut self.daily_gameplay,
                &mut self.gameplay_budget_reported,
                "gameplay",
            ),
            TrafficClass::Control => (
                &mut self.daily_control,
                &mut self.control_budget_reported,
                "control",
            ),
        };

        if budget.would_fit(charge, now) {
            // Re-arm the warning for the next window.
            *reported = false;
        } else {
            if !*reported {
                warn!(
                    "daily {} egress budget of {} bytes is exhausted; \
                     dropping {} traffic until the window rolls over",
                    label,
                    budget.window_allowance(),
                    label
                );
                *reported = true;
            }
            return Ok(());
        }

        let encoded = {
            let Some(session) = self.connection_manager.get_by_id(&target) else {
                return Ok(());
            };

            if session.traffic.would_fit(charge, now) {
                session.traffic.consume(charge, now);
                session.bytes_sent_total = session.bytes_sent_total.saturating_add(charge);

                let packet_type = if is_unreliable {
                    PacketType::Unreliable
                } else {
                    PacketType::ReliableOrdered
                };

                Some((session.channel.encode(&data, packet_type), session.addr))
            } else {
                // Unreliable loss is normal under load.
                if !is_unreliable {
                    session.traffic_violations = session.traffic_violations.saturating_add(1);
                }

                debug!("dropping {} byte packet to {}: client rate limit", charge, target);
                None
            }
        };

        // Charge global budgets only for packets that actually go out.
        let Some((pkt, addr)) = encoded else {
            return Ok(());
        };

        self.global_traffic.consume(charge, now);
        match class {
            TrafficClass::Gameplay => self.daily_gameplay.consume(charge, now),
            TrafficClass::Control => self.daily_control.consume(charge, now),
        }

        self.socket.send_to(&pkt, addr).await?;
        Ok(())
    }

    pub async fn do_resends(&mut self, interval: Duration) {
        let now = Instant::now();

        for (addr, pkt) in self.connection_manager.get_resends(interval) {
            // Resends are egress too.
            if !self.charge_control(pkt.len(), now) {
                continue;
            }

            if let Err(e) = self.socket.send_to(&pkt, addr).await {
                warn!("failed to resend pkt {}", e);
                continue;
            }
        }
    }

    /// Charges acks and resends, which bypass `send`, against the daily control
    /// budget. Not rate limited: throttling recovery traffic under load would
    /// disconnect healthy clients.
    fn charge_control(&mut self, bytes: usize, now: Instant) -> bool {
        let charge = bytes as u64 + WIRE_OVERHEAD_ESTIMATE;

        if self.daily_control.would_fit(charge, now) {
            self.daily_control.consume(charge, now);
            self.control_budget_reported = false;
            return true;
        }

        if !self.control_budget_reported {
            warn!(
                "daily control egress budget of {} bytes is exhausted; \
                 dropping acks and resends until the window rolls over",
                self.daily_control.window_allowance()
            );
            self.control_budget_reported = true;
        }

        false
    }

    pub fn remove_client(&mut self, id: &u64) {
        self.connection_manager.remove_session(id);
    }
}