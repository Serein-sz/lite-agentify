mod gateway;

use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    start_web_server().await?;

    Ok(())
}

async fn start_web_server() -> anyhow::Result<()> {
    let config = gateway::load_config_from_env().inspect_err(|error| {
        error!(%error, "failed to load gateway config");
    })?;

    let listen_addr = config.listen_addr;
    let app = gateway::build_router(config).inspect_err(|error| {
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
