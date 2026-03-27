//! Embedding service abstractions and implementations.
//!
//! The trait is async from the start so batching, retries, network timeouts,
//! and mock backends all share the same call shape.

use std::ops::Range;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::types::embeddings::{CreateEmbeddingRequestArgs, EmbeddingInput};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use tiktoken_rs::{CoreBPE, get_bpe_from_model};

use crate::config::{AppConfig, EmbedderKind};

const MAX_EMBED_INPUT_TOKENS: usize = 8_192;

pub trait EmbedService: Send + Sync {
    fn dim(&self) -> usize;

    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;
}

pub enum AnyEmbedder {
    OpenAi(OpenAiEmbedder),
    Mock(MockEmbedder),
}

impl AnyEmbedder {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        match config.embedder {
            EmbedderKind::OpenAi => Ok(Self::OpenAi(OpenAiEmbedder::from_config(config)?)),
            EmbedderKind::Mock => Ok(Self::Mock(MockEmbedder::from_config(config)?)),
        }
    }
}

impl EmbedService for AnyEmbedder {
    fn dim(&self) -> usize {
        match self {
            Self::OpenAi(embedder) => embedder.dim(),
            Self::Mock(embedder) => embedder.dim(),
        }
    }

    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        match self {
            Self::OpenAi(embedder) => embedder.embed_batch(texts),
            Self::Mock(embedder) => embedder.embed_batch(texts),
        }
    }
}

pub struct OpenAiEmbedder {
    client: Client<OpenAIConfig>,
    model: String,
    dim: usize,
    max_retries: u32,
    retry_backoff: Duration,
    max_batch_tokens: usize,
    max_batch_size: usize,
    tokenizer: CoreBPE,
}

impl OpenAiEmbedder {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        Self::new(
            config.embedding_model.clone(),
            config.embed_timeout_secs,
            config.embed_max_retries,
            config.embed_retry_backoff_ms,
            config.embed_max_batch_tokens,
            config.embed_max_batch_size,
        )
    }

    pub fn new(
        model: String,
        timeout_secs: u64,
        max_retries: u32,
        retry_backoff_ms: u64,
        max_batch_tokens: usize,
        max_batch_size: usize,
    ) -> Result<Self> {
        if max_batch_tokens == 0 {
            bail!("embed_max_batch_tokens must be greater than zero");
        }
        if max_batch_size == 0 {
            bail!("embed_max_batch_size must be greater than zero");
        }

        let dim = embedding_dimension_for_model(&model)?;
        let tokenizer =
            get_bpe_from_model(&model).with_context(|| format!("loading tokenizer for {model}"))?;
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("building OpenAI HTTP client")?;
        let client = Client::with_config(OpenAIConfig::default()).with_http_client(http_client);

        Ok(Self {
            client,
            model,
            dim,
            max_retries,
            retry_backoff: Duration::from_millis(retry_backoff_ms),
            max_batch_tokens,
            max_batch_size,
            tokenizer,
        })
    }

    fn batch_ranges(&self, texts: &[String]) -> Result<Vec<Range<usize>>> {
        let token_counts = texts
            .iter()
            .enumerate()
            .map(|(index, text)| self.count_tokens(index, text))
            .collect::<Result<Vec<_>>>()?;

        pack_batch_ranges(&token_counts, self.max_batch_tokens, self.max_batch_size)
    }

    fn count_tokens(&self, index: usize, text: &str) -> Result<usize> {
        if text.is_empty() {
            bail!("embedding input at index {index} must not be empty");
        }

        let token_count = self.tokenizer.encode_ordinary(text).len();
        if token_count > MAX_EMBED_INPUT_TOKENS {
            bail!(
                "embedding input at index {index} exceeds {MAX_EMBED_INPUT_TOKENS} tokens ({token_count})"
            );
        }

        Ok(token_count)
    }

    async fn embed_one_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let request = CreateEmbeddingRequestArgs::default()
            .model(self.model.clone())
            .input(EmbeddingInput::StringArray(texts.to_vec()))
            .build()
            .context("building OpenAI embedding request")?;

        let mut attempt = 0_u32;
        loop {
            let result = self.client.embeddings().create(request.clone()).await;
            match result {
                Ok(response) => {
                    let mut data = response.data;
                    data.sort_by_key(|embedding| embedding.index);
                    return Ok(data.into_iter().map(|embedding| embedding.embedding).collect());
                }
                Err(error) if is_transient_openai_error(&error) && attempt < self.max_retries => {
                    let delay = exponential_backoff(self.retry_backoff, attempt);
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(error) => {
                    return Err(anyhow!(error)).with_context(|| {
                        format!(
                            "requesting embeddings from model {} after {} attempt(s)",
                            self.model,
                            attempt + 1
                        )
                    });
                }
            }
        }
    }
}

