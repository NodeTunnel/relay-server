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
}

pub fn load_config(path: &str) -> Result<Config, ConfigError> {
    let config_path = PathBuf::from(path);

    if config_path.exists() {
        let config_str = fs::read_to_string(path)?;
        return Ok(toml::from_str(&config_str)?);
    }

    // Fallback to environment variables
    match envy::from_env::<Config>() {
        Ok(cfg) => Ok(cfg),
        Err(_) => Ok(Config {
            udp_bind_address: defaults::udp_bind_address(),
            whitelist: defaults::whitelist(),
            allowed_versions: defaults::allowed_versions(),
            remote_whitelist_endpoint: defaults::empty_string(),
            remote_whitelist_token: defaults::empty_string(),
            relay_id: defaults::empty_string(),
        }),
    }
}

mod defaults {
    pub fn udp_bind_address() -> String { "0.0.0.0:8080".to_string() }
    pub fn whitelist() -> Vec<String> { vec![] }
    pub fn allowed_versions() -> Vec<String> { vec![] }
    pub fn empty_string() -> String { "".to_string() }
}