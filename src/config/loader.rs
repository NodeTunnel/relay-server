use std::fs;
use serde::Deserialize;
use std::path::PathBuf;
use crate::config::error::ConfigError;

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default = "defaults::udp_bind_address")]
    pub udp_bind_address: String,

    #[serde(default = "defaults::app_whitelist")]
    pub app_whitelist: Vec<String>,

    #[serde(default = "defaults::allowed_versions")]
    pub allowed_versions: Vec<String>,

    #[serde(default = "defaults::relay_id")]
    pub relay_id: String,
}

pub fn load_config(path: &str) -> Result<Config, ConfigError> {
    let config_path = PathBuf::from(path);
    if config_path.exists() {
        let config_str = fs::read_to_string(path)?;
        return Ok(toml::from_str(&config_str)?);
    }

    Err(ConfigError::NotFound(path.to_string()))
}

mod defaults {
    pub fn udp_bind_address() -> String { "0.0.0.0:8080".to_string() }
    pub fn app_whitelist() -> Vec<String> { vec![] }
    pub fn allowed_versions() -> Vec<String> { vec![] }
    pub fn relay_id() -> String { "".to_string() }
}