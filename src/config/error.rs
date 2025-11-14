use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error("Config file could not be read: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Config file could not be parsed: {0}")]
    ParseError(#[from] toml::de::Error),
}