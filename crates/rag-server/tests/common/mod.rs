use std::sync::Arc;

use axum::Router;
use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_server::router::build_router;
use rag_server::state::AppState;

/// Build a full AppState with mock embedder and mock LLM, backed by real
/// Postgres and Qdrant (requires `just up`).
pub async fn full_app() -> (Router, Arc<AppState>) {
    // SAFETY: test-only env manipulation; each integration test runs in its
    // own process so there are no data races with other threads reading env.
    unsafe { std::env::set_var("RAG_EMBEDDER", "mock") };
    let config = AppConfig::from_env().expect("test config");
    let stores = Stores::new(&config).await.expect("test stores");
    let ingest = IngestService::new(stores.clone(), &config).expect("test ingest");
    let retrieval = RetrievalService::new(stores.clone(), &config).expect("test retrieval");
    let chat = ChatService::with_mock_llm(stores.clone(), &config, "Mock LLM response.".into())
        .expect("test chat");
    let tenant_header = config.tenant_header.parse().expect("tenant header");
    let state = Arc::new(AppState { ingest, retrieval, chat, stores, config, tenant_header });
    let router = build_router(state.clone());
    (router, state)
}
