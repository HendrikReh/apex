use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rag_core::retrieval::HybridOverrides;
use serde::{Deserialize, Serialize};

use crate::state::{ApiError, AppState, Ctx, ErrorBody};

#[derive(Deserialize, utoipa::ToSchema)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    pub top_k: Option<u64>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Serialize, utoipa::ToSchema)]
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

fn classify_search_error(err: anyhow::Error) -> ApiError {
    let msg = format!("{err:#}");
    let msg_lower = msg.to_ascii_lowercase();
    let is_client_error = msg_lower.contains("collection")
        && (msg_lower.contains("does not exist") || msg_lower.contains("not found"));

    if is_client_error {
        ApiError { status: StatusCode::BAD_REQUEST, message: msg }
    } else {
        ApiError::internal_with_context("search request failed", &err)
    }
}

#[utoipa::path(post, path = "/search/dense", tag = "Search",
    request_body = SearchRequest,
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    responses(
        (status = 200, description = "Dense search results", body = SearchResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
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
        .map_err(classify_search_error)?;

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

#[utoipa::path(post, path = "/search/sparse", tag = "Search",
    request_body = SearchRequest,
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    responses(
        (status = 200, description = "Sparse BM25 search results", body = SearchResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
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
        .map_err(classify_search_error)?;

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

#[derive(Deserialize, utoipa::ToSchema)]
pub struct HybridSearchRequest {
    pub query: String,
    pub collection: String,
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}

#[utoipa::path(post, path = "/search/hybrid", tag = "Search",
    request_body = HybridSearchRequest,
    params(("x-tenant" = String, Header, description = "Tenant identifier")),
    responses(
        (status = 200, description = "Hybrid search results with RRF fusion", body = HybridSearchResponse),
        (status = 400, description = "Invalid request", body = ErrorBody),
    ),
    security(("api_key" = []))
)]
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

    if payload.dense_top_k == Some(0) {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "dense_top_k must be greater than 0".into(),
        });
    }
    if payload.sparse_top_k == Some(0) {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "sparse_top_k must be greater than 0".into(),
        });
    }
    if payload.rrf_k == Some(0) {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "rrf_k must be greater than 0".into(),
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
        .map_err(classify_search_error)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_collection_not_found_is_actionable() {
        let err = classify_search_error(anyhow::anyhow!(
            "dense search: searching dense vectors in collection 'missing' (tenant 't'): Not found: collection does not exist"
        ));

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("collection 'missing'"));
    }

    #[test]
    fn search_backend_failures_are_sanitized() {
        let err = classify_search_error(anyhow::anyhow!(
            "dense search: qdrant unavailable at http://127.0.0.1:6334"
        ));

        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.message, "internal error");
    }
}
