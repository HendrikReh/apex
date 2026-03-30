use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_server::router;
use rag_server::state::AppState;

mod bootstrap;

// tokio::main macro internally uses .expect() — false positive for ADR-001.
#[allow(clippy::disallowed_methods)]
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let config = AppConfig::from_env().context("loading config")?;
    tracing::info!(bind = %config.bind_addr, "starting server");
    bootstrap::print_banner(
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_LICENSE"),
        &config.bind_addr,
    );

    let stores = Stores::new(&config).await.context("connecting stores")?;

    let ingest = IngestService::new(stores.clone(), &config).context("building ingest service")?;
    let retrieval =
        RetrievalService::new(stores.clone(), &config).context("building retrieval service")?;
    let chat = ChatService::new(stores.clone(), &config).context("building chat service")?;

    let tenant_header = config.tenant_header.parse().context("parsing tenant header name")?;

    let bind_addr = config.bind_addr.clone();

    let state = Arc::new(AppState { ingest, retrieval, chat, stores, config, tenant_header });

    let app = router::build_router(state);

    let listener = TcpListener::bind(&bind_addr).await.context("binding listener")?;

    tracing::info!(addr = %listener.local_addr()?, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(bootstrap::shutdown_signal())
        .await
        .context("running server")?;

    Ok(())
}
