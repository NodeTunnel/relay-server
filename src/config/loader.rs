use crate::config::error::ConfigError;
use crate::protocol::version::PROTOCOL_VERSION;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

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

    #[serde(default = "defaults::session_timeout_secs")]
    pub session_timeout_secs: u64,
}

pub fn load_config(path: &str) -> Result<Config, ConfigError> {
    let config_path = PathBuf::from(path);

    let config = if config_path.exists() {
        let config_str = fs::read_to_string(path)?;
        toml::from_str(&config_str)?
    } else {
        envy::from_env::<Config>()?
    };

    validate(&config)?;

    Ok(config)
}

fn validate(config: &Config) -> Result<(), ConfigError> {
    if config.allowed_versions.is_empty() {
        return Err(ConfigError::Invalid(
            "allowed_versions is empty, which rejects every client".to_string(),
        ));
    }

    if config.session_timeout_secs == 0 {
        return Err(ConfigError::Invalid(
            "session_timeout_secs is 0, which disconnects every client on the first cleanup tick"
                .to_string(),
        ));
    }

    Ok(())
}

mod defaults {
    pub fn udp_bind_address() -> String {
        "0.0.0.0:8080".to_string()
    }
    pub fn whitelist() -> Vec<String> {
        vec![]
    }
    pub fn allowed_versions() -> Vec<String> {
        vec![super::PROTOCOL_VERSION.to_string()]
    }
    pub fn empty_string() -> String {
        String::new()
    }
    pub fn session_timeout_secs() -> u64 {
        5
    }
}
