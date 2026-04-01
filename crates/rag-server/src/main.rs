use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_server::agents::AgentManager;
use rag_server::auth::oidc::JwksCache;
use rag_server::middleware::rate_limit::RateLimiterState;
use rag_server::router;
use rag_server::state::{AppState, AuthState};

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
        &config.llm_provider.to_string(),
        &config.llm_model,
    );

    let stores = Stores::new(&config).await.context("connecting stores")?;

    let ingest = IngestService::new(stores.clone(), &config).context("building ingest service")?;
    let retrieval = Arc::new(
        RetrievalService::new(stores.clone(), &config).context("building retrieval service")?,
    );
    let chat =
        Arc::new(ChatService::new(stores.clone(), &config).context("building chat service")?);
    let agents = Arc::new(
        AgentManager::load_default(&config.agent_specs_dir, retrieval.clone(), chat.clone())
            .await
            .context("loading agent manager")?,
    );

    let tenant_header = config.tenant_header.parse().context("parsing tenant header name")?;

    let bind_addr = config.bind_addr.clone();

    let auth = AuthState {
        jwks_cache: JwksCache::default(),
        rate_limiter: RateLimiterState::new(
            config.rate_limit_global_rps,
            config.rate_limit_global_burst,
            config.rate_limit_global_concurrency,
            config.rate_limit_tenant_rps,
            config.rate_limit_tenant_burst,
        ),
    };

    let state =
        Arc::new(AppState { ingest, retrieval, chat, agents, stores, config, tenant_header, auth });

    if state.config.auth_mode == rag_core::config::AuthMode::None {
        tracing::warn!("auth_mode=none: running without authentication (development only)");
    }

    let app = router::build_router(state);

    let listener = TcpListener::bind(&bind_addr).await.context("binding listener")?;

    tracing::info!(addr = %listener.local_addr()?, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(bootstrap::shutdown_signal())
        .await
        .context("running server")?;

    Ok(())
}
