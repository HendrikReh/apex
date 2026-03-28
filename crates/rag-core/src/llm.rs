//! LLM backend abstraction with provider-agnostic completion API.
//!
//! Supports OpenAI-compatible endpoints (OpenAI, xAI, Groq, Ollama, etc.)
//! and native Anthropic Messages API.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs,
};
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
        client: Box<async_openai::Client<async_openai::config::OpenAIConfig>>,
        model: String,
    },
    Anthropic {
        client: reqwest::Client,
        model: String,
        base_url: String,
    },
}

impl ChatBackend {
    /// Construct a backend from application config.
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        let api_key =
            config.llm_api_key.as_deref().ok_or_else(|| anyhow!("LLM_API_KEY must be set"))?;

        match config.llm_provider {
            LlmProvider::OpenAiCompatible => {
                let oai_config =
                    OpenAIConfig::new().with_api_key(api_key).with_api_base(&config.llm_base_url);
                let client = Box::new(async_openai::Client::with_config(oai_config));
                Ok(Self::OpenAiCompatible { client, model: config.llm_model.clone() })
            }
            LlmProvider::Anthropic => {
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

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let backoff = Duration::from_millis(retry_backoff_ms * 2u64.pow(attempt - 1));
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
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("retry loop exhausted")))
    }
}

/// Check if an error is transient (retryable): 429, 500, 502, 503.
fn is_transient_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    for code in ["429", "500", "502", "503"] {
        if msg.contains(code) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// OpenAI-compatible implementation
// ---------------------------------------------------------------------------

async fn complete_openai(
    client: &async_openai::Client<OpenAIConfig>,
    model: &str,
    request: &CompletionRequest<'_>,
    messages: &[ChatMessage],
) -> Result<LlmResponse> {
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
        .max_completion_tokens(request.max_tokens);

    if !request.stop.is_empty() {
        req_builder.stop(request.stop.clone());
    }

    let oai_request = req_builder.build().context("building OpenAI request")?;
    let response = client.chat().create(oai_request).await.context("OpenAI chat completion")?;

    let choice = response.choices.first().ok_or_else(|| anyhow!("OpenAI returned no choices"))?;
    let text = choice.message.content.clone().unwrap_or_default();

    let usage = response.usage.map_or(TokenUsage::default(), |u| TokenUsage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
    });

    Ok(LlmResponse { text, usage, model: response.model })
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
    if messages.is_empty() {
        bail!("Anthropic requires at least one message");
    }

    let anthropic_messages: Vec<AnthropicMessage> = messages
        .iter()
        .map(|m| AnthropicMessage {
            role: match m.role {
                ChatRole::User => "user".to_owned(),
                ChatRole::Assistant => "assistant".to_owned(),
            },
            content: m.content.clone(),
        })
        .collect();

    let body = AnthropicRequest {
        model,
        max_tokens: request.max_tokens,
        system: request.system,
        messages: anthropic_messages,
        temperature: Some(request.temperature),
        stop_sequences: request.stop.clone(),
    };

    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
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
}
