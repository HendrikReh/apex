use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use rag_core::TenantId;
use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};

use crate::agents::AgentManager;
use crate::auth::Principal;
use crate::auth::oidc::JwksCache;
use crate::middleware::rate_limit::RateLimiterState;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

/// Runtime authentication and rate-limiting state.
pub struct AuthState {
    pub jwks_cache: JwksCache,
    pub rate_limiter: RateLimiterState,
}

/// Shared application state, wrapped in `Arc` for Axum handlers.
pub struct AppState {
    pub ingest: IngestService,
    pub retrieval: Arc<RetrievalService>,
    pub chat: Arc<ChatService>,
    pub agents: Arc<AgentManager>,
    pub stores: Stores,
    pub config: AppConfig,
    pub tenant_header: axum::http::HeaderName,
    pub auth: AuthState,
}

/// Per-request context injected by middleware.
#[derive(Clone)]
pub struct RequestContext {
    pub request_id: Uuid,
    pub tenant: TenantId,
    pub principal: Principal,
}

/// Typed extractor for `RequestContext`. Returns 500 if middleware did not run.
pub struct Ctx(pub RequestContext);

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<RequestContext>().cloned().map(Ctx).ok_or_else(|| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "missing request context".into(),
        })
    }
}

/// Standardised JSON error response.
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

const INTERNAL_ERROR_MESSAGE: &str = "internal error";

impl ApiError {
    pub fn internal() -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: INTERNAL_ERROR_MESSAGE.into() }
    }

    pub fn internal_with_context<E>(context: &'static str, error: E) -> Self
    where
        E: std::fmt::Display,
    {
        tracing::error!(context = context, error = %error, "request failed");
        Self::internal()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: self.message };
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| format!(r#"{{"error":"{INTERNAL_ERROR_MESSAGE}"}}"#));

        // SAFETY: all values are hardcoded constants — the builder cannot fail.
        #[allow(clippy::disallowed_methods)]
        Response::builder()
            .status(self.status)
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(json))
            .unwrap_or_else(|_| {
                // Final fallback: status only, no headers.
                // SAFETY: hardcoded 500 status is always valid.
                #[allow(clippy::disallowed_methods)]
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(axum::body::Body::from(format!(
                        r#"{{"error":"{INTERNAL_ERROR_MESSAGE}"}}"#
                    )))
                    .expect("hardcoded response must build")
            })
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self::internal_with_context("unhandled request error", &err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test — .expect() is fine
    async fn anyhow_errors_are_sanitized_for_clients() {
        let response =
            ApiError::from(anyhow::anyhow!("database connection failed: postgres://secret"))
                .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body bytes");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(parsed["error"], INTERNAL_ERROR_MESSAGE);
    }

    #[test]
    fn explicit_client_errors_are_preserved() {
        let err = ApiError { status: StatusCode::BAD_REQUEST, message: "invalid role".into() };
        assert_eq!(err.message, "invalid role");
    }
}
