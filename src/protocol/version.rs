/// This is not used as Renet silently disconnects if protocol versions don't match.
/// See the Authenticate packet. Allowed versions are set in config.toml
pub const PROTOCOL_VERSION: u64 = 1;