use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rag_core::retrieval::HybridOverrides;
use serde::{Deserialize, Serialize};

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    pub top_k: Option<u64>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

fn validate_search(req: &SearchRequest) -> Result<(), ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if req.collection.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }
    Ok(())
}

pub async fn search_dense(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    validate_search(&payload)?;
    let top_k = payload.top_k.unwrap_or(state.config.dense_top_k);
    let chunks = state
        .retrieval
        .search_dense(&payload.collection, &payload.query, ctx.tenant.as_str(), top_k)
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| SearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            score: c.score,
        })
        .collect();

    Ok(Json(SearchResponse { results }))
}

pub async fn search_sparse(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    validate_search(&payload)?;
    let top_k = payload.top_k.unwrap_or(state.config.sparse_top_k);
    let chunks = state
        .retrieval
        .search_sparse(&payload.collection, &payload.query, ctx.tenant.as_str(), top_k)
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| SearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            score: c.score,
        })
        .collect();

    Ok(Json(SearchResponse { results }))
}

#[derive(Deserialize)]
pub struct HybridSearchRequest {
    pub query: String,
    pub collection: String,
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}

#[derive(Serialize)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Serialize)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}

pub async fn search_hybrid(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<HybridSearchRequest>,
) -> Result<Json<HybridSearchResponse>, ApiError> {
    if payload.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if payload.collection.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }

    let overrides = if payload.dense_top_k.is_some()
        || payload.sparse_top_k.is_some()
        || payload.rrf_k.is_some()
    {
        Some(HybridOverrides {
            dense_top_k: payload.dense_top_k,
            sparse_top_k: payload.sparse_top_k,
            rrf_k: payload.rrf_k,
        })
    } else {
        None
    };

    let chunks = state
        .retrieval
        .search_hybrid(&payload.collection, &payload.query, ctx.tenant.as_str(), overrides)
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| HybridSearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            fused_score: c.fused_score,
        })
        .collect();

    Ok(Json(HybridSearchResponse { results }))
}
