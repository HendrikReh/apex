//! Qdrant vector storage operations.
//!
//! Provides collection management (create / validate / list / delete) and
//! point-level operations (upsert, delete-by-filter, dense search, sparse
//! search) with tenant isolation enforced via payload filters.
//!
//! Collections use **named vectors**: a `"dense"` vector for embeddings and a
//! `"bm25_sparse"` sparse vector for BM25-based retrieval.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use qdrant_client::qdrant::{
    Condition, CreateCollection, DeletePointsBuilder, Distance, Filter, Modifier, PointStruct,
    ScoredPoint, SearchPoints, SparseIndices, SparseVectorConfig, SparseVectorParams,
    UpsertPointsBuilder, VectorParams, VectorParamsMap, VectorsConfig, WithPayloadSelector,
    vectors_config::Config as VectorsConfigVariant, with_payload_selector::SelectorOptions,
};

use super::Stores;

/// Named vector key for dense embeddings.
pub const DENSE_VECTOR_NAME: &str = "dense";
/// Named vector key for BM25 sparse embeddings.
pub const SPARSE_VECTOR_NAME: &str = "bm25_sparse";

impl Stores {
    async fn validate_collection_config(
        &self,
        name: &str,
        dense_size: u64,
        distance: Distance,
    ) -> Result<()> {
        let info = self
            .qdrant
            .collection_info(name)
            .await
            .with_context(|| format!("fetching collection info for '{name}'"))?;

        let params = info
            .result
            .as_ref()
            .and_then(|r| r.config.as_ref())
            .and_then(|c| c.params.as_ref())
            .ok_or_else(|| anyhow!("collection '{name}' is missing configuration details"))?;

        let vectors_config = params
            .vectors_config
            .as_ref()
            .ok_or_else(|| anyhow!("collection '{name}' is missing vectors_config"))?;

        match &vectors_config.config {
            Some(VectorsConfigVariant::ParamsMap(map)) => {
                let dense_params = map.map.get(DENSE_VECTOR_NAME).ok_or_else(|| {
                    anyhow!("collection '{name}' is missing named vector '{DENSE_VECTOR_NAME}'")
                })?;
                if dense_params.size != dense_size {
                    return Err(anyhow!(
                        "collection '{name}' dense vector size {}, expected {dense_size}",
                        dense_params.size,
                    ));
                }
                if dense_params.distance() != distance {
                    return Err(anyhow!(
                        "collection '{name}' dense distance {:?}, expected {distance:?}",
                        dense_params.distance(),
                    ));
                }
            }
            _ => {
                return Err(anyhow!(
                    "collection '{name}' does not use named vectors (expected ParamsMap)"
                ));
            }
        }

        let sparse_config = params
            .sparse_vectors_config
            .as_ref()
            .ok_or_else(|| anyhow!("collection '{name}' is missing sparse_vectors_config"))?;
        let sparse_params = sparse_config.map.get(SPARSE_VECTOR_NAME).ok_or_else(|| {
            anyhow!("collection '{name}' is missing sparse vector '{SPARSE_VECTOR_NAME}'")
        })?;
        if sparse_params.modifier != Some(Modifier::Idf.into()) {
            return Err(anyhow!(
                "collection '{name}' sparse vector '{SPARSE_VECTOR_NAME}' modifier {:?}, expected {:?}",
                sparse_params.modifier,
                Modifier::Idf,
            ));
        }

        Ok(())
    }

