use thiserror::Error;

#[derive(Debug, Error)]
pub enum UdpError {
    #[error("failed to bind UDP socket: {0}")]
    BindError(std::io::Error),

    #[error("failed to send packet: {0}")]
    SendError(std::io::Error),

    #[error("failed to recv packet: {0}")]
    RecvError(std::io::Error),

    #[error("clock may have gone backwards: {0}")]
    ClockError(#[from] std::time::SystemTimeError),

    #[error("failed to create Netcode server udp: {0}")]
    NetcodeCreationFailed(std::io::Error),
}