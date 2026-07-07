mod admin;
mod config;
mod domain;
mod model;
mod pricing;
mod proxy;
mod reload;
mod state;
mod usage;

use std::fmt;

use chrono::{FixedOffset, Utc};
use tracing::{error, info};
use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

struct UtcPlus8Timer;

impl FormatTime for UtcPlus8Timer {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        let offset = FixedOffset::east_opt(8 * 60 * 60).ok_or(fmt::Error)?;
        let now = Utc::now().with_timezone(&offset);
        write!(writer, "{}", now.format("%Y-%m-%dT%H:%M:%S%.6f%:z"))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_timer(UtcPlus8Timer).init();

    start_web_server().await?;

    Ok(())
}

async fn start_web_server() -> anyhow::Result<()> {
    let config_path = config::resolve_config_path();
    let mut gateway_config =
        config::GatewayConfig::load_from_path(&config_path).inspect_err(|error| {
            error!(%error, "failed to load gateway config");
        })?;

    // Hash a plaintext admin_password (rewriting the config file) before the
    // watcher exists, so the write-back cannot trigger a spurious reload.
    admin::bootstrap_admin_password(&mut gateway_config, &config_path);

    let listen_addr = gateway_config.listen_addr;
    let (app, shared) = proxy::build_router(gateway_config, config_path)
        .await
        .inspect_err(|error| {
            error!(%error, "failed to build gateway router");
        })?;
    reload::spawn_config_watcher(shared);

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .inspect_err(|error| {
            error!(%error, %listen_addr, "failed to bind gateway listener");
        })?;

    info!(%listen_addr, "llm gateway server started");
    axum::serve(listener, app).await.inspect_err(|error| {
        error!(%error, "llm gateway server stopped with error");
    })?;

    Ok(())
}
