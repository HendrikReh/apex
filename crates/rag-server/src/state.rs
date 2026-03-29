use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_core::TenantId;
use serde::Serialize;
use uuid::Uuid;

/// Shared application state, wrapped in `Arc` for Axum handlers.
pub struct AppState {
    pub ingest: IngestService,
    pub retrieval: RetrievalService,
    pub chat: ChatService,
    pub stores: Stores,
    pub config: AppConfig,
    pub tenant_header: axum::http::HeaderName,
}

/// Per-request context injected by middleware.
#[derive(Clone)]
pub struct RequestContext {
    pub request_id: Uuid,
    pub tenant: TenantId,
}

/// Typed extractor for `RequestContext`. Returns 500 if middleware did not run.
pub struct Ctx(pub RequestContext);

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<RequestContext>()
            .cloned()
            .map(Ctx)
            .ok_or_else(|| ApiError {
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

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: self.message };
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| r#"{"error":"internal error"}"#.to_string());

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
                    .body(axum::body::Body::from(r#"{"error":"internal error"}"#))
                    .expect("hardcoded response must build")
            })
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("{err:#}"),
        }
    }
}
