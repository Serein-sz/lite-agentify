mod gateway;

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
    let config = gateway::load_config_from_env().inspect_err(|error| {
        error!(%error, "failed to load gateway config");
    })?;

    let listen_addr = config.listen_addr;
    let app = gateway::build_router(config).await.inspect_err(|error| {
        error!(%error, "failed to build gateway router");
    })?;

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
