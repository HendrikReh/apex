//! Application configuration with TOML file + environment variable overrides.
//!
//! Loading order (highest precedence first):
//! 1. Environment variables
//! 2. `config/app.toml` (or path from `APP_CONFIG_PATH`)
//! 3. Hardcoded defaults

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Which embedding backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedderKind {
    OpenAi,
    Mock,
}

impl FromStr for EmbedderKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::OpenAi),
            "mock" => Ok(Self::Mock),
            other => Err(anyhow::anyhow!("unknown embedder kind: {other:?}")),
        }
    }
}

impl fmt::Display for EmbedderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAi => f.write_str("openai"),
            Self::Mock => f.write_str("mock"),
        }
    }
}

/// Authentication mode for the HTTP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    None,
    ApiKey,
    Oidc,
}

impl FromStr for AuthMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "api-key" => Ok(Self::ApiKey),
            "oidc" => Ok(Self::Oidc),
            other => Err(anyhow::anyhow!("unknown auth mode: {other:?}")),
        }
    }
}

impl fmt::Display for AuthMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::ApiKey => f.write_str("api-key"),
            Self::Oidc => f.write_str("oidc"),
        }
    }
}

// ---------------------------------------------------------------------------
// TOML intermediate structs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct AppSettings {
    app: Option<AppSection>,
}

#[derive(Deserialize, Default)]
struct AppSection {
    qdrant_url: Option<String>,
    qdrant_api_key: Option<String>,
    qdrant_timeout_secs: Option<u64>,
    qdrant_connect_timeout_secs: Option<u64>,
    postgres_url: Option<String>,
    postgres_max_connections: Option<u32>,
    postgres_connect_timeout_secs: Option<u64>,
    bm25_avgdl: Option<f32>,
    bm25_k1: Option<f32>,
    bm25_b: Option<f32>,
    bm25_query_b: Option<f32>,
    default_collection: Option<String>,
    embedding_model: Option<String>,
    embedder: Option<String>,
    embed_timeout_secs: Option<u64>,
    embed_max_retries: Option<u32>,
    embed_retry_backoff_ms: Option<u64>,
    embed_max_batch_tokens: Option<usize>,
    embed_max_batch_size: Option<usize>,
    bind_addr: Option<String>,
    auth_mode: Option<String>,
    tenant_header: Option<String>,
    request_id_header: Option<String>,
}

// ---------------------------------------------------------------------------
// AppConfig
// ---------------------------------------------------------------------------

/// Top-level application configuration.
///
/// Constructed via [`AppConfig::from_env`], which merges values from a TOML
/// file (`config/app.toml` by default) with environment variable overrides.
#[derive(Debug, Clone)]
pub struct AppConfig {
    // Qdrant
    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_timeout_secs: u64,
    pub qdrant_connect_timeout_secs: u64,
    // Postgres
    pub postgres_url: String,
    pub postgres_max_connections: u32,
    pub postgres_connect_timeout_secs: u64,
    // BM25
    pub bm25_avgdl: f32,
    pub bm25_k1: f32,
    pub bm25_b: f32,
    pub bm25_query_b: f32,
    // Collections
    pub default_collection: String,
    // Embedding
    pub embedding_model: String,
    pub embedder: EmbedderKind,
    pub embed_timeout_secs: u64,
    pub embed_max_retries: u32,
    pub embed_retry_backoff_ms: u64,
    pub embed_max_batch_tokens: usize,
    pub embed_max_batch_size: usize,
    // Server
    pub bind_addr: String,
    // Auth
    pub auth_mode: AuthMode,
    pub tenant_header: String,
    pub request_id_header: String,
}

