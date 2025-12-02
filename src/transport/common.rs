#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Reliable,
    Unreliable,
}

#[derive(Debug, Clone)]
pub struct Packet {
    pub client_id: u64,
    pub data: Vec<u8>,
    pub channel: Channel,
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    ClientConnected { client_id: u64 },
    ClientDisconnected { client_id: u64 },
    PacketReceived { client_id: u64, data: Vec<u8>, channel: Channel },
}