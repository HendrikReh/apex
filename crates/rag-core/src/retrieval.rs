//! Retrieval service orchestrating dense, sparse, and hybrid search.

use anyhow::{Context, Result};

use crate::bm25::Bm25Embedder;
use crate::config::AppConfig;
use crate::embed::{AnyEmbedder, EmbedService};
use crate::fusion::{FusedChunk, RetrievedChunk, rrf_fusion};
use crate::stores::Stores;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Default retrieval parameters, loaded from config.
#[derive(Debug, Clone)]
pub struct RetrievalDefaults {
    pub rrf_k: u32,
    pub dense_top_k: u64,
    pub sparse_top_k: u64,
}

impl RetrievalDefaults {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            rrf_k: config.rrf_k,
            dense_top_k: config.dense_top_k,
            sparse_top_k: config.sparse_top_k,
        }
    }
}

/// Optional per-call overrides for hybrid search parameters.
#[derive(Debug, Clone, Default)]
pub struct HybridOverrides {
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct RetrievalService {
    stores: Stores,
    embedder: AnyEmbedder,
    bm25: Bm25Embedder,
    defaults: RetrievalDefaults,
}

impl RetrievalService {
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let embedder = AnyEmbedder::from_config(config).context("building embedder")?;
        let bm25 = Bm25Embedder::from_app_config(config).context("building BM25 embedder")?;
        let defaults = RetrievalDefaults::from_config(config);

        Ok(Self { stores, embedder, bm25, defaults })
    }

    /// Dense vector search: embed query, search Qdrant, return ranked chunks.
    pub async fn search_dense(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<RetrievedChunk>> {
        let query_vec = self
            .embedder
            .embed_batch(&[query.to_string()])
            .await
            .context("embedding query for dense search")?
            .into_iter()
            .next()
            .context("embedder returned empty result")?;

        let scored = self
            .stores
            .search_dense(collection, query_vec, tenant, limit)
            .await
            .context("dense search")?;

        Ok(scored_points_to_chunks(scored))
    }

    /// Sparse BM25 search: BM25-embed query, search Qdrant, return ranked chunks.
    pub async fn search_sparse(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<RetrievedChunk>> {
        let sparse = self.bm25.embed_query(query, None);

        if sparse.indices.is_empty() {
            return Ok(Vec::new());
        }

        let scored = self
            .stores
            .search_sparse(collection, sparse.indices, sparse.values, tenant, limit)
            .await
            .context("sparse search")?;

        Ok(scored_points_to_chunks(scored))
    }

    /// Hybrid search: run dense + sparse in parallel, fuse with RRF.
    pub async fn search_hybrid(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        overrides: Option<HybridOverrides>,
    ) -> Result<Vec<FusedChunk>> {
        let ov = overrides.unwrap_or_default();
        let dense_k = ov.dense_top_k.unwrap_or(self.defaults.dense_top_k);
        let sparse_k = ov.sparse_top_k.unwrap_or(self.defaults.sparse_top_k);
        let rrf_k = ov.rrf_k.unwrap_or(self.defaults.rrf_k);

        let (dense_result, sparse_result) = tokio::join!(
            self.search_dense(collection, query, tenant, dense_k),
            self.search_sparse(collection, query, tenant, sparse_k),
        );

        let dense = dense_result.context("hybrid dense leg")?;
        let sparse = sparse_result.context("hybrid sparse leg")?;

        Ok(rrf_fusion(&[("dense", dense), ("sparse", sparse)], rrf_k))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_point_id(id: &qdrant_client::qdrant::PointId) -> String {
    use qdrant_client::qdrant::point_id::PointIdOptions;
    match &id.point_id_options {
        Some(PointIdOptions::Uuid(s)) => s.clone(),
        Some(PointIdOptions::Num(n)) => n.to_string(),
        None => String::new(),
    }
}

fn scored_points_to_chunks(
    scored: Vec<qdrant_client::qdrant::ScoredPoint>,
) -> Vec<RetrievedChunk> {
    scored
        .into_iter()
        .map(|point| {
            let payload = &point.payload;
            let chunk_id = point.id.as_ref().map(extract_point_id).unwrap_or_default();
            let document_id = payload
                .get("document_id")
                .and_then(|v| v.as_str())
                .map_or("", |v| v)
                .to_string();
            let chunk_index = payload
                .get("chunk_index")
                .and_then(|v| v.as_integer())
                .unwrap_or(0) as i32;
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .map_or("", |v| v)
                .to_string();

            RetrievedChunk {
                chunk_id,
                document_id,
                chunk_index,
                text,
                score: point.score,
            }
        })
        .collect()
}
