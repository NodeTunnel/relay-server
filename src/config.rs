use std::error::Error;
use serde::Deserialize;
use std::fs;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub server: ServerConfig,
}

#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    #[serde(default = "defaults::udp_bind_address")]
    pub udp_bind_address: String,

    #[serde(default = "defaults::app_whitelist")]
    pub app_whitelist: Vec<String>,
}

pub(crate) fn load_config() -> Result<Config, Box<dyn Error>> {
    let paths = [
        "config.toml",
        "./config/config.toml",
        "/etc/relay-server/config.toml",
        "/app/config.toml",
    ];

    for path in &paths {
        if let Ok(config_str) = fs::read_to_string(path) {
            println!("Loaded config from: {}", path);
            return Ok(toml::from_str(&config_str)?);
        }
    }

    Err("Could not find config.toml in any expected location".into())
}

mod defaults {
    pub fn udp_bind_address() -> String { "0.0.0.0:8080".to_string() }
    pub fn app_whitelist() -> Vec<String> { vec![] }
}