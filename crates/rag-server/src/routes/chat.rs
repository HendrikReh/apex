use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rag_core::chat::ChatRequest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct ChatHttpRequest {
    pub query: String,
    pub collection: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub language: Option<String>,
    pub history_limit: Option<i64>,
}

#[derive(Serialize)]
pub struct ChatHttpResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<CitationJson>,
    pub usage: UsageJson,
    pub model: String,
}

#[derive(Serialize)]
pub struct CitationJson {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Serialize)]
pub struct UsageJson {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

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

    let request = ChatRequest {
        query: payload.query,
        collection: payload.collection,
        tenant: ctx.tenant,
        conversation_id: payload.conversation_id,
        language: payload.language,
        history_limit: payload.history_limit,
    };

    let response = state.chat.chat(request).await.map_err(ApiError::from)?;

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
