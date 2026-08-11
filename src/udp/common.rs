#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferChannel {
    Reliable,
    Unreliable,
}

/// What a packet is for. Daily budgets split on this, not on channel, since
/// relayed `GameData` uses whichever channel it arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficClass {
    /// Relayed peer traffic. Dropped first.
    Gameplay,
    /// Auth results, room listings, join and disconnect notices.
    Control,
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    ClientConnected { client_id: u64 },
    ClientDisconnected { client_id: u64 },
    PacketReceived { client_id: u64, data: Vec<u8>, channel: TransferChannel },
}