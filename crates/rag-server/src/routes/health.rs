use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::AppState;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[derive(Serialize)]
struct ReadinessResponse {
    ready: bool,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
struct ReadinessChecks {
    postgres: String,
    qdrant: String,
}

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
