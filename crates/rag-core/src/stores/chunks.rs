//! Chunk batch operations (tenant-scoped).
//!
//! Chunks are stored with a `(tenant, document_id, chunk_index)` unique
//! constraint. The foreign key to `documents` cascades deletes.

use anyhow::{Context, Result};

use super::Stores;

/// A row from the `chunks` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChunkRecord {
    pub id: i64,
    pub tenant: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
}

impl Stores {
    /// Insert (or update) chunks for a document, removing stale trailing chunks.
    ///
    /// Each chunk is assigned a zero-based `chunk_index`. On conflict the
    /// text is replaced. After upserting, any chunks with `chunk_index >=`
    /// the new chunk count are deleted so that re-ingestion with fewer chunks
    /// does not leave stale data behind.
    pub async fn insert_chunks(
        &self,
        tenant: &str,
        document_id: &str,
        chunks: &[String],
    ) -> Result<()> {
        for (idx, text) in chunks.iter().enumerate() {
            let chunk_index: i32 = idx.try_into().context("chunk index exceeds i32::MAX")?;

            sqlx::query(
                r#"
                INSERT INTO chunks (tenant, document_id, chunk_index, text)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (tenant, document_id, chunk_index)
                DO UPDATE SET text = EXCLUDED.text
                "#,
            )
            .bind(tenant)
            .bind(document_id)
            .bind(chunk_index)
            .bind(text)
            .execute(&self.pool)
            .await
            .with_context(|| format!("inserting chunk {chunk_index} for document {document_id}"))?;
        }

        // Remove stale chunks left over from a previous ingestion that
        // produced more chunks than the current one.
        let new_count: i32 = chunks.len().try_into().context("chunk count exceeds i32::MAX")?;
        sqlx::query(
            "DELETE FROM chunks WHERE tenant = $1 AND document_id = $2 AND chunk_index >= $3",
        )
        .bind(tenant)
        .bind(document_id)
        .bind(new_count)
        .execute(&self.pool)
        .await
        .with_context(|| {
            format!("deleting stale chunks (index >= {new_count}) for document {document_id}")
        })?;

        Ok(())
    }

    /// Fetch all chunks for a document, ordered by `chunk_index`.
    pub async fn get_chunks_by_document(
        &self,
        tenant: &str,
        document_id: &str,
    ) -> Result<Vec<ChunkRecord>> {
        let rows = sqlx::query_as::<_, ChunkRecord>(
            r#"
            SELECT id, tenant, document_id, chunk_index, text
            FROM chunks
            WHERE tenant = $1 AND document_id = $2
            ORDER BY chunk_index
            "#,
        )
        .bind(tenant)
        .bind(document_id)
        .fetch_all(&self.pool)
        .await
        .context("fetching chunks by document")?;

        Ok(rows)
    }

    /// Delete all chunks belonging to a document.
    ///
    /// Returns the number of rows deleted.
    pub async fn delete_document_chunks(&self, tenant: &str, document_id: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM chunks WHERE tenant = $1 AND document_id = $2")
            .bind(tenant)
            .bind(document_id)
            .execute(&self.pool)
            .await
            .context("deleting document chunks")?;

        Ok(result.rows_affected())
    }
}
