#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![warn(rust_2018_idioms)]
#![warn(unused_qualifications)]
#![warn(unused_crate_dependencies)]

use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::FmtSubscriber;
use crate::limits::TrafficLimits;
use crate::relay::server::RelayServer;
use crate::udp::paper_interface::PaperInterface;

mod config;
mod udp;
mod protocol;
mod relay;
mod limits;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    dotenvy::dotenv().ok();
    let config = config::loader::load_config("config.toml")?;
    let addr: SocketAddr = config.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let limits = TrafficLimits::from_config(&config);
    info!(
        "egress limits: {} B/s per client, {} B/s global, {} bytes/day gameplay + {} control, {}ms windows",
        limits.client_bytes_per_sec,
        limits.global_bytes_per_sec,
        limits.daily_bytes,
        limits.daily_control_bytes,
        limits.interval.as_millis()
    );

    let transport = PaperInterface::new(addr, limits).await?;

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