impl EmbedService for OpenAiEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }

            let ranges = self.batch_ranges(texts)?;
            let mut embeddings = Vec::with_capacity(texts.len());

            for range in ranges {
                let batch_embeddings = self.embed_one_batch(&texts[range]).await?;
                embeddings.extend(batch_embeddings);
            }

            Ok(embeddings)
        })
    }
}

pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        Ok(Self { dim: embedding_dimension_for_model(&config.embedding_model)? })
    }

    pub fn new(dim: usize) -> Result<Self> {
        if dim == 0 {
            bail!("mock embedder dimension must be greater than zero");
        }

        Ok(Self { dim })
    }

    fn embed_one(&self, text: &str) -> Vec<f32> {
        let digest = Sha256::digest(text.as_bytes());
        let mut state = u64::from_le_bytes(digest[..8].try_into().expect("sha256 length is fixed"));
        if state == 0 {
            state = 1;
        }

        let mut values = Vec::with_capacity(self.dim);
        for _ in 0..self.dim {
            state = xorshift64(state);
            let unit = (state as f64) / (u64::MAX as f64);
            values.push(((unit * 2.0) - 1.0) as f32);
        }

        normalize_vector(values)
    }
}

impl EmbedService for MockEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move { Ok(texts.iter().map(|text| self.embed_one(text)).collect()) })
    }
}

fn embedding_dimension_for_model(model: &str) -> Result<usize> {
    match model {
        "text-embedding-3-small" => Ok(1_536),
        "text-embedding-3-large" => Ok(3_072),
        other => bail!("unsupported embedding model {other:?}"),
    }
}

fn pack_batch_ranges(
    token_counts: &[usize],
    max_batch_tokens: usize,
    max_batch_size: usize,
) -> Result<Vec<Range<usize>>> {
    if max_batch_tokens == 0 {
        bail!("max_batch_tokens must be greater than zero");
    }
    if max_batch_size == 0 {
        bail!("max_batch_size must be greater than zero");
    }

    let mut ranges = Vec::new();
    let mut batch_start = 0_usize;
    let mut batch_len = 0_usize;
    let mut batch_tokens = 0_usize;

    for (index, token_count) in token_counts.iter().copied().enumerate() {
        if token_count == 0 {
            bail!("embedding input at index {index} produced zero tokens");
        }
        if token_count > max_batch_tokens {
            bail!(
                "embedding input at index {index} exceeds the batch token limit ({token_count} > {max_batch_tokens})"
            );
        }

        let would_exceed_tokens = batch_tokens + token_count > max_batch_tokens;
        let would_exceed_items = batch_len == max_batch_size;
        if batch_len > 0 && (would_exceed_tokens || would_exceed_items) {
            ranges.push(batch_start..index);
            batch_start = index;
            batch_len = 0;
            batch_tokens = 0;
        }

        batch_len += 1;
        batch_tokens += token_count;
    }

    if batch_len > 0 {
        ranges.push(batch_start..(batch_start + batch_len));
    }

    Ok(ranges)
}

