mod account;
mod admin;
mod catalog;
mod config;
mod db;
mod domain;
mod model;
mod pricing;
mod proxy;
mod pubsub;
mod quota;
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
    reload::spawn_config_watcher(shared.clone());

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .inspect_err(|error| {
            error!(%error, %listen_addr, "failed to bind gateway listener");
        })?;

    info!(%listen_addr, "llm gateway server started");
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .inspect_err(|error| {
            error!(%error, "llm gateway server stopped with error");
        });

    // The listener has stopped accepting connections and in-flight requests
    // have drained. Flush any buffered usage records before exit; a no-op when
    // persistence is disabled.
    info!("flushing buffered usage records before shutdown");
    shared.load().usage_recorder.shutdown().await;

    serve_result?;
    Ok(())
}

/// Resolves when the process receives Ctrl-C or (on Unix) SIGTERM, triggering
/// graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            error!(%error, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    info!("shutdown signal received; stopping gateway");
}
