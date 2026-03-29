//! `ApiClient` trait for abstracting over the HTTP client.
//!
//! CLI command handlers are generic over `impl ApiClient`, enabling
//! unit testing with a `FakeClient`.

use crate::error::ClientError;
use crate::types::*;

#[trait_variant::make(Send)]
pub trait ApiClient {
    async fn health(&self) -> Result<(), ClientError>;
    async fn readiness(&self) -> Result<ReadinessResponse, ClientError>;
    async fn ingest(&self, req: &IngestRequest) -> Result<IngestResponse, ClientError>;
    async fn search_dense(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_sparse(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_hybrid(
        &self,
        req: &HybridSearchRequest,
    ) -> Result<HybridSearchResponse, ClientError>;
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError>;
    async fn collection_stats(
        &self,
        collection: &str,
    ) -> Result<CollectionStatsResponse, ClientError>;
}
