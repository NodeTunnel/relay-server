#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferChannel {
    Reliable,
    Unreliable,
}

#[derive(Debug, Clone)]
pub struct Packet {
    pub client_id: u64,
    pub data: Vec<u8>,
    pub channel: TransferChannel,
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    ClientDisconnected { client_id: u64 },
    PacketReceived { client_id: u64, data: Vec<u8>, channel: TransferChannel },
}