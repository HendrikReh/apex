//! LLM backend abstraction with provider-agnostic completion API.
//!
//! Supports OpenAI-compatible endpoints (OpenAI, xAI, Groq, Ollama, etc.)
//! and native Anthropic Messages API.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_openai::config::{Config as OpenAiConfigTrait, OpenAIConfig};
use async_openai::error::OpenAIError;
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequest, CreateChatCompletionRequestArgs,
};
use reqwest::header::{AUTHORIZATION, HeaderMap};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, LlmProvider};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Role for messages in a completion request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

/// A single message in a completion request.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// Request for LLM completion.
pub struct CompletionRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [ChatMessage],
    pub temperature: f32,
    pub max_tokens: u32,
    pub stop: Vec<String>,
}

/// Response from LLM completion.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub usage: TokenUsage,
    pub model: String,
}

/// Token usage from LLM response.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// Message coalescing
// ---------------------------------------------------------------------------

/// Coalesce consecutive messages with the same role by joining with newline.
/// Required for Anthropic (strict alternation), harmless for OpenAI-compatible.
pub fn coalesce_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut result: Vec<ChatMessage> = Vec::new();
    for msg in messages {
        if let Some(last) = result.last_mut()
            && last.role == msg.role
        {
            last.content.push('\n');
            last.content.push_str(&msg.content);
            continue;
        }
        result.push(msg.clone());
    }
    result
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// LLM provider backend.
pub enum ChatBackend {
    OpenAiCompatible {
        client: Box<async_openai::Client<Box<dyn OpenAiConfigTrait>>>,
        model: String,
    },
    Anthropic {
        client: reqwest::Client,
        model: String,
        base_url: String,
    },
}

#[derive(Clone, Debug)]
struct OpenAiCompatibleConfig {
    api_base: String,
    api_key: SecretString,
}

impl OpenAiCompatibleConfig {
    fn new(api_base: String, api_key: Option<&str>) -> Self {
        Self { api_base, api_key: SecretString::from(api_key.unwrap_or_default().to_owned()) }
    }
}

impl OpenAiConfigTrait for OpenAiCompatibleConfig {
    // Trait signature returns `HeaderMap` (not `Result`), so we cannot propagate
    // errors here.  The API key is validated in `ChatBackend::from_config()`
    // before this config is constructed, so the `.expect()` is unreachable in
    // practice.
    #[allow(clippy::disallowed_methods)]
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let api_key = self.api_key.expose_secret();
        if !api_key.is_empty() {
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {api_key}")
                    .parse()
                    .expect("API key validated in from_config"),
            );
        }
        headers
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        vec![]
    }

    fn api_base(&self) -> &str {
        &self.api_base
    }

    fn api_key(&self) -> &SecretString {
        &self.api_key
    }
}

impl ChatBackend {
    /// Construct a backend from application config.
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        match config.llm_provider {
            LlmProvider::OpenAiCompatible => {
                if let Some(ref key) = config.llm_api_key {
                    validate_api_key_header_safe(key.expose_secret())?;
                }
                let oai_config: Box<dyn OpenAiConfigTrait> = match &config.llm_api_key {
                    Some(api_key) => Box::new(
                        OpenAIConfig::new()
                            .with_api_key(api_key.expose_secret())
                            .with_api_base(&config.llm_base_url),
                    ),
                    None => {
                        Box::new(OpenAiCompatibleConfig::new(config.llm_base_url.clone(), None))
                    }
                };
                let client = build_openai_client(oai_config, config.llm_timeout_secs)?;
                Ok(Self::OpenAiCompatible { client, model: config.llm_model.clone() })
            }
            LlmProvider::Anthropic => {
                let api_key_secret = config
                    .llm_api_key
                    .as_ref()
                    .ok_or_else(|| anyhow!("LLM_API_KEY must be set for anthropic"))?;
                let api_key: &str = api_key_secret.expose_secret();
                validate_api_key_header_safe(api_key)?;
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(config.llm_timeout_secs))
                    .default_headers({
                        let mut headers = reqwest::header::HeaderMap::new();
                        headers.insert(
                            "x-api-key",
                            api_key.parse().context("invalid API key header value")?,
                        );
                        headers.insert(
                            "anthropic-version",
                            "2023-06-01".parse().context("invalid anthropic-version header")?,
                        );
                        headers.insert(
                            reqwest::header::CONTENT_TYPE,
                            "application/json".parse().context("invalid content-type header")?,
                        );
                        headers
                    })
                    .build()
                    .context("building Anthropic HTTP client")?;
                Ok(Self::Anthropic {
                    client,
                    model: config.llm_model.clone(),
                    base_url: config.llm_base_url.clone(),
                })
            }
        }
    }

    /// Send a completion request to the configured LLM provider with retry.
    ///
    /// Retries on transient HTTP errors (429, 500, 502, 503) with exponential
    /// backoff. Non-retryable errors propagate immediately.
    pub async fn complete(
        &self,
        request: &CompletionRequest<'_>,
        max_retries: u32,
        retry_backoff_ms: u64,
    ) -> Result<LlmResponse> {
        let messages = coalesce_messages(request.messages);
        let mut last_err = None;
        let provider_name = match self {
            Self::OpenAiCompatible { .. } => "OpenAI-compatible completion",
            Self::Anthropic { .. } => "Anthropic completion",
        };

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let backoff = retry_delay(attempt, retry_backoff_ms);
                tokio::time::sleep(backoff).await;
            }

            let result = match self {
                Self::OpenAiCompatible { client, model } => {
                    complete_openai(client, model, request, &messages).await
                }
                Self::Anthropic { client, model, base_url } => {
                    complete_anthropic(client, model, base_url, request, &messages).await
                }
            };

            match result {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if is_transient_error(&e) && attempt < max_retries {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e).context(provider_name);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("retry loop exhausted"))).context(provider_name)
    }
}

