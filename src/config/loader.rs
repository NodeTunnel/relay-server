use std::fs;
use serde::Deserialize;
use std::path::PathBuf;
use crate::config::error::ConfigError;

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default = "defaults::udp_bind_address")]
    pub udp_bind_address: String,

    #[serde(default = "defaults::whitelist")]
    pub whitelist: Vec<String>,

    #[serde(default = "defaults::allowed_versions")]
    pub allowed_versions: Vec<String>,

    #[serde(default = "defaults::empty_string")]
    pub remote_whitelist_endpoint: String,

    #[serde(default = "defaults::empty_string")]
    pub remote_whitelist_token: String,

    #[serde(default = "defaults::empty_string")]
    pub relay_id: String,

    /// Sustained egress allowed to a single client.
    #[serde(default = "defaults::max_client_bytes_per_sec")]
    pub max_client_bytes_per_sec: u64,

    /// Sustained egress allowed across every client at once.
    #[serde(default = "defaults::max_global_bytes_per_sec")]
    pub max_global_bytes_per_sec: u64,

    /// Relayed gameplay egress allowed in a day. This is what bounds the bill.
    #[serde(default = "defaults::max_daily_bytes")]
    pub max_daily_bytes: u64,

    /// Control-plane egress allowed in a day, on top of `max_daily_bytes`.
    /// Kept separate so a throttled relay can still tell clients why.
    #[serde(default = "defaults::max_daily_control_bytes")]
    pub max_daily_control_bytes: u64,

    /// Window the rate limits are sliced into.
    #[serde(default = "defaults::traffic_interval_ms")]
    pub traffic_interval_ms: u64,

    /// Lifetime egress to one session before it is dropped.
    #[serde(default = "defaults::max_session_bytes")]
    pub max_session_bytes: u64,

    /// How long a single session may live.
    #[serde(default = "defaults::max_session_secs")]
    pub max_session_secs: u64,
}

pub fn load_config(path: &str) -> Result<Config, ConfigError> {
    let config_path = PathBuf::from(path);

    let config = if config_path.exists() {
        let config_str = fs::read_to_string(path)?;
        toml::from_str(&config_str)?
    } else {
        // Fatal on purpose. Falling back to defaults would silently drop every limit.
        envy::from_env::<Config>()?
    };

    validate(&config)?;

    Ok(config)
}

fn validate(config: &Config) -> Result<(), ConfigError> {
    // 0 never means "unlimited".
    let limits = [
        ("max_client_bytes_per_sec", config.max_client_bytes_per_sec),
        ("max_global_bytes_per_sec", config.max_global_bytes_per_sec),
        ("max_daily_bytes", config.max_daily_bytes),
        ("max_daily_control_bytes", config.max_daily_control_bytes),
        ("traffic_interval_ms", config.traffic_interval_ms),
        ("max_session_bytes", config.max_session_bytes),
        ("max_session_secs", config.max_session_secs),
    ];

    for (name, value) in limits {
        if value == 0 {
            return Err(ConfigError::Invalid(format!(
                "{name} is 0; it must be a positive value (0 does not mean unlimited)"
            )));
        }
    }

    if config.max_client_bytes_per_sec > config.max_global_bytes_per_sec {
        return Err(ConfigError::Invalid(format!(
            "max_client_bytes_per_sec ({}) is above max_global_bytes_per_sec ({}), \
             so the per-client limit could never be reached",
            config.max_client_bytes_per_sec, config.max_global_bytes_per_sec
        )));
    }

    if config.traffic_interval_ms > 60_000 {
        return Err(ConfigError::Invalid(format!(
            "traffic_interval_ms ({}) is above 60000; a window that long lets a \
             burst run for a minute before the limit applies",
            config.traffic_interval_ms
        )));
    }

    Ok(())
}

mod defaults {
    pub fn udp_bind_address() -> String { "0.0.0.0:8080".to_string() }
    pub fn whitelist() -> Vec<String> { vec![] }
    pub fn allowed_versions() -> Vec<String> { vec![] }
    pub fn empty_string() -> String { "".to_string() }

    pub fn max_client_bytes_per_sec() -> u64 { 64 * 1024 }
    pub fn max_global_bytes_per_sec() -> u64 { 2_000_000 }
    pub fn max_daily_bytes() -> u64 { 5_000_000_000 }
    /// 5% of the gameplay budget.
    pub fn max_daily_control_bytes() -> u64 { 250_000_000 }
    /// At the default per-client rate one window is exactly `MAX_RELIABLE_SEND_BYTES`.
    pub fn traffic_interval_ms() -> u64 { 250 }
    pub fn max_session_bytes() -> u64 { 2_000_000_000 }
    pub fn max_session_secs() -> u64 { 4 * 60 * 60 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Config {
        Config {
            udp_bind_address: defaults::udp_bind_address(),
            whitelist: defaults::whitelist(),
            allowed_versions: defaults::allowed_versions(),
            remote_whitelist_endpoint: defaults::empty_string(),
            remote_whitelist_token: defaults::empty_string(),
            relay_id: defaults::empty_string(),
            max_client_bytes_per_sec: defaults::max_client_bytes_per_sec(),
            max_global_bytes_per_sec: defaults::max_global_bytes_per_sec(),
            max_daily_bytes: defaults::max_daily_bytes(),
            max_daily_control_bytes: defaults::max_daily_control_bytes(),
            traffic_interval_ms: defaults::traffic_interval_ms(),
            max_session_bytes: defaults::max_session_bytes(),
            max_session_secs: defaults::max_session_secs(),
        }
    }

    #[test]
    fn defaults_are_valid() {
        assert!(validate(&valid()).is_ok());
    }

    #[test]
    fn zero_is_rejected_rather_than_meaning_unlimited() {
        let cases: Vec<(&str, fn(&mut Config))> = vec![
            ("max_client_bytes_per_sec", |c| c.max_client_bytes_per_sec = 0),
            ("max_global_bytes_per_sec", |c| c.max_global_bytes_per_sec = 0),
            ("max_daily_bytes", |c| c.max_daily_bytes = 0),
            ("max_daily_control_bytes", |c| c.max_daily_control_bytes = 0),
            ("traffic_interval_ms", |c| c.traffic_interval_ms = 0),
            ("max_session_bytes", |c| c.max_session_bytes = 0),
            ("max_session_secs", |c| c.max_session_secs = 0),
        ];

        for (name, zero_it) in cases {
            let mut config = valid();
            zero_it(&mut config);
            assert!(validate(&config).is_err(), "{name} at 0 should be rejected");
        }
    }

    #[test]
    fn per_client_limit_above_the_global_one_is_rejected() {
        let mut config = valid();
        config.max_client_bytes_per_sec = config.max_global_bytes_per_sec + 1;

        assert!(validate(&config).is_err());
    }

    #[test]
    fn per_client_limit_equal_to_the_global_one_is_allowed() {
        let mut config = valid();
        config.max_client_bytes_per_sec = config.max_global_bytes_per_sec;

        assert!(validate(&config).is_ok());
    }

    #[test]
    fn absurdly_long_traffic_windows_are_rejected() {
        let mut config = valid();
        config.traffic_interval_ms = 60_001;

        assert!(validate(&config).is_err());
    }
}