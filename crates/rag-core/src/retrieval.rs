//! Retrieval service orchestrating dense, sparse, and hybrid search.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};

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

        Ok(scored_points_to_chunks(scored, "dense"))
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

        Ok(scored_points_to_chunks(scored, "sparse"))
    }

    /// Postgres full-text search over chunk text for lexical lookups.
    pub async fn search_fts(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<RetrievedChunk>> {
        let rows = self
            .stores
            .search_chunks_fts(tenant, collection, query, limit)
            .await
            .context("fts search")?;

        Ok(rows
            .into_iter()
            .map(|row| RetrievedChunk {
                chunk_id: stable_chunk_id(tenant, &row.document_id, row.chunk_index),
                document_id: row.document_id,
                chunk_index: row.chunk_index,
                text: row.text,
                title: string_to_option(row.title),
                source_url: metadata_string(row.metadata.as_ref(), "/source/url"),
                source_domain: metadata_string(row.metadata.as_ref(), "/source/domain"),
                language: row.language,
                tags: metadata_tags(row.metadata.as_ref()),
                section_heading: None,
                collection: row.collection,
                score: row.score,
                score_type: "fts".to_string(),
            })
            .collect())
    }

    /// Retrieve adjacent chunks from Postgres for evidence expansion.
    pub async fn expand_chunk_neighbors(
        &self,
        tenant: &str,
        document_id: &str,
        chunk_index: i32,
        before: i32,
        after: i32,
    ) -> Result<Vec<RetrievedChunk>> {
        let rows = self
            .stores
            .get_chunk_neighbors(tenant, document_id, chunk_index, before, after)
            .await
            .context("chunk neighbor expansion")?;

        Ok(rows
            .into_iter()
            .map(|row| RetrievedChunk {
                chunk_id: stable_chunk_id(tenant, &row.document_id, row.chunk_index),
                document_id: row.document_id,
                chunk_index: row.chunk_index,
                text: row.text,
                title: string_to_option(row.title),
                source_url: metadata_string(row.metadata.as_ref(), "/source/url"),
                source_domain: metadata_string(row.metadata.as_ref(), "/source/domain"),
                language: row.language,
                tags: metadata_tags(row.metadata.as_ref()),
                section_heading: None,
                collection: row.collection,
                score: row.score,
                score_type: "neighbor".to_string(),
            })
            .collect())
    }

    /// Hybrid search: run dense + sparse in parallel, fuse with RRF.
    #[allow(clippy::disallowed_methods)] // tokio::join! internally uses .expect()
    pub async fn search_hybrid(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        overrides: Option<HybridOverrides>,
    ) -> Result<Vec<FusedChunk>> {
        let ov = overrides.unwrap_or_default();
        let dense_k =
            resolve_override_u64(ov.dense_top_k, self.defaults.dense_top_k, "dense_top_k")?;
        let sparse_k =
            resolve_override_u64(ov.sparse_top_k, self.defaults.sparse_top_k, "sparse_top_k")?;
        let rrf_k = resolve_override_u32(ov.rrf_k, self.defaults.rrf_k, "rrf_k")?;

        #[allow(clippy::disallowed_methods)] // tokio::join! internally uses .expect()
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

fn resolve_override_u64(value: Option<u64>, default: u64, field: &str) -> Result<u64> {
    match value {
        Some(0) => Err(anyhow!("{field} override must be greater than 0")),
        Some(value) => Ok(value),
        None => Ok(default),
    }
}

fn resolve_override_u32(value: Option<u32>, default: u32, field: &str) -> Result<u32> {
    match value {
        Some(0) => Err(anyhow!("{field} override must be greater than 0")),
        Some(value) => Ok(value),
        None => Ok(default),
    }
}

fn extract_point_id(id: &qdrant_client::qdrant::PointId) -> Option<String> {
    use qdrant_client::qdrant::point_id::PointIdOptions;
    match &id.point_id_options {
        Some(PointIdOptions::Uuid(s)) => Some(s.clone()),
        Some(PointIdOptions::Num(n)) => Some(n.to_string()),
        None => None,
    }
}

fn stable_chunk_id(tenant: &str, document_id: &str, chunk_index: i32) -> String {
    let input = format!("{tenant}:{document_id}:{chunk_index}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, input.as_bytes()).to_string()
}

fn string_to_option(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn payload_string(
    payload: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| if value.is_empty() { None } else { Some(value.to_string()) })
}

fn metadata_string(metadata: Option<&serde_json::Value>, pointer: &str) -> Option<String> {
    metadata
        .and_then(|metadata| metadata.pointer(pointer))
        .and_then(|value| value.as_str())
        .and_then(|value| if value.is_empty() { None } else { Some(value.to_string()) })
}

fn metadata_tags(metadata: Option<&serde_json::Value>) -> Vec<String> {
    metadata
        .and_then(|metadata| metadata.get("tags"))
        .and_then(|tags| serde_json::from_value(tags.clone()).ok())
        .unwrap_or_default()
}

fn payload_tags(payload: &HashMap<String, qdrant_client::qdrant::Value>) -> Vec<String> {
    payload
        .get("tags")
        .and_then(|value| value.as_str())
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .unwrap_or_default()
}

fn scored_points_to_chunks(
    scored: Vec<qdrant_client::qdrant::ScoredPoint>,
    score_type: &str,
) -> Vec<RetrievedChunk> {
    scored
        .into_iter()
        .filter_map(|point| {
            // Fusion and citations rely on stable chunk IDs, so skip malformed
            // search results that do not include a usable Qdrant point ID.
            let payload = &point.payload;
            let chunk_id = point.id.as_ref().and_then(extract_point_id)?;
            let document_id =
                payload.get("document_id").and_then(|v| v.as_str()).map_or("", |v| v).to_string();
            let chunk_index =
                payload.get("chunk_index").and_then(|v| v.as_integer()).unwrap_or(0) as i32;
            let text = payload.get("text").and_then(|v| v.as_str()).map_or("", |v| v).to_string();

            Some(RetrievedChunk {
                chunk_id,
                document_id,
                chunk_index,
                text,
                title: payload_string(payload, "title"),
                source_url: payload_string(payload, "source_url"),
                source_domain: payload_string(payload, "source_domain"),
                language: payload_string(payload, "language"),
                tags: payload_tags(payload),
                section_heading: payload_string(payload, "section_heading"),
                collection: payload_string(payload, "collection"),
                score: point.score,
                score_type: score_type.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{resolve_override_u32, resolve_override_u64};

    #[test]
    fn zero_u64_override_is_rejected() {
        let err =
            resolve_override_u64(Some(0), 10, "dense_top_k").expect_err("zero override must fail");
        assert!(err.to_string().contains("dense_top_k"));
    }

    #[test]
    fn zero_u32_override_is_rejected() {
        let err = resolve_override_u32(Some(0), 60, "rrf_k").expect_err("zero override must fail");
        assert!(err.to_string().contains("rrf_k"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions using unwrap for brevity
    fn missing_override_uses_default() {
        assert_eq!(resolve_override_u64(None, 10, "dense_top_k").unwrap(), 10);
        assert_eq!(resolve_override_u32(None, 60, "rrf_k").unwrap(), 60);
    }
}
