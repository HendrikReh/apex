use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};

use crate::middleware::request_id::request_id;
use crate::middleware::tenant::tenant_extraction;
use crate::routes;
use crate::state::AppState;

/// Build the full application router.
///
/// Public routes (`/health`, `/readiness`) have no middleware.
/// Protected routes get request-ID and tenant-extraction middleware.
pub fn build_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/ingest", post(routes::ingest::ingest_paths))
        .route(
            "/ingest/upload",
            post(routes::ingest::ingest_upload).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/search/dense", post(routes::search::search_dense))
        .route("/search/sparse", post(routes::search::search_sparse))
        .route("/search/hybrid", post(routes::search::search_hybrid))
        .route("/chat", post(routes::chat::chat))
        .route("/collections/:collection/stats", get(routes::collections::collection_stats))
        .layer(from_fn_with_state(state.clone(), tenant_extraction))
        .layer(from_fn(request_id))
        .with_state(state.clone());

    let public = Router::new()
        .route("/health", get(routes::health::health))
        .route("/readiness", get(routes::health::readiness))
        .with_state(state);

    Router::new().merge(public).merge(protected).layer(DefaultBodyLimit::max(10 * 1024 * 1024))
}
