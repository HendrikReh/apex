//! Document metadata CRUD operations (tenant-scoped).
//!
//! All queries filter on `(tenant, id)` as the composite primary key (ADR-002).

use anyhow::{Context, Result};

use super::Stores;

/// A row from the `documents` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DocumentRow {
    pub tenant: String,
    pub id: String,
    pub title: String,
    pub language: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub source_path: Option<String>,
    pub version: Option<String>,
    pub checksum: Option<String>,
    pub ingest_run_id: Option<uuid::Uuid>,
    pub token_count: Option<i64>,
    pub collection: Option<String>,
    pub stats_collection: Option<String>,
    pub stats_token_count: Option<i64>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Stores {
    /// Insert or update a document row.
    ///
    /// On conflict `(tenant, id)` all mutable columns are replaced with the
    /// new values (`created_at` is left untouched).
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_document(
        &self,
        tenant: &str,
        id: &str,
        title: &str,
        language: Option<&str>,
        metadata: Option<&serde_json::Value>,
        source_path: Option<&str>,
        version: Option<&str>,
        checksum: Option<&str>,
        ingest_run_id: Option<uuid::Uuid>,
        token_count: Option<i64>,
        collection: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO documents
                (tenant, id, title, language, metadata, source_path,
                 version, checksum, ingest_run_id, token_count, collection)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (tenant, id) DO UPDATE SET
                title         = EXCLUDED.title,
                language      = EXCLUDED.language,
                metadata      = EXCLUDED.metadata,
                source_path   = EXCLUDED.source_path,
                version       = EXCLUDED.version,
                checksum      = EXCLUDED.checksum,
                ingest_run_id = EXCLUDED.ingest_run_id,
                token_count   = EXCLUDED.token_count,
                collection    = EXCLUDED.collection,
                updated_at    = now()
            "#,
        )
        .bind(tenant)
        .bind(id)
        .bind(title)
        .bind(language)
        .bind(metadata)
        .bind(source_path)
        .bind(version)
        .bind(checksum)
        .bind(ingest_run_id)
        .bind(token_count)
        .bind(collection)
        .execute(&self.pool)
        .await
        .context("upserting document")?;

        Ok(())
    }

    /// Fetch a single document by `(tenant, id)`.
    ///
    /// Returns `None` if no matching row exists.
    pub async fn get_document(&self, tenant: &str, id: &str) -> Result<Option<DocumentRow>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT tenant, id, title, language, metadata, source_path, \
             version, checksum, ingest_run_id, token_count, collection, \
             stats_collection, stats_token_count, created_at, updated_at \
             FROM documents WHERE tenant = $1 AND id = $2",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("fetching document")?;

        Ok(row)
    }

    /// Delete a document by `(tenant, id)`.
    ///
    /// Returns `true` if a row was actually deleted. The `ON DELETE CASCADE`
    /// foreign key on `chunks` ensures associated chunks are removed as well.
    /// Corpus statistics are decremented if the document had a recorded
    /// `token_count` and `collection`.
    pub async fn delete_document(&self, tenant: &str, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await.context("starting delete_document transaction")?;

        // Lock the row and read stats columns atomically so a concurrent
        // update_corpus_stats cannot change them between our read and delete.
        let doc: Option<(Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT stats_token_count, stats_collection FROM documents \
             WHERE tenant = $1 AND id = $2 \
             FOR UPDATE",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .context("fetching document before delete for corpus stats adjustment")?;

        let result = sqlx::query("DELETE FROM documents WHERE tenant = $1 AND id = $2")
            .bind(tenant)
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("deleting document")?;

        let deleted = result.rows_affected() > 0;

        // Decrement corpus_stats if the document was previously counted.
        if deleted {
            if let Some((Some(token_count), Some(collection))) = doc {
                sqlx::query(
                    r#"
                    UPDATE corpus_stats
                    SET total_docs   = total_docs - 1,
                        total_tokens = total_tokens - $3,
                        updated_at   = now()
                    WHERE tenant = $1 AND collection = $2
                    "#,
                )
                .bind(tenant)
                .bind(&collection)
                .bind(token_count)
                .execute(&mut *tx)
                .await
                .context("decrementing corpus stats after document delete")?;
            }
        }

        tx.commit().await.context("committing delete_document transaction")?;

        Ok(deleted)
    }

    /// Retrieve only the checksum for a document.
    ///
    /// Useful for skip-if-unchanged checks during re-ingestion.
    /// Returns `None` if the document doesn't exist or has a NULL checksum.
    pub async fn get_document_checksum(&self, tenant: &str, id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT checksum FROM documents WHERE tenant = $1 AND id = $2")
                .bind(tenant)
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .context("fetching document checksum")?;

        Ok(row.and_then(|(c,)| c))
    }
}
