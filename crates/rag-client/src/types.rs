//! Request and response types matching the Apex RAG server API.
//!
//! These types are defined independently of `rag-server` so that
//! `rag-client` has no workspace dependencies.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Ingest ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct IngestRequest {
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(default)]
    pub failures: Vec<IngestFailure>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestFailure {
    pub path: String,
    pub error: String,
}

// ── Search ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct HybridSearchRequest {
    pub query: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense_top_k: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparse_top_k: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}

// ── Chat ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<Citation>,
    pub usage: Usage,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ── Collections ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CollectionStatsResponse {
    pub collection: String,
    pub tenant: String,
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}

// ── Health / Readiness ──────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub checks: ReadinessChecks,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReadinessChecks {
    pub postgres: String,
    pub qdrant: String,
}
