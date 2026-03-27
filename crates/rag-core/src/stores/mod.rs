//! Storage layer for Postgres and Qdrant operations.
//!
//! This module provides the [`Stores`] handle that manages connections to both
//! Postgres (via sqlx) and Qdrant, running migrations on startup and exposing
//! tenant-scoped CRUD operations across submodules:
//!
//! - [`documents`]: Document metadata CRUD
//! - [`chunks`]: Chunk batch insert / query / delete
//! - [`corpus_stats`]: BM25 avgdl tracking
//! - [`vectors`]: Qdrant collection and point operations

pub mod chunks;
pub mod corpus_stats;
pub mod documents;
pub mod vectors;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::AppConfig;

/// Main storage handle for Postgres and Qdrant.
///
/// Created via [`Stores::new`], which connects to Postgres, runs pending
/// migrations, and builds a Qdrant client. All database operations are
/// implemented as `async` methods on this struct in the submodules.
pub struct Stores {
    pool: PgPool,
    qdrant: Arc<Qdrant>,
    /// The default Qdrant collection name from configuration.
    pub default_collection: String,
}

impl Stores {
    /// Create and initialise a `Stores` handle from [`AppConfig`].
    ///
    /// 1. Connects to Postgres with pool options from the config.
    /// 2. Runs all pending sqlx migrations (`crates/rag-core/migrations/`).
    /// 3. Builds a Qdrant gRPC client (with optional API key).
    ///
    /// # Errors
    ///
    /// Returns an error if Postgres is unreachable, migrations fail, or the
    /// Qdrant client cannot be constructed.
    #[allow(clippy::disallowed_methods)] // sqlx::migrate! internally uses .expect()
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        // -- Postgres --
        let pool = PgPoolOptions::new()
            .max_connections(cfg.postgres_max_connections)
            .acquire_timeout(Duration::from_secs(cfg.postgres_connect_timeout_secs))
            .connect(&cfg.postgres_url)
            .await
            .context("connecting to Postgres")?;

        sqlx::migrate!("./migrations").run(&pool).await.context("running database migrations")?;

        // -- Qdrant --
        let mut builder = Qdrant::from_url(&cfg.qdrant_url)
            .timeout(Duration::from_secs(cfg.qdrant_timeout_secs))
            .connect_timeout(Duration::from_secs(cfg.qdrant_connect_timeout_secs));

        if let Some(ref key) = cfg.qdrant_api_key {
            builder = builder.api_key(key.clone());
        }

        let qdrant = Arc::new(builder.build().context("building Qdrant client")?);

        Ok(Self { pool, qdrant, default_collection: cfg.default_collection.clone() })
    }

    /// Borrow the underlying Postgres connection pool.
    pub fn pg_pool(&self) -> &PgPool {
        &self.pool
    }

    /// Borrow the underlying Qdrant client.
    pub fn qdrant_client(&self) -> &Qdrant {
        &self.qdrant
    }
}