/// Validate that an API key can be used as an HTTP header value.
/// Called at config time so downstream `.expect()` in trait impls is unreachable.
fn validate_api_key_header_safe(key: &str) -> Result<()> {
    key.parse::<reqwest::header::HeaderValue>()
        .map(|_| ())
        .map_err(|_| anyhow!("LLM_API_KEY contains characters invalid for an HTTP header value"))
}

fn retry_delay(attempt: u32, retry_backoff_ms: u64) -> Duration {
    let shift = (attempt - 1).min(63);
    let multiplier = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    Duration::from_millis(retry_backoff_ms.saturating_mul(multiplier))
}

fn build_openai_client(
    config: Box<dyn OpenAiConfigTrait>,
    timeout_secs: u64,
) -> Result<Box<async_openai::Client<Box<dyn OpenAiConfigTrait>>>> {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .context("building OpenAI HTTP client")?;

    // Disable async-openai's internal retries so the outer retry loop remains
    // the single source of truth for llm_max_retries.
    Ok(Box::new(async_openai::Client::build(http_client, config, build_openai_backoff())))
}

fn build_openai_backoff() -> backoff::ExponentialBackoff {
    let mut backoff = backoff::ExponentialBackoffBuilder::new();
    backoff.with_max_elapsed_time(Some(Duration::ZERO));
    backoff.build()
}

/// Check if an error is transient (retryable): 429, 500, 502, 503, 504.
///
/// Anthropic errors are formatted as `"Anthropic API error ({status}): ..."`,
/// where `{status}` includes both the numeric code and reason phrase
/// (for example `"429 Too Many Requests"`). Extract the numeric code from the
/// first parenthesized segment so transient errors still retry.
fn is_transient_error(err: &anyhow::Error) -> bool {
    if is_transient_openai_error(err) {
        return true;
    }

    err.chain().any(|cause| {
        let msg = cause.to_string();
        extract_status_code(&msg)
            .is_some_and(|code| matches!(code, 429 | 500 | 502 | 503 | 504 | 529))
    })
}

fn is_transient_openai_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let Some(openai_err) = cause.downcast_ref::<OpenAIError>() else {
            return false;
        };

        match openai_err {
            OpenAIError::Reqwest(_) => return true,
            OpenAIError::ApiError(api_error) => {
                let is_rate_limited =
                    matches!(api_error.code.as_deref(), Some("rate_limit_exceeded"))
                        || matches!(api_error.r#type.as_deref(), Some("rate_limit_exceeded"));
                let is_server_error =
                    matches!(api_error.code.as_deref(), Some("server_error"))
                        || matches!(api_error.r#type.as_deref(), Some("server_error"));
                let is_quota_error =
                    matches!(api_error.code.as_deref(), Some("insufficient_quota"))
                        || matches!(api_error.r#type.as_deref(), Some("insufficient_quota"));

                return (is_rate_limited && !is_quota_error) || is_server_error;
            }
            _ => {}
        }

        false
    })
}