impl AppConfig {
    /// Load configuration by merging (in order of precedence):
    /// 1. Environment variables
    /// 2. TOML file at `APP_CONFIG_PATH` (default `config/app.toml`)
    /// 3. Hardcoded defaults
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML file exists but cannot be parsed.
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::from_current_env()
    }

    /// Build config from the current process environment and TOML file,
    /// without loading `.env`. Useful in tests where the environment is
    /// controlled explicitly.
    fn from_current_env() -> Result<Self> {
        let config_path =
            std::env::var("APP_CONFIG_PATH").unwrap_or_else(|_| "config/app.toml".to_owned());

        let file_settings = if std::path::Path::new(&config_path).exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading config file {config_path}"))?;
            let settings: AppSettings = toml::from_str(&contents)
                .with_context(|| format!("parsing config file {config_path}"))?;
            settings.app.unwrap_or_default()
        } else {
            AppSection::default()
        };

        let f = &file_settings;

        // Helper: read env var as a parsed value. Returns Err if the var
        // is set but cannot be parsed (catches operator typos).
        fn env_parsed<T: FromStr>(name: &str) -> Result<Option<T>>
        where
            T::Err: std::fmt::Display,
        {
            match std::env::var(name) {
                Ok(v) if !v.is_empty() => {
                    let parsed = v.parse::<T>().map_err(|e| {
                        anyhow::anyhow!("invalid value for env var {name}={v:?}: {e}")
                    })?;
                    Ok(Some(parsed))
                }
                _ => Ok(None),
            }
        }

        fn env_string(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|v| !v.is_empty())
        }

        let qdrant_url = env_string("QDRANT_URL")
            .or_else(|| f.qdrant_url.clone())
            .unwrap_or_else(|| "http://127.0.0.1:6334".to_owned());

        let qdrant_api_key = env_string("QDRANT_API_KEY").or_else(|| f.qdrant_api_key.clone());

        let qdrant_timeout_secs =
            env_parsed("QDRANT_TIMEOUT_SECS")?.or(f.qdrant_timeout_secs).unwrap_or(30);

        let qdrant_connect_timeout_secs = env_parsed("QDRANT_CONNECT_TIMEOUT_SECS")?
            .or(f.qdrant_connect_timeout_secs)
            .unwrap_or(5);

        let postgres_url = env_string("DATABASE_URL")
            .or_else(|| f.postgres_url.clone())
            .unwrap_or_else(|| "postgres://postgres:postgres@127.0.0.1:5432/postgres".to_owned());

        let postgres_max_connections =
            env_parsed("POSTGRES_MAX_CONNECTIONS")?.or(f.postgres_max_connections).unwrap_or(10);

        let postgres_connect_timeout_secs = env_parsed("POSTGRES_CONNECT_TIMEOUT_SECS")?
            .or(f.postgres_connect_timeout_secs)
            .unwrap_or(5);

        let bm25_avgdl = env_parsed("BM25_AVGDL")?.or(f.bm25_avgdl).unwrap_or(300.0);

        let bm25_k1 = env_parsed("BM25_K1")?.or(f.bm25_k1).unwrap_or(1.2);

        let bm25_b = env_parsed("BM25_B")?.or(f.bm25_b).unwrap_or(0.75);

        let bm25_query_b = env_parsed("BM25_QUERY_B")?.or(f.bm25_query_b).unwrap_or(0.3);

        let default_collection = env_string("DEFAULT_COLLECTION")
            .or_else(|| f.default_collection.clone())
            .unwrap_or_else(|| "hybrid_docs".to_owned());

        let embedding_model = env_string("EMBEDDING_MODEL")
            .or_else(|| f.embedding_model.clone())
            .unwrap_or_else(|| "text-embedding-3-small".to_owned());

        let embedder_str = env_string("RAG_EMBEDDER")
            .or_else(|| f.embedder.clone())
            .unwrap_or_else(|| "openai".to_owned());
        let embedder = embedder_str
            .parse::<EmbedderKind>()
            .with_context(|| format!("parsing embedder from {embedder_str:?}"))?;

        let embed_timeout_secs =
            env_parsed("EMBED_TIMEOUT_SECS")?.or(f.embed_timeout_secs).unwrap_or(30);

        let embed_max_retries =
            env_parsed("EMBED_MAX_RETRIES")?.or(f.embed_max_retries).unwrap_or(3);
        let embed_retry_backoff_ms =
            env_parsed("EMBED_RETRY_BACKOFF_MS")?.or(f.embed_retry_backoff_ms).unwrap_or(500);
        let embed_max_batch_tokens =
            env_parsed("EMBED_MAX_BATCH_TOKENS")?.or(f.embed_max_batch_tokens).unwrap_or(8_192);
        let embed_max_batch_size =
            env_parsed("EMBED_MAX_BATCH_SIZE")?.or(f.embed_max_batch_size).unwrap_or(32);

        let bind_addr = env_string("BIND_ADDR")
            .or_else(|| f.bind_addr.clone())
            .unwrap_or_else(|| "0.0.0.0:8080".to_owned());

        let auth_mode_str = env_string("AUTH_MODE")
            .or_else(|| f.auth_mode.clone())
            .unwrap_or_else(|| "none".to_owned());
        let auth_mode = auth_mode_str
            .parse::<AuthMode>()
            .with_context(|| format!("parsing auth_mode from {auth_mode_str:?}"))?;

        let tenant_header = env_string("TENANT_HEADER")
            .or_else(|| f.tenant_header.clone())
            .unwrap_or_else(|| "x-tenant".to_owned());

        let request_id_header = env_string("REQUEST_ID_HEADER")
            .or_else(|| f.request_id_header.clone())
            .unwrap_or_else(|| "x-request-id".to_owned());

        Ok(Self {
            qdrant_url,
            qdrant_api_key,
            qdrant_timeout_secs,
            qdrant_connect_timeout_secs,
            postgres_url,
            postgres_max_connections,
            postgres_connect_timeout_secs,
            bm25_avgdl,
            bm25_k1,
            bm25_b,
            bm25_query_b,
            default_collection,
            embedding_model,
            embedder,
            embed_timeout_secs,
            embed_max_retries,
            embed_retry_backoff_ms,
            embed_max_batch_tokens,
            embed_max_batch_size,
            bind_addr,
            auth_mode,
            tenant_header,
            request_id_header,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Clear all config-relevant env vars so tests get deterministic defaults.
    ///
    /// # Safety
    ///
    /// Must only be called from single-threaded test contexts. Environment
    /// variable mutation is inherently racy across threads.
    unsafe fn clear_config_env() {
        unsafe {
            std::env::set_var("APP_CONFIG_PATH", "/tmp/nonexistent-apex-config.toml");
            std::env::remove_var("DATABASE_URL");
            std::env::remove_var("QDRANT_URL");
            std::env::remove_var("QDRANT_API_KEY");
            std::env::remove_var("QDRANT_TIMEOUT_SECS");
            std::env::remove_var("QDRANT_CONNECT_TIMEOUT_SECS");
            std::env::remove_var("POSTGRES_MAX_CONNECTIONS");
            std::env::remove_var("POSTGRES_CONNECT_TIMEOUT_SECS");
            std::env::remove_var("BM25_AVGDL");
            std::env::remove_var("BM25_K1");
            std::env::remove_var("BM25_B");
            std::env::remove_var("BM25_QUERY_B");
            std::env::remove_var("DEFAULT_COLLECTION");
            std::env::remove_var("EMBEDDING_MODEL");
            std::env::remove_var("RAG_EMBEDDER");
            std::env::remove_var("EMBED_TIMEOUT_SECS");
            std::env::remove_var("EMBED_MAX_RETRIES");
            std::env::remove_var("EMBED_RETRY_BACKOFF_MS");
            std::env::remove_var("EMBED_MAX_BATCH_TOKENS");
            std::env::remove_var("EMBED_MAX_BATCH_SIZE");
            std::env::remove_var("BIND_ADDR");
            std::env::remove_var("AUTH_MODE");
            std::env::remove_var("TENANT_HEADER");
            std::env::remove_var("REQUEST_ID_HEADER");
        }
    }

    /// Tests that touch environment variables are combined into a single
    /// test function to avoid races (cargo runs tests in parallel threads
    /// within the same process, sharing the environment).
    #[test]
    fn from_env_defaults_and_override() {
        // SAFETY: test-only env manipulation; single logical test avoids races.
        unsafe { clear_config_env() };

        // -- Part 1: verify all defaults --
        let cfg = match AppConfig::from_current_env() {
            Ok(c) => c,
            Err(e) => panic!("from_env failed: {e}"),
        };

        assert_eq!(cfg.qdrant_url, "http://127.0.0.1:6334");
        assert!(cfg.qdrant_api_key.is_none());
        assert_eq!(cfg.qdrant_timeout_secs, 30);
        assert_eq!(cfg.qdrant_connect_timeout_secs, 5);
        assert_eq!(cfg.postgres_url, "postgres://postgres:postgres@127.0.0.1:5432/postgres");
        assert_eq!(cfg.postgres_max_connections, 10);
        assert_eq!(cfg.postgres_connect_timeout_secs, 5);
        assert!((cfg.bm25_avgdl - 300.0).abs() < f32::EPSILON);
        assert!((cfg.bm25_k1 - 1.2).abs() < f32::EPSILON);
        assert!((cfg.bm25_b - 0.75).abs() < f32::EPSILON);
        assert!((cfg.bm25_query_b - 0.3).abs() < f32::EPSILON);
        assert_eq!(cfg.default_collection, "hybrid_docs");
        assert_eq!(cfg.embedding_model, "text-embedding-3-small");
        assert_eq!(cfg.embedder, EmbedderKind::OpenAi);
        assert_eq!(cfg.embed_timeout_secs, 30);
        assert_eq!(cfg.embed_max_retries, 3);
        assert_eq!(cfg.embed_retry_backoff_ms, 500);
        assert_eq!(cfg.embed_max_batch_tokens, 8_192);
        assert_eq!(cfg.embed_max_batch_size, 32);
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");
        assert_eq!(cfg.auth_mode, AuthMode::None);
        assert_eq!(cfg.tenant_header, "x-tenant");
        assert_eq!(cfg.request_id_header, "x-request-id");

        // -- Part 2: env var overrides default --
        unsafe { std::env::set_var("DATABASE_URL", "postgres://custom:pw@db:5432/mydb") };

        let cfg = match AppConfig::from_current_env() {
            Ok(c) => c,
            Err(e) => panic!("from_env failed: {e}"),
        };
        assert_eq!(cfg.postgres_url, "postgres://custom:pw@db:5432/mydb");

        // Clean up.
        unsafe { std::env::remove_var("DATABASE_URL") };
    }

    #[test]
    fn embedder_kind_parsing() {
        assert_eq!("openai".parse::<EmbedderKind>().ok(), Some(EmbedderKind::OpenAi));
        assert_eq!("mock".parse::<EmbedderKind>().ok(), Some(EmbedderKind::Mock));
        // Case-insensitive.
        assert_eq!("OPENAI".parse::<EmbedderKind>().ok(), Some(EmbedderKind::OpenAi));
        assert_eq!("Mock".parse::<EmbedderKind>().ok(), Some(EmbedderKind::Mock));
        // Invalid value.
        assert!("invalid".parse::<EmbedderKind>().is_err());
    }

    #[test]
    fn auth_mode_parsing() {
        assert_eq!("none".parse::<AuthMode>().ok(), Some(AuthMode::None));
        assert_eq!("api-key".parse::<AuthMode>().ok(), Some(AuthMode::ApiKey));
        assert_eq!("oidc".parse::<AuthMode>().ok(), Some(AuthMode::Oidc));
        // Case-insensitive.
        assert_eq!("NONE".parse::<AuthMode>().ok(), Some(AuthMode::None));
        assert_eq!("API-KEY".parse::<AuthMode>().ok(), Some(AuthMode::ApiKey));
        assert_eq!("OIDC".parse::<AuthMode>().ok(), Some(AuthMode::Oidc));
        // Invalid value.
        assert!("invalid".parse::<AuthMode>().is_err());
    }
}
