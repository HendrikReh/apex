//! Adapter traits for external capabilities.
//!
//! These traits define the seam between agent-core and the rest of the system.
//! Implementations live outside this crate (typically in `rag-server`), keeping
//! agent-core free of concrete storage, LLM, or approval-UI dependencies.

use crate::types::{CheckpointDecision, GroundedAnswer, PendingCheckpoint, ScoredChunk};

/// Hybrid retrieval capability.
#[async_trait::async_trait]
pub trait RetrievalPort: Send + Sync {
    async fn search_dense(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>>;

    async fn search_sparse(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>>;

    async fn search_hybrid(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
    ) -> anyhow::Result<Vec<ScoredChunk>>;

    async fn search_fts(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>>;

    async fn expand_chunk_neighbors(
        &self,
        tenant: &str,
        document_id: &str,
        chunk_index: i32,
        before: i32,
        after: i32,
    ) -> anyhow::Result<Vec<ScoredChunk>>;

    async fn fetch_document(
        &self,
        tenant: &str,
        document_id: &str,
    ) -> anyhow::Result<serde_json::Value>;
}

#[async_trait::async_trait]
pub trait BaselineAnswerPort: Send + Sync {
    async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> anyhow::Result<GroundedAnswer>;
}

/// LLM summarization / answer generation capability.
#[async_trait::async_trait]
pub trait ChatPort: Send + Sync {
    /// Generate a summary or answer given context chunks and a query.
    async fn summarize(
        &self,
        query: &str,
        context_chunks: &[ScoredChunk],
        tenant: &str,
    ) -> anyhow::Result<String>;
}

/// Checkpoint approval capability.
///
/// Abstracts graph-flow's `WaitForInput` so the rest of the crate never sees it.
#[async_trait::async_trait]
pub trait ApprovalPort: Send + Sync {
    /// Request approval for a checkpoint. Returns `None` if no decision is
    /// available yet (the run should pause).
    async fn request_approval(
        &self,
        checkpoint: &PendingCheckpoint,
    ) -> anyhow::Result<Option<CheckpointDecision>>;
}