fn extract_status_code(message: &str) -> Option<u16> {
    let start = message.find('(')?;
    let rest = &message[start + 1..];
    let end = rest.find(')')?;
    let token = rest[..end].split_whitespace().next()?;
    token.parse().ok()
}

// ---------------------------------------------------------------------------
// OpenAI-compatible implementation
// ---------------------------------------------------------------------------

async fn complete_openai(
    client: &async_openai::Client<Box<dyn OpenAiConfigTrait>>,
    model: &str,
    request: &CompletionRequest<'_>,
    messages: &[ChatMessage],
) -> Result<LlmResponse> {
    let oai_request = build_openai_request(model, request, messages)?;
    let response = client.chat().create(oai_request).await?;

    let choice = response.choices.first().ok_or_else(|| anyhow!("OpenAI returned no choices"))?;
    let text = choice.message.content.clone().unwrap_or_default();

    let usage = response.usage.map_or(TokenUsage::default(), |u| TokenUsage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
    });

    Ok(LlmResponse { text, usage, model: response.model })
}

fn build_openai_request(
    model: &str,
    request: &CompletionRequest<'_>,
    messages: &[ChatMessage],
) -> Result<CreateChatCompletionRequest> {
    let mut oai_messages: Vec<ChatCompletionRequestMessage> = Vec::new();

    if !request.system.is_empty() {
        oai_messages.push(
            ChatCompletionRequestSystemMessageArgs::default()
                .content(request.system)
                .build()
                .context("building system message")?
                .into(),
        );
    }

    for msg in messages {
        let oai_msg = match msg.role {
            ChatRole::User => ChatCompletionRequestUserMessageArgs::default()
                .content(msg.content.as_str())
                .build()
                .context("building user message")?
                .into(),
            ChatRole::Assistant => ChatCompletionRequestAssistantMessageArgs::default()
                .content(msg.content.as_str())
                .build()
                .context("building assistant message")?
                .into(),
        };
        oai_messages.push(oai_msg);
    }

    let mut req_builder = CreateChatCompletionRequestArgs::default();
    req_builder
        .model(model)
        .messages(oai_messages)
        .temperature(request.temperature)
        .max_tokens(request.max_tokens);

    if !request.stop.is_empty() {
        req_builder.stop(request.stop.clone());
    }

    req_builder.build().context("building OpenAI request")
}

// ---------------------------------------------------------------------------
// Anthropic implementation
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    system: &'a str,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    model: String,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicContent {
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Deserialize)]
struct AnthropicError {
    error: AnthropicErrorBody,
}

#[derive(Deserialize)]
struct AnthropicErrorBody {
    message: String,
}

