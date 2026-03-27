//! BM25 corpus statistics tracking.
//!
//! The `corpus_stats` table maintains `(tenant, collection)` scoped counts of
//! documents and tokens. The `avgdl` column is a Postgres `GENERATED ALWAYS`
//! column computed as `total_tokens / total_docs`.

use anyhow::{Context, Result, anyhow};

use super::Stores;

/// Corpus statistics for a `(tenant, collection)` pair.
#[derive(Debug, Clone)]
pub struct CorpusStats {
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}

impl Stores {
    /// Update corpus statistics for a document's token count.
    ///
    /// Uses `stats_collection` and `stats_token_count` (columns managed
    /// exclusively by this method) to determine whether this is a first
    /// count, a re-ingest in the same collection, or a collection move.
    /// This is immune to callers setting `token_count` via `upsert_document`
    /// before calling this method.
    ///
    /// - **First count** (`stats_collection` is NULL): increments the target
    ///   collection's `total_docs` by 1 and `total_tokens` by the new count.
    /// - **Re-ingest** (`stats_collection` == target collection): adjusts
    ///   `total_tokens` by the difference (new − old) without touching
    ///   `total_docs`.
    /// - **Collection move** (`stats_collection` != target collection):
    ///   decrements the old collection and increments the new one.
    ///
    /// # Errors
    ///
    /// Returns an error if `document_id` does not exist in the `documents`
    /// table (call `upsert_document` first).
    pub async fn update_corpus_stats(
        &self,
        tenant: &str,
        collection: &str,
        document_id: &str,
        new_token_count: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("starting corpus stats update transaction")?;

        // Fetch the document's previous stats state. Both columns are only
        // written by this method, so they reflect what was actually counted
        // regardless of what upsert_document wrote to token_count. Lock the
        // row so concurrent updates for the same document serialize.
        let prev: Option<(Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT stats_token_count, stats_collection FROM documents \
             WHERE tenant = $1 AND id = $2 \
             FOR UPDATE",
        )
        .bind(tenant)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await
        .context("fetching document stats state for corpus stats delta")?;

        let (old_stats_tokens, old_stats_collection) = match prev {
            Some((tc, sc)) => (tc, sc),
            None => {
                return Err(anyhow!(
                    "document '{document_id}' (tenant '{tenant}') does not exist; \
                     call upsert_document before update_corpus_stats"
                ));
            }
        };

        // Decrement old collection if the document is moving.
        if let Some(ref old_coll) = old_stats_collection
            && old_coll != collection
        {
            let old_tokens = old_stats_tokens.unwrap_or(0);
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
            .bind(old_coll)
            .bind(old_tokens)
            .execute(&mut *tx)
            .await
            .context("decrementing old collection corpus stats")?;
        }

        // Compute delta for the target collection.
        let (docs_delta, tokens_delta) = match &old_stats_collection {
            Some(old_coll) if old_coll == collection => {
                // Re-ingest in same collection: adjust tokens only.
                let old = old_stats_tokens.unwrap_or(0);
                (0_i64, new_token_count - old)
            }
            _ => {
                // First count or collection move: full increment.
                (1_i64, new_token_count)
            }
        };

        sqlx::query(
            r#"
            INSERT INTO corpus_stats (tenant, collection, total_docs, total_tokens, updated_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (tenant, collection) DO UPDATE SET
                total_docs   = corpus_stats.total_docs + $3,
                total_tokens = corpus_stats.total_tokens + $4,
                updated_at   = now()
            "#,
        )
        .bind(tenant)
        .bind(collection)
        .bind(docs_delta)
        .bind(tokens_delta)
        .execute(&mut *tx)
        .await
        .context("updating corpus stats")?;

        // Update the document's stats columns and token_count so future
        // calls compute the correct delta.
        sqlx::query(
            "UPDATE documents \
             SET token_count = $3, stats_token_count = $3, stats_collection = $4 \
             WHERE tenant = $1 AND id = $2",
        )
        .bind(tenant)
        .bind(document_id)
        .bind(new_token_count)
        .bind(collection)
        .execute(&mut *tx)
        .await
        .context("updating document token_count, stats_token_count, and stats_collection")?;

        tx.commit().await.context("committing corpus stats update transaction")?;

        Ok(())
    }

    /// Fetch corpus statistics for a `(tenant, collection)` pair.
    ///
    /// If no row exists, returns a zero-count struct with `avgdl` set to
    /// `default_avgdl` (typically the configured `bm25_avgdl`).
    pub async fn get_corpus_stats(
        &self,
        tenant: &str,
        collection: &str,
        default_avgdl: f64,
    ) -> Result<CorpusStats> {
        let row: Option<(i64, i64, f64)> = sqlx::query_as(
            "SELECT total_docs, total_tokens, avgdl FROM corpus_stats \
             WHERE tenant = $1 AND collection = $2",
        )
        .bind(tenant)
        .bind(collection)
        .fetch_optional(&self.pool)
        .await
        .context("fetching corpus stats")?;

        match row {
            Some((total_docs, total_tokens, avgdl)) => {
                let effective_avgdl = if total_docs > 0 { avgdl } else { default_avgdl };
                Ok(CorpusStats { total_docs, total_tokens, avgdl: effective_avgdl })
            }
            None => Ok(CorpusStats { total_docs: 0, total_tokens: 0, avgdl: default_avgdl }),
        }
    }

    /// Convenience: return only the `avgdl` value for a collection.
    ///
    /// Falls back to `default_avgdl` when no stats row exists or
    /// `total_docs` is zero.
    pub async fn get_avgdl(
        &self,
        tenant: &str,
        collection: &str,
        default_avgdl: f64,
    ) -> Result<f64> {
        let stats = self.get_corpus_stats(tenant, collection, default_avgdl).await?;
        Ok(stats.avgdl)
    }
}
