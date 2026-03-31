use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::AppState;

#[utoipa::path(get, path = "/health", tag = "Health", responses(
    (status = 200, description = "Service is alive"),
))]
pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ReadinessResponse {
    ready: bool,
    checks: ReadinessChecks,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ReadinessChecks {
    postgres: String,
    qdrant: String,
}

#[utoipa::path(get, path = "/readiness", tag = "Health", responses(
    (status = 200, description = "All backends reachable", body = ReadinessResponse),
    (status = 503, description = "One or more backends unreachable", body = ReadinessResponse),
))]
pub async fn readiness(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let timeout = Duration::from_secs(5);

    let pg_check = tokio::time::timeout(timeout, async {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(state.stores.pg_pool())
            .await
            .map(|_| "ok".to_string())
            .map_err(|e| format!("{e}"))
    });

    let qdrant_check = tokio::time::timeout(timeout, async {
        state
            .stores
            .qdrant_client()
            .health_check()
            .await
            .map(|_| "ok".to_string())
            .map_err(|e| format!("{e}"))
    });

    // tokio::join! macro internally uses .expect() — false positive for ADR-001.
    #[allow(clippy::disallowed_methods)]
    let (pg_result, qdrant_result) = tokio::join!(pg_check, qdrant_check);

    let pg_status = match pg_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(_) => "timeout after 5s".to_string(),
    };

    let qdrant_status = match qdrant_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(_) => "timeout after 5s".to_string(),
    };

    let ready = pg_status == "ok" && qdrant_status == "ok";
    let status = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };

    let body = ReadinessResponse {
        ready,
        checks: ReadinessChecks { postgres: pg_status, qdrant: qdrant_status },
    };

    (status, axum::Json(body))
}
