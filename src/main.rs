use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::FmtSubscriber;
use crate::http::wrapper::HttpWrapper;
use crate::relay::server::RelayServer;
use crate::udp::paper_interface::PaperInterface;

mod config;
mod udp;
mod protocol;
mod relay;
mod http;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let config = config::loader::load_config("config.toml")?;
    let addr: SocketAddr = config.udp_bind_address
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve host name")?;

    let transport = PaperInterface::new(addr).await?;

    let http = if config.pocketbase_url.is_empty() {
        warn!("server running without http, app IDs will be validated against app_whitelist");
        None
    } else {
        HttpWrapper::new(
            &config.pocketbase_url,
            &config.pocketbase_email,
            &config.pocketbase_password
        )
            .await
            .map(Some)
            .unwrap_or_else(|e| {
                error!("failed to create http: {}", e);
                None
            })
    };

    let mut server = RelayServer::new(transport, http, config);
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
