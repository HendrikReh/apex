//! Qdrant vector storage operations.
//!
//! Provides collection management (create / validate / list / delete) and
//! point-level operations (upsert, delete-by-filter, dense search) with
//! tenant isolation enforced via payload filters.

use anyhow::{Context, Result, anyhow};
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, DeletePointsBuilder, Distance, Filter, PointStruct,
    ScoredPoint, SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
    vectors_config::Config as VectorsConfigVariant,
};

use super::Stores;

impl Stores {
    /// Ensure a Qdrant collection exists with the expected vector parameters.
    ///
    /// * If the collection does **not** exist it is created with the given
    ///   `vector_size` and `distance` metric.
    /// * If the collection **does** exist its vector size is validated against
    ///   the expected value; a mismatch returns an error.
    pub async fn ensure_collection(
        &self,
        name: &str,
        vector_size: u64,
        distance: Distance,
    ) -> Result<()> {
        if self
            .qdrant
            .collection_exists(name)
            .await
            .context("checking if Qdrant collection exists")?
        {
            // Validate existing collection vector size.
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

            let (actual_size, actual_distance) = match &vectors_config.config {
                Some(VectorsConfigVariant::Params(p)) => (p.size, p.distance()),
                Some(VectorsConfigVariant::ParamsMap(_)) => {
                    return Err(anyhow!(
                        "collection '{name}' uses named vectors, which is incompatible \
                         with this store's unnamed-vector operations"
                    ));
                }
                None => {
                    return Err(anyhow!("collection '{name}' has no vector params configured"));
                }
            };

            if actual_size != vector_size {
                return Err(anyhow!(
                    "collection '{name}' has vector size {actual_size}, expected {vector_size}"
                ));
            }

            if actual_distance != distance {
                return Err(anyhow!(
                    "collection '{name}' uses distance metric {actual_distance:?}, \
                     expected {distance:?}"
                ));
            }

            Ok(())
        } else {
            let builder = CreateCollectionBuilder::new(name)
                .vectors_config(VectorParamsBuilder::new(vector_size, distance));
            self.qdrant
                .create_collection(builder)
                .await
                .with_context(|| format!("creating Qdrant collection '{name}'"))?;
            Ok(())
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
        let request = SearchPointsBuilder::new(collection, vector, limit)
            .filter(Filter::must([Condition::matches("tenant", tenant.to_string())]));

        let response = self.qdrant.search_points(request).await.with_context(|| {
            format!(
                "searching dense vectors in collection '{collection}' \
                     for tenant '{tenant}'"
            )
        })?;

        Ok(response.result)
    }
}
