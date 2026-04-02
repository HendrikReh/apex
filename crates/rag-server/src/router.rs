use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::middleware::auth::authenticate;
use crate::middleware::authz::authorize;
use crate::middleware::rate_limit::rate_limit;
use crate::middleware::request_id::request_id;
use crate::middleware::tenant::tenant_extraction;
use crate::openapi::ApiDoc;
use crate::routes;
use crate::state::AppState;

/// Build the full application router.
///
/// Public routes (`/health`, `/readiness`) have no auth middleware.
/// Protected routes get the full middleware stack:
///   request-ID → tenant → auth → rate-limit → authz → handler
pub fn build_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/agents", get(routes::agents::list_agents))
        .route("/agents/:id", get(routes::agents::get_agent))
        .route("/agents/:id/execute", post(routes::agents::execute_agent))
        .route("/runs", get(routes::agents::list_runs))
        .route("/runs/:id", get(routes::agents::get_run))
        .route("/runs/:id/approve", post(routes::agents::approve_run))
        .route("/runs/:id/reject", post(routes::agents::reject_run))
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
        // API key management
        .route("/auth/service-accounts", post(routes::api_keys::create_service_account))
        .route(
            "/auth/api-keys",
            post(routes::api_keys::create_api_key).get(routes::api_keys::list_api_keys),
        )
        .route("/auth/api-keys/:id", delete(routes::api_keys::revoke_api_key))
        // Middleware stack (applied in reverse: last layer = outermost = runs first)
        .layer(from_fn(authorize))
        .layer(from_fn_with_state(state.clone(), rate_limit))
        .layer(from_fn_with_state(state.clone(), authenticate))
        .layer(from_fn_with_state(state.clone(), tenant_extraction))
        .layer(from_fn(request_id))
        .with_state(state.clone());

    let public = Router::new()
        .route("/health", get(routes::health::health))
        .route("/readiness", get(routes::health::readiness))
        .with_state(state);

    let swagger =
        Router::<()>::from(SwaggerUi::new("/swagger-ui").url("/openapi.json", ApiDoc::openapi()));

    Router::new()
        .merge(public)
        .merge(protected)
        .merge(swagger)
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
}
