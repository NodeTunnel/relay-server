#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(rust_2018_idioms)]
#![warn(unused_qualifications)]
#![warn(unused_crate_dependencies)]

use crate::relay::server::RelayServer;
use crate::udp::paper_interface::PaperInterface;
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::FmtSubscriber;

mod config;
mod protocol;
mod relay;
mod udp;

fn get_log_level() -> tracing::Level {
    let log_level_str = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "debug".to_string());

    match log_level_str.to_lowercase().as_str() {
        "trace" => tracing::Level::TRACE,
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => {
            eprintln!("Invalid log level '{}', defaulting to DEBUG", log_level_str);
            tracing::Level::DEBUG
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(get_log_level())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let config = config::loader::load_config("config.toml")?;
    let addr: SocketAddr = config
        .udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = PaperInterface::new(addr).await?;

    let mut server = RelayServer::new(transport, config);
    info!("relay server started");
    tokio::select! {
        res = server.run() => {
            if let Err(e) = res {
                error!("server error: {}", e);
            }
        }
        _ = signal::ctrl_c() => {
            info!("shutdown signal received");
        }
    }

    info!("shutting down server");
    server.cleanup().await;

    Ok(())
}
