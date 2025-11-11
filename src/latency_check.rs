use std::sync::Arc;
use tokio::net::UdpSocket;

pub struct LatencyCheck {
    socket: Arc<UdpSocket>,
}

impl LatencyCheck {
    pub async fn new(addr: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        println!("LatencyCheck server listening on {addr}");
        Ok(Self {
            socket: Arc::new(socket),
        })
    }

    pub async fn run(&self) -> std::io::Result<()> {
        let mut buf = [0u8; 1024];
        loop {
            let (len, addr) = self.socket.recv_from(&mut buf).await?;
            self.socket.send_to(&buf[..len], addr).await?;
        }
    }
}