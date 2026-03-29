use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rag_core::TenantId;
use uuid::Uuid;

use crate::state::{ApiError, AppState, RequestContext};

/// Middleware that extracts and validates the tenant from the configured header.
///
/// Falls back to `"default"` if the header is absent. Returns 400 if the value
/// fails `TenantId` validation. Depends on request-ID middleware having run
/// first (reads `Uuid` from extensions).
pub async fn tenant_extraction(
    State(state): State<Arc<AppState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let tenant_str = req
        .headers()
        .get(&state.tenant_header)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default");

    let tenant = TenantId::new(tenant_str).map_err(|e| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid tenant: {e}"),
    })?;

    let request_id = req
        .extensions()
        .get::<Uuid>()
        .copied()
        .unwrap_or_else(Uuid::new_v4);

    let ctx = RequestContext { request_id, tenant: tenant.clone() };
    req.extensions_mut().insert(ctx);

    let mut response = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(tenant.as_str()) {
        response.headers_mut().insert(state.tenant_header.clone(), val);
    }

    Ok(response)
}