fn is_transient_openai_error(error: &OpenAIError) -> bool {
    match error {
        OpenAIError::Reqwest(reqwest_error) => {
            reqwest_error.is_timeout()
                || reqwest_error.is_connect()
                || matches!(
                    reqwest_error.status().map(|status| status.as_u16()),
                    Some(429 | 500 | 502 | 503 | 504)
                )
        }
        OpenAIError::ApiError(api_error) => {
            let type_lower = api_error.r#type.as_deref().unwrap_or_default().to_ascii_lowercase();
            let code_lower = api_error.code.as_deref().unwrap_or_default().to_ascii_lowercase();
            let message_lower = api_error.message.to_ascii_lowercase();

            if type_lower.contains("authentication")
                || type_lower.contains("invalid_request")
                || type_lower.contains("not_found")
                || code_lower.contains("invalid_api_key")
                || code_lower.contains("model_not_found")
                || message_lower.contains("incorrect api key")
                || message_lower.contains("model not found")
            {
                return false;
            }

            type_lower.contains("server_error")
                || type_lower.contains("rate_limit")
                || code_lower.contains("rate_limit")
                || message_lower.contains("rate limit")
                || message_lower.contains("temporarily unavailable")
                || message_lower.contains("server had an error")
                || message_lower.contains("overloaded")
        }
        _ => false,
    }
}

fn exponential_backoff(base: Duration, attempt: u32) -> Duration {
    let multiplier = 1_u32.checked_shl(attempt.min(20)).unwrap_or(u32::MAX);
    base.saturating_mul(multiplier)
}

fn xorshift64(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

fn normalize_vector(mut values: Vec<f32>) -> Vec<f32> {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut values {
            *value /= norm;
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, EmbedderKind};

    #[tokio::test]
    async fn mock_embedder_is_deterministic() {
        let embedder = MockEmbedder::new(8).expect("mock embedder should build");
        let texts = vec!["alpha".to_string(), "beta".to_string(), "alpha".to_string()];

        let vectors = embedder.embed_batch(&texts).await.expect("mock embedding should succeed");

        assert_eq!(vectors.len(), 3);
        assert_eq!(vectors[0].len(), 8);
        assert_eq!(vectors[0], vectors[2]);
        assert_ne!(vectors[0], vectors[1]);
    }

    #[test]
    fn pack_batch_ranges_respects_token_and_item_limits() {
        let ranges = pack_batch_ranges(&[3, 4, 5, 2], 8, 2).expect("batch packing should succeed");
        assert_eq!(ranges, vec![0..2, 2..4]);
    }

    #[test]
    fn openai_embedder_rejects_unknown_models() {
        let err = match OpenAiEmbedder::new("unknown-model".to_string(), 30, 3, 500, 8_192, 32) {
            Ok(_) => panic!("unknown model should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unsupported embedding model"));
    }

    #[test]
    fn any_embedder_from_config_selects_mock() {
        let config = AppConfig {
            qdrant_url: "http://127.0.0.1:6334".to_string(),
            qdrant_api_key: None,
            qdrant_timeout_secs: 30,
            qdrant_connect_timeout_secs: 5,
            postgres_url: "postgres://postgres:postgres@127.0.0.1:5432/postgres".to_string(),
            postgres_max_connections: 10,
            postgres_connect_timeout_secs: 5,
            bm25_avgdl: 300.0,
            bm25_k1: 1.2,
            bm25_b: 0.75,
            bm25_query_b: 0.3,
            default_collection: "hybrid_docs".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            embedder: EmbedderKind::Mock,
            embed_timeout_secs: 30,
            embed_max_retries: 3,
            embed_retry_backoff_ms: 500,
            embed_max_batch_tokens: 8_192,
            embed_max_batch_size: 32,
            bind_addr: "0.0.0.0:8080".to_string(),
            auth_mode: AuthMode::None,
            tenant_header: "x-tenant".to_string(),
            request_id_header: "x-request-id".to_string(),
        };

        let embedder = AnyEmbedder::from_config(&config).expect("mock embedder should build");
        assert_eq!(embedder.dim(), 1_536);
    }
}