async fn complete_anthropic(
    client: &reqwest::Client,
    model: &str,
    base_url: &str,
    request: &CompletionRequest<'_>,
    messages: &[ChatMessage],
) -> Result<LlmResponse> {
    // Anthropic requires at least one message. For system-only prompts
    // (empty messages), synthesize a minimal user turn so the API accepts it.
    let mut anthropic_messages: Vec<AnthropicMessage> = messages
        .iter()
        .map(|m| AnthropicMessage {
            role: match m.role {
                ChatRole::User => "user".to_owned(),
                ChatRole::Assistant => "assistant".to_owned(),
            },
            content: m.content.clone(),
        })
        .collect();
    if anthropic_messages.is_empty() {
        anthropic_messages
            .push(AnthropicMessage { role: "user".to_owned(), content: ".".to_owned() });
    }

    let temperature = if request.temperature > 1.0 {
        tracing::warn!(
            requested = request.temperature,
            clamped = 1.0,
            "Anthropic max temperature is 1.0; clamping"
        );
        1.0
    } else {
        request.temperature
    };

    let body = AnthropicRequest {
        model,
        max_tokens: request.max_tokens,
        system: request.system,
        messages: anthropic_messages,
        temperature: Some(temperature),
        stop_sequences: request.stop.clone(),
    };

    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    let url = format!("{base}/v1/messages");
    let http_response =
        client.post(&url).json(&body).send().await.context("Anthropic HTTP request")?;

    let status = http_response.status();
    if !status.is_success() {
        let body_text = http_response.text().await.unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<AnthropicError>(&body_text) {
            bail!("Anthropic API error ({}): {}", status, err.error.message);
        }
        bail!("Anthropic API error ({}): {}", status, body_text);
    }

    let response: AnthropicResponse =
        http_response.json().await.context("parsing Anthropic response")?;

    let text = response.content.into_iter().filter_map(|c| c.text).collect::<Vec<_>>().join("");

    Ok(LlmResponse {
        text,
        usage: TokenUsage {
            prompt_tokens: response.usage.input_tokens,
            completion_tokens: response.usage.output_tokens,
        },
        model: response.model,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthMode;
    use backoff::backoff::Backoff;

    fn test_config() -> AppConfig {
        AppConfig {
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
            chunking_max_tokens: 600,
            chunking_overlap_ratio: 0.15,
            embedding_model: "text-embedding-3-small".to_string(),
            embedder: crate::config::EmbedderKind::Mock,
            embed_timeout_secs: 30,
            embed_max_retries: 3,
            embed_retry_backoff_ms: 500,
            embed_max_batch_tokens: 8_192,
            embed_max_batch_size: 32,
            bind_addr: "0.0.0.0:8080".to_string(),
            auth_mode: AuthMode::None,
            tenant_header: "x-tenant".to_string(),
            request_id_header: "x-request-id".to_string(),
            rrf_k: 60,
            dense_top_k: 20,
            sparse_top_k: 20,
            context_max_tokens: 8000,
            context_max_chunks: 50,
            llm_provider: LlmProvider::OpenAiCompatible,
            llm_api_key: None,
            llm_model: "gpt-4o".to_string(),
            llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
            llm_temperature: 0.1,
            llm_max_tokens: 4096,
            llm_timeout_secs: 60,
            llm_max_retries: 3,
            llm_retry_backoff_ms: 500,
            llm_prompt_template_path: "prompts/chat_system.hbs".to_string(),
        }
    }

    #[test]
    fn coalesce_alternating_roles_unchanged() {
        let msgs = vec![
            ChatMessage { role: ChatRole::User, content: "Hello".into() },
            ChatMessage { role: ChatRole::Assistant, content: "Hi".into() },
            ChatMessage { role: ChatRole::User, content: "How?".into() },
        ];
        let result = coalesce_messages(&msgs);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "Hello");
        assert_eq!(result[1].content, "Hi");
        assert_eq!(result[2].content, "How?");
    }

    #[test]
    fn coalesce_consecutive_same_role() {
        let msgs = vec![
            ChatMessage { role: ChatRole::User, content: "Part 1".into() },
            ChatMessage { role: ChatRole::User, content: "Part 2".into() },
            ChatMessage { role: ChatRole::Assistant, content: "Reply".into() },
        ];
        let result = coalesce_messages(&msgs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "Part 1\nPart 2");
        assert_eq!(result[0].role, ChatRole::User);
        assert_eq!(result[1].content, "Reply");
    }

    #[test]
    fn coalesce_empty_messages() {
        let result = coalesce_messages(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn coalesce_single_message() {
        let msgs = vec![ChatMessage { role: ChatRole::User, content: "Solo".into() }];
        let result = coalesce_messages(&msgs);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Solo");
    }

    #[test]
    fn is_transient_error_detects_numeric_status_prefix() {
        // Anthropic-formatted transient errors should be detected.
        assert!(is_transient_error(&anyhow!(
            "Anthropic API error (429 Too Many Requests): rate limited"
        )));
        assert!(is_transient_error(&anyhow!(
            "Anthropic API error (500 Internal Server Error): internal"
        )));
        assert!(is_transient_error(&anyhow!("Anthropic API error (502 Bad Gateway): bad gateway")));
        assert!(is_transient_error(&anyhow!(
            "Anthropic API error (503 Service Unavailable): overloaded"
        )));
        assert!(is_transient_error(&anyhow!("Anthropic API error (504 Gateway Timeout): timeout")));
        assert!(is_transient_error(&anyhow!(
            "Anthropic API error (529 Overloaded): overloaded"
        )));

        // Bare numeric code also remains supported.
        assert!(is_transient_error(&anyhow!("Anthropic API error (429): rate limited")));

        // Non-transient errors should not match.
        assert!(!is_transient_error(&anyhow!(
            "Anthropic API error (400 Bad Request): bad request"
        )));
        assert!(!is_transient_error(&anyhow!(
            "Anthropic API error (401 Unauthorized): unauthorized"
        )));

        // Bare numbers embedded in text must not false-positive.
        assert!(!is_transient_error(&anyhow!("requested 50200 tokens")));
        assert!(!is_transient_error(&anyhow!("model limit is 429000 tokens")));
    }

    #[test]
    fn is_transient_error_checks_wrapped_causes() {
        let err = anyhow!("Anthropic API error (503 Service Unavailable): overloaded")
            .context("Anthropic completion");
        assert!(is_transient_error(&err));
    }

    #[test]
    fn is_transient_error_detects_openai_rate_limit_and_server_errors() {
        let rate_limit = OpenAIError::ApiError(async_openai::error::ApiError {
            message: "too many requests".into(),
            r#type: Some("rate_limit_exceeded".into()),
            param: None,
            code: Some("rate_limit_exceeded".into()),
        });
        assert!(is_transient_error(&anyhow!(rate_limit)));

        let server_error = OpenAIError::ApiError(async_openai::error::ApiError {
            message: "backend unavailable".into(),
            r#type: Some("server_error".into()),
            param: None,
            code: None,
        });
        assert!(is_transient_error(&anyhow!(server_error)));

        let quota_error = OpenAIError::ApiError(async_openai::error::ApiError {
            message: "quota exhausted".into(),
            r#type: Some("insufficient_quota".into()),
            param: None,
            code: Some("insufficient_quota".into()),
        });
        assert!(!is_transient_error(&anyhow!(quota_error)));
    }

    #[test]
    fn retry_delay_uses_saturating_math() {
        assert_eq!(retry_delay(1, 500), Duration::from_millis(500));
        assert_eq!(retry_delay(2, 500), Duration::from_millis(1_000));
        assert_eq!(retry_delay(4, 500), Duration::from_millis(4_000));
        assert_eq!(retry_delay(100, u64::MAX), Duration::from_millis(u64::MAX));
    }

    #[test]
    fn from_config_allows_keyless_openai_compatible_backend() {
        let cfg = test_config();
        let backend = ChatBackend::from_config(&cfg).expect("keyless openai-compatible backend");
        assert!(matches!(backend, ChatBackend::OpenAiCompatible { .. }));
    }

    #[test]
    fn from_config_requires_api_key_for_anthropic() {
        let mut cfg = test_config();
        cfg.llm_provider = LlmProvider::Anthropic;
        cfg.llm_model = "claude-sonnet-4-20250514".to_string();
        cfg.llm_base_url = "https://api.anthropic.com".to_string();

        let err = match ChatBackend::from_config(&cfg) {
            Ok(_) => panic!("anthropic should require API key"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("LLM_API_KEY must be set for anthropic"));
    }

    #[test]
    fn build_openai_request_uses_max_tokens_field() {
        let request = CompletionRequest {
            system: "You are a helpful assistant.",
            messages: &[ChatMessage { role: ChatRole::User, content: "hello".into() }],
            temperature: 0.0,
            max_tokens: 123,
            stop: vec![],
        };

        let built = build_openai_request("gpt-4o", &request, request.messages).expect("request");
        let json = serde_json::to_value(&built).expect("serialize request");

        assert_eq!(json.get("max_tokens").and_then(serde_json::Value::as_u64), Some(123));
        assert!(json.get("max_completion_tokens").is_none());
    }

    #[test]
    fn openai_internal_backoff_is_disabled() {
        let mut backoff = build_openai_backoff();
        assert_eq!(backoff.next_backoff(), None);
    }

    #[test]
    fn is_transient_error_detects_openai_reqwest_transport_failures() {
        // Build a reqwest error by attempting to parse an invalid URL.
        let reqwest_err = reqwest::Client::new()
            .get("http://[::0:0:0:0:0:0:0:0:0:0:invalid")
            .build()
            .expect_err("should produce a reqwest error");
        let openai_err = OpenAIError::Reqwest(reqwest_err);
        assert!(is_transient_error(&anyhow!(openai_err)));
    }

    #[test]
    fn anthropic_url_strips_duplicate_v1_suffix() {
        // base_url already contains /v1 — should NOT produce /v1/v1/messages
        let base = "https://api.anthropic.com/v1";
        let trimmed = base.trim_end_matches('/');
        let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        assert_eq!(format!("{trimmed}/v1/messages"), "https://api.anthropic.com/v1/messages");

        // base_url without /v1 — should produce /v1/messages
        let base = "https://api.anthropic.com";
        let trimmed = base.trim_end_matches('/');
        let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        assert_eq!(format!("{trimmed}/v1/messages"), "https://api.anthropic.com/v1/messages");

        // base_url with trailing slash and /v1
        let base = "https://api.anthropic.com/v1/";
        let trimmed = base.trim_end_matches('/');
        let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
        assert_eq!(format!("{trimmed}/v1/messages"), "https://api.anthropic.com/v1/messages");
    }
}