    /// Ensure a Qdrant collection exists with the expected vector parameters.
    ///
    /// * If the collection does **not** exist it is created with named vectors:
    ///   a `"dense"` vector with the given `dense_size` and `distance` metric,
    ///   and a `"bm25_sparse"` sparse vector with IDF modifier.
    /// * If the collection **does** exist its configuration is validated against
    ///   the expected parameters; a mismatch returns an error.
    pub async fn ensure_collection(
        &self,
        name: &str,
        dense_size: u64,
        distance: Distance,
    ) -> Result<()> {
        if self
            .qdrant
            .collection_exists(name)
            .await
            .context("checking if Qdrant collection exists")?
        {
            self.validate_collection_config(name, dense_size, distance).await
        } else {
            let mut dense_map = HashMap::new();
            dense_map.insert(
                DENSE_VECTOR_NAME.to_string(),
                VectorParams { size: dense_size, distance: distance.into(), ..Default::default() },
            );

            let mut sparse_map = HashMap::new();
            sparse_map.insert(
                SPARSE_VECTOR_NAME.to_string(),
                SparseVectorParams { index: None, modifier: Some(Modifier::Idf.into()) },
            );

            let request = CreateCollection {
                collection_name: name.to_string(),
                vectors_config: Some(VectorsConfig {
                    config: Some(VectorsConfigVariant::ParamsMap(VectorParamsMap {
                        map: dense_map,
                    })),
                }),
                sparse_vectors_config: Some(SparseVectorConfig { map: sparse_map }),
                ..Default::default()
            };

            match self.qdrant.create_collection(request).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    // Handle TOCTOU race: another caller may have created
                    // the collection between our exists check and create.
                    if self
                        .qdrant
                        .collection_exists(name)
                        .await
                        .context("rechecking Qdrant collection after create race")?
                    {
                        self.validate_collection_config(name, dense_size, distance).await
                    } else {
                        Err(e).with_context(|| format!("creating Qdrant collection '{name}'"))
                    }
                }
            }
        }
    }

    /// Check whether a Qdrant collection exists.
    pub async fn collection_exists(&self, name: &str) -> Result<bool> {
        self.qdrant.collection_exists(name).await.context("checking Qdrant collection existence")
    }

    /// Delete a Qdrant collection.
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        self.qdrant
            .delete_collection(name)
            .await
            .with_context(|| format!("deleting Qdrant collection '{name}'"))?;
        Ok(())
    }

    /// List all Qdrant collections by name.
    pub async fn list_collections(&self) -> Result<Vec<String>> {
        let response =
            self.qdrant.list_collections().await.context("listing Qdrant collections")?;
        Ok(response.collections.into_iter().map(|c| c.name).collect())
    }

    /// Upsert (insert or update) points into a collection.
    ///
    /// Waits for the write to be applied before returning so that subsequent
    /// reads are guaranteed to see the new points.
    pub async fn upsert_points(&self, collection: &str, points: Vec<PointStruct>) -> Result<()> {
        self.qdrant
            .upsert_points(UpsertPointsBuilder::new(collection, points).wait(true))
            .await
            .with_context(|| format!("upserting points into Qdrant collection '{collection}'"))?;
        Ok(())
    }

    /// Delete all points matching a `(tenant, document_id)` filter.
    pub async fn delete_document_points(
        &self,
        collection: &str,
        document_id: &str,
        tenant: &str,
    ) -> Result<()> {
        let filter = Filter::must([
            Condition::matches("tenant", tenant.to_string()),
            Condition::matches("document_id", document_id.to_string()),
        ]);

        self.qdrant
            .delete_points(DeletePointsBuilder::new(collection).points(filter).wait(true))
            .await
            .with_context(|| {
                format!(
                    "deleting points for document '{document_id}' \
                     (tenant '{tenant}') from collection '{collection}'"
                )
            })?;
        Ok(())
    }

    /// Dense vector similarity search with a tenant filter.
    ///
    /// Returns up to `limit` scored points ordered by descending similarity.
    pub async fn search_dense(
        &self,
        collection: &str,
        vector: Vec<f32>,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<ScoredPoint>> {
        let request = SearchPoints {
            collection_name: collection.to_string(),
            vector,
            filter: Some(Filter::must([Condition::matches("tenant", tenant.to_string())])),
            limit,
            vector_name: Some(DENSE_VECTOR_NAME.to_string()),
            with_payload: Some(WithPayloadSelector {
                selector_options: Some(SelectorOptions::Enable(true)),
            }),
            ..Default::default()
        };

        let response = self.qdrant.search_points(request).await.with_context(|| {
            format!(
                "searching dense vectors in collection '{collection}' \
                     for tenant '{tenant}'"
            )
        })?;

        Ok(response.result)
    }

    /// Sparse (BM25) vector search with a tenant filter.
    ///
    /// Returns up to `limit` scored points ordered by descending relevance.
    pub async fn search_sparse(
        &self,
        collection: &str,
        indices: Vec<u32>,
        values: Vec<f32>,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<ScoredPoint>> {
        if indices.is_empty() {
            return Ok(Vec::new());
        }

        let request = SearchPoints {
            collection_name: collection.to_string(),
            vector: values,
            sparse_indices: Some(SparseIndices { data: indices }),
            filter: Some(Filter::must([Condition::matches("tenant", tenant.to_string())])),
            limit,
            vector_name: Some(SPARSE_VECTOR_NAME.to_string()),
            with_payload: Some(WithPayloadSelector {
                selector_options: Some(SelectorOptions::Enable(true)),
            }),
            ..Default::default()
        };

        let response = self.qdrant.search_points(request).await.with_context(|| {
            format!(
                "searching sparse vectors in collection '{collection}' \
                     for tenant '{tenant}'"
            )
        })?;

        Ok(response.result)
    }
}
