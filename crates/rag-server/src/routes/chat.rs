use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rag_core::chat::ChatRequest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{ApiError, AppState, Ctx, ErrorBody};

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ChatHttpRequest {
    pub query: String,
    pub collection: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub language: Option<String>,
    pub history_limit: Option<i64>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ChatHttpResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<CitationJson>,
    pub usage: UsageJson,
    pub model: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CitationJson {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct UsageJson {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[utoipa::path(post, path = "/chat", tag = "Chat",
    request_body = ChatHttpRequest,
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    responses(
        (status = 200, description = "Chat response with citations", body = ChatHttpResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
pub async fn chat(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatHttpRequest>,
) -> Result<Json<ChatHttpResponse>, ApiError> {
    if payload.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if let Some(limit) = payload.history_limit
        && limit <= 0
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: format!("history_limit must be > 0, got {limit}"),
        });
    }
    if let Some(ref coll) = payload.collection
        && coll.trim().is_empty()
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }
    if payload.conversation_id.is_none() && payload.collection.is_none() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection is required for the first message in a conversation".into(),
        });
    }

    let request = ChatRequest {
        query: payload.query,
        collection: payload.collection,
        tenant: ctx.tenant,
        conversation_id: payload.conversation_id,
        language: payload.language,
        history_limit: payload.history_limit,
    };

    let response = state.chat.chat(request).await.map_err(classify_chat_error)?;

    let citations = response
        .citations
        .into_iter()
        .map(|c| CitationJson {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            sources: c.sources,
        })
        .collect();

    Ok(Json(ChatHttpResponse {
        answer: response.answer,
        conversation_id: response.conversation_id,
        citations,
        usage: UsageJson {
            prompt_tokens: response.usage.prompt_tokens,
            completion_tokens: response.usage.completion_tokens,
        },
        model: response.model,
    }))
}

/// Classify `ChatService::chat` errors as client (400) or server (500).
///
/// The service uses `anyhow::bail!` for request-level problems (conversation
/// not found, collection mismatch, etc.).  We inspect the root error message
/// for known prefixes so these surface as 400 to the caller.
fn classify_chat_error(err: anyhow::Error) -> ApiError {
    // Match against the root cause only — rag-core uses `bail!` for client
    // errors, so the root message is always the user-facing text. The
    // intermediate context chain may contain internal details and is never
    // returned to the client.
    let root = format!("{}", err.root_cause());

    const CLIENT_PREFIXES: &[&str] = &[
        "collection mismatch",
        "collection must not be empty",
        "collection is required",
        "conversation not found",
        "has no stored collection",
        "history_limit must be",
    ];

    let is_client_error = CLIENT_PREFIXES.iter().any(|p| root.contains(p));

    if is_client_error {
        ApiError { status: StatusCode::BAD_REQUEST, message: root }
    } else {
        ApiError::internal_with_context("chat request failed", &err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_client_errors_remain_actionable() {
        let err = classify_chat_error(anyhow::anyhow!("conversation not found: id=123"));

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("conversation not found"));
    }

    #[test]
    fn chat_client_errors_strip_context_chain() {
        let inner = anyhow::anyhow!("conversation not found: id=123");
        let chained = inner.context("internal db detail: SELECT * FROM conversations");

        let err = classify_chat_error(chained);

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        // Only the root cause is returned, not the context chain
        assert!(!err.message.contains("SELECT"));
        assert!(err.message.contains("conversation not found"));
    }

    #[test]
    fn chat_server_errors_are_sanitized() {
        let err = classify_chat_error(anyhow::anyhow!(
            "sql query failed: password authentication failed"
        ));

        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.message, "internal error");
    }
}
