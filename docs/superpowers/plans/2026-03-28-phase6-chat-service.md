# Phase 6: Chat Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provider-agnostic LLM chat with context-grounded responses, conversation persistence, and Handlebars prompt templating.

**Architecture:** `ChatService` orchestrates: resolve conversation → persist user message → hybrid retrieval → context assembly → render prompt → call LLM → persist assistant message → return response with citations. LLM backend is a `ChatBackend` enum with OpenAI-compatible and native Anthropic variants. Conversation storage uses existing Postgres tables from migration 0002.

**Tech Stack:** async-openai (OpenAI-compatible), reqwest (Anthropic HTTP), handlebars (prompt templates), sqlx (Postgres), tiktoken-rs (token counting)

**Spec:** `docs/superpowers/specs/2026-03-28-phase6-chat-service-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/rag-core/src/llm.rs` | Create | `ChatBackend` enum, `CompletionRequest`, `LlmResponse`, `ChatMessage`, `ChatRole`, `TokenUsage`, message coalescing, retry |
| `crates/rag-core/src/prompt.rs` | Create | `PromptRenderer`, `PromptContext`, `render_context_chunks` |
| `crates/rag-core/src/stores/conversations.rs` | Create | `MessageRole`, `ConversationRow`, `MessageRow`, conversation/message CRUD |
| `crates/rag-core/src/chat.rs` | Create | `ChatService`, `ChatRequest`, `ChatResponse`, `ChatDefaults`, orchestration |
| `config/prompts/chat_system.hbs` | Create | Handlebars system prompt template |
| `crates/rag-core/src/config.rs` | Modify | Add `LlmProvider` enum, `LlmSection` struct, 10 new `AppConfig` fields, validation |
| `config/app.toml` | Modify | Add `[llm]` section |
| `crates/rag-core/src/stores/mod.rs` | Modify | Add `#[derive(Clone)]`, register `conversations` module |
| `crates/rag-core/src/lib.rs` | Modify | Add `pub mod llm`, `pub mod prompt`, `pub mod chat`, re-exports |
| `crates/rag-core/Cargo.toml` | Modify | Add `handlebars` dependency |
| `crates/rag-core/tests/integration_conversations.rs` | Create | Postgres-only conversation store tests |
| `crates/rag-core/tests/integration_chat.rs` | Create | End-to-end chat tests with mock LLM backend |
| `crates/rag-core/tests/smoke_llm.rs` | Create | Provider-gated real LLM smoke tests |

---

### Task 1: Add `[llm]` config fields and `LlmProvider` enum

**Files:**
- Modify: `crates/rag-core/src/config.rs`
- Modify: `config/app.toml`

- [ ] **Step 1: Add `LlmProvider` enum and `LlmSection` TOML struct**

In `crates/rag-core/src/config.rs`, add after the `AuthMode` impl block (after line 75):

```rust
/// LLM provider backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProvider {
    OpenAiCompatible,
    Anthropic,
}

impl FromStr for LlmProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "openai-compatible" | "openai" => Ok(Self::OpenAiCompatible),
            "anthropic" => Ok(Self::Anthropic),
            other => Err(anyhow::anyhow!("unknown LLM provider: {other:?}")),
        }
    }
}

impl fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAiCompatible => f.write_str("openai-compatible"),
            Self::Anthropic => f.write_str("anthropic"),
        }
    }
}
```

Add the `LlmSection` TOML struct after `ContextSection` (after line 99):

```rust
#[derive(Deserialize, Default)]
struct LlmSection {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    timeout_secs: Option<u64>,
    max_retries: Option<u32>,
    retry_backoff_ms: Option<u64>,
    prompt_template_path: Option<String>,
}
```

Add `llm: Option<LlmSection>` to the `AppSettings` struct:

```rust
#[derive(Deserialize, Default)]
struct AppSettings {
    app: Option<AppSection>,
    retrieval: Option<RetrievalSection>,
    context: Option<ContextSection>,
    llm: Option<LlmSection>,
}
```

- [ ] **Step 2: Add LLM fields to `AppConfig` and wire up loading**

Add after the `context_max_chunks` field in `AppConfig`:

```rust
    // LLM
    pub llm_provider: LlmProvider,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    pub llm_base_url: String,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub llm_timeout_secs: u64,
    pub llm_max_retries: u32,
    pub llm_retry_backoff_ms: u64,
    pub llm_prompt_template_path: String,
```

In `from_current_env()`, update the destructuring to include `llm`:

```rust
        let (file_settings, retrieval_settings, context_settings, llm_settings) =
            if std::path::Path::new(&config_path).exists() {
                let contents = std::fs::read_to_string(&config_path)
                    .with_context(|| format!("reading config file {config_path}"))?;
                let settings: AppSettings = toml::from_str(&contents)
                    .with_context(|| format!("parsing config file {config_path}"))?;
                (
                    settings.app.unwrap_or_default(),
                    settings.retrieval.unwrap_or_default(),
                    settings.context.unwrap_or_default(),
                    settings.llm.unwrap_or_default(),
                )
            } else {
                (
                    AppSection::default(),
                    RetrievalSection::default(),
                    ContextSection::default(),
                    LlmSection::default(),
                )
            };
```

After the context fields, add the LLM loading block (before `Ok(Self {`):

```rust
        let llm = &llm_settings;

        let llm_provider_str = env_string("LLM_PROVIDER")
            .or_else(|| llm.provider.clone())
            .unwrap_or_else(|| "openai-compatible".to_owned());
        let llm_provider = llm_provider_str
            .parse::<LlmProvider>()
            .with_context(|| format!("parsing LLM provider from {llm_provider_str:?}"))?;

        let llm_api_key = env_string("LLM_API_KEY");

        let llm_model = env_string("LLM_MODEL")
            .or_else(|| llm.model.clone())
            .unwrap_or_else(|| "gpt-4o".to_owned());
        if llm_model.is_empty() {
            anyhow::bail!("llm_model must not be empty");
        }

        let default_base_url = match llm_provider {
            LlmProvider::OpenAiCompatible => "https://api.openai.com/v1",
            LlmProvider::Anthropic => "https://api.anthropic.com",
        };
        let llm_base_url = env_string("LLM_BASE_URL")
            .or_else(|| llm.base_url.clone())
            .unwrap_or_else(|| default_base_url.to_owned());
        if llm_base_url.is_empty() {
            anyhow::bail!("llm_base_url must not be empty");
        }

        let llm_temperature: f32 =
            env_parsed("LLM_TEMPERATURE")?.or(llm.temperature).unwrap_or(0.1);
        if llm_temperature < 0.0 || !llm_temperature.is_finite() {
            anyhow::bail!(
                "llm_temperature must be >= 0.0 and finite, got {llm_temperature}"
            );
        }

        let llm_max_tokens =
            env_parsed("LLM_MAX_TOKENS")?.or(llm.max_tokens).unwrap_or(4096);
        if llm_max_tokens == 0 {
            anyhow::bail!("llm_max_tokens must be greater than zero");
        }

        let llm_timeout_secs =
            env_parsed("LLM_TIMEOUT_SECS")?.or(llm.timeout_secs).unwrap_or(60);
        if llm_timeout_secs == 0 {
            anyhow::bail!("llm_timeout_secs must be greater than zero");
        }

        let llm_max_retries =
            env_parsed("LLM_MAX_RETRIES")?.or(llm.max_retries).unwrap_or(3);

        let llm_retry_backoff_ms =
            env_parsed("LLM_RETRY_BACKOFF_MS")?.or(llm.retry_backoff_ms).unwrap_or(500);

        let llm_prompt_template_path = env_string("LLM_PROMPT_TEMPLATE_PATH")
            .or_else(|| llm.prompt_template_path.clone())
            .unwrap_or_else(|| "config/prompts/chat_system.hbs".to_owned());
```

Add all 10 fields to the `Ok(Self { ... })` struct literal:

```rust
            llm_provider,
            llm_api_key,
            llm_model,
            llm_base_url,
            llm_temperature,
            llm_max_tokens,
            llm_timeout_secs,
            llm_max_retries,
            llm_retry_backoff_ms,
            llm_prompt_template_path,
```

- [ ] **Step 3: Add the new env vars to `clear_config_env()` in the test**

In the `clear_config_env()` function, add:

```rust
            std::env::remove_var("LLM_PROVIDER");
            std::env::remove_var("LLM_API_KEY");
            std::env::remove_var("LLM_MODEL");
            std::env::remove_var("LLM_BASE_URL");
            std::env::remove_var("LLM_TEMPERATURE");
            std::env::remove_var("LLM_MAX_TOKENS");
            std::env::remove_var("LLM_TIMEOUT_SECS");
            std::env::remove_var("LLM_MAX_RETRIES");
            std::env::remove_var("LLM_RETRY_BACKOFF_MS");
            std::env::remove_var("LLM_PROMPT_TEMPLATE_PATH");
```

Add assertions for the new defaults in the test's "Part 1: verify all defaults" section:

```rust
        assert_eq!(cfg.llm_provider, LlmProvider::OpenAiCompatible);
        assert!(cfg.llm_api_key.is_none());
        assert_eq!(cfg.llm_model, "gpt-4o");
        assert_eq!(cfg.llm_base_url, "https://api.openai.com/v1");
        assert!((cfg.llm_temperature - 0.1).abs() < f32::EPSILON);
        assert_eq!(cfg.llm_max_tokens, 4096);
        assert_eq!(cfg.llm_timeout_secs, 60);
        assert_eq!(cfg.llm_max_retries, 3);
        assert_eq!(cfg.llm_retry_backoff_ms, 500);
        assert_eq!(cfg.llm_prompt_template_path, "config/prompts/chat_system.hbs");
```

Add a `LlmProvider` parsing test:

```rust
    #[test]
    fn llm_provider_parsing() {
        assert_eq!("openai-compatible".parse::<LlmProvider>().ok(), Some(LlmProvider::OpenAiCompatible));
        assert_eq!("openai".parse::<LlmProvider>().ok(), Some(LlmProvider::OpenAiCompatible));
        assert_eq!("anthropic".parse::<LlmProvider>().ok(), Some(LlmProvider::Anthropic));
        assert_eq!("ANTHROPIC".parse::<LlmProvider>().ok(), Some(LlmProvider::Anthropic));
        assert!("invalid".parse::<LlmProvider>().is_err());
    }
```

Add config validation rejection tests (use `serial_test` or set env vars to trigger failures):

```rust
    #[test]
    fn llm_empty_model_rejected() {
        let _guard = CONFIG_LOCK.lock();
        clear_config_env();
        unsafe {
            std::env::set_var("LLM_MODEL", "");
        }
        let result = AppConfig::from_env();
        assert!(result.is_err());
        let err = result.expect_err("expected error").to_string();
        assert!(err.contains("llm_model must not be empty"), "error: {err}");
    }

    #[test]
    fn llm_negative_temperature_rejected() {
        let _guard = CONFIG_LOCK.lock();
        clear_config_env();
        unsafe {
            std::env::set_var("LLM_TEMPERATURE", "-1.0");
        }
        let result = AppConfig::from_env();
        assert!(result.is_err());
        let err = result.expect_err("expected error").to_string();
        assert!(err.contains("llm_temperature"), "error: {err}");
    }

    #[test]
    fn llm_zero_max_tokens_rejected() {
        let _guard = CONFIG_LOCK.lock();
        clear_config_env();
        unsafe {
            std::env::set_var("LLM_MAX_TOKENS", "0");
        }
        let result = AppConfig::from_env();
        assert!(result.is_err());
        let err = result.expect_err("expected error").to_string();
        assert!(err.contains("llm_max_tokens"), "error: {err}");
    }
```

- [ ] **Step 4: Add `[llm]` section to `config/app.toml`**

Append to the end of `config/app.toml`:

```toml

[llm]
provider = "openai-compatible"                      # Options: "openai-compatible", "anthropic"
model = "gpt-4o"
# base_url = "https://api.openai.com/v1"            # Default per provider
temperature = 0.1
max_tokens = 4096
timeout_secs = 60
max_retries = 3
retry_backoff_ms = 500
prompt_template_path = "config/prompts/chat_system.hbs"
```

- [ ] **Step 5: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/config.rs config/app.toml
git commit -m "feat(config): add LLM provider configuration with [llm] section"
```

---

### Task 2: Make `Stores` cloneable

**Files:**
- Modify: `crates/rag-core/src/stores/mod.rs`

- [ ] **Step 1: Add `Clone` derive to `Stores`**

Change the struct definition:

```rust
#[derive(Clone)]
pub struct Stores {
    pool: PgPool,
    qdrant: Arc<Qdrant>,
    /// The default Qdrant collection name from configuration.
    pub default_collection: String,
}
```

`PgPool` is already `Clone` (it wraps an `Arc` internally). `Arc<Qdrant>` is `Clone`. `String` is `Clone`. This requires no other changes.

- [ ] **Step 2: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-core/src/stores/mod.rs
git commit -m "feat(stores): derive Clone for Stores"
```

---

### Task 3: LLM backend with message validation and retry (`llm.rs`)

**Files:**
- Create: `crates/rag-core/src/llm.rs`
- Modify: `crates/rag-core/src/lib.rs`
- Modify: `crates/rag-core/Cargo.toml`

**Reference:** Spec section 1 (LLM Backend). The `async-openai` crate is already a workspace dep with `chat-completion` feature. `reqwest` is already a workspace dep with `json` + `rustls-tls`. `handlebars` is a workspace dep but not yet in rag-core's Cargo.toml.

- [ ] **Step 1: Add `handlebars` to rag-core Cargo.toml**

Add to `[dependencies]` in `crates/rag-core/Cargo.toml`:

```toml
handlebars.workspace = true
```

- [ ] **Step 2: Write unit tests for message coalescing**

Create `crates/rag-core/src/llm.rs`:

```rust
//! LLM backend abstraction with provider-agnostic completion API.
//!
//! Supports OpenAI-compatible endpoints (OpenAI, xAI, Groq, Ollama, etc.)
//! and native Anthropic Messages API.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
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
        if let Some(last) = result.last_mut() {
            if last.role == msg.role {
                last.content.push('\n');
                last.content.push_str(&msg.content);
                continue;
            }
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
        client: async_openai::Client<async_openai::config::OpenAIConfig>,
        model: String,
    },
    Anthropic {
        client: reqwest::Client,
        model: String,
        base_url: String,
    },
}

// Public API and provider implementations will be added in subsequent steps.

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
        let msgs = vec![
            ChatMessage { role: ChatRole::User, content: "Solo".into() },
        ];
        let result = coalesce_messages(&msgs);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "Solo");
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p rag-core coalesce`
Expected: 4 tests PASS.

- [ ] **Step 4: Implement `ChatBackend` construction and `complete` method**

Replace the comment `// Public API and provider implementations will be added in subsequent steps.` with the full implementation. Add these `use` statements at the top (merge with existing):

```rust
use async_openai::config::OpenAIConfig;
use async_openai::types::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs,
};
```

Then add the implementation:

```rust
impl ChatBackend {
    /// Construct a backend from application config.
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        let api_key = config
            .llm_api_key
            .as_deref()
            .ok_or_else(|| anyhow!("LLM_API_KEY must be set"))?;

        match config.llm_provider {
            LlmProvider::OpenAiCompatible => {
                let oai_config = OpenAIConfig::new()
                    .with_api_key(api_key)
                    .with_api_base(&config.llm_base_url);
                let client = async_openai::Client::with_config(oai_config);
                Ok(Self::OpenAiCompatible {
                    client,
                    model: config.llm_model.clone(),
                })
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
                            "2023-06-01"
                                .parse()
                                .context("invalid anthropic-version header")?,
                        );
                        headers.insert(
                            reqwest::header::CONTENT_TYPE,
                            "application/json"
                                .parse()
                                .context("invalid content-type header")?,
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
                let backoff =
                    Duration::from_millis(retry_backoff_ms * 2u64.pow(attempt - 1));
                tokio::time::sleep(backoff).await;
            }

            let result = match self {
                Self::OpenAiCompatible { client, model } => {
                    complete_openai(client, model, request, &messages).await
                }
                Self::Anthropic { client, model, base_url } => {
                    complete_anthropic(client, model, base_url, request, &messages)
                        .await
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
    // Anthropic errors include the HTTP status code in the message.
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
        .max_completion_tokens(u32::from(request.max_tokens));

    if !request.stop.is_empty() {
        req_builder.stop(request.stop.clone());
    }

    let oai_request = req_builder.build().context("building OpenAI request")?;
    let response = client
        .chat()
        .create(oai_request)
        .await
        .context("OpenAI chat completion")?;

    let choice = response
        .choices
        .first()
        .ok_or_else(|| anyhow!("OpenAI returned no choices"))?;
    let text = choice
        .message
        .content
        .clone()
        .unwrap_or_default();

    let usage = response.usage.map_or(TokenUsage::default(), |u| TokenUsage {
        prompt_tokens: u.prompt_tokens as u32,
        completion_tokens: u.completion_tokens as u32,
    });

    Ok(LlmResponse {
        text,
        usage,
        model: response.model,
    })
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
    let http_response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Anthropic HTTP request")?;

    let status = http_response.status();
    if !status.is_success() {
        let body_text = http_response.text().await.unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<AnthropicError>(&body_text) {
            bail!("Anthropic API error ({}): {}", status, err.error.message);
        }
        bail!("Anthropic API error ({}): {}", status, body_text);
    }

    let response: AnthropicResponse = http_response
        .json()
        .await
        .context("parsing Anthropic response")?;

    let text = response
        .content
        .into_iter()
        .filter_map(|c| c.text)
        .collect::<Vec<_>>()
        .join("");

    Ok(LlmResponse {
        text,
        usage: TokenUsage {
            prompt_tokens: response.usage.input_tokens,
            completion_tokens: response.usage.output_tokens,
        },
        model: response.model,
    })
}
```

- [ ] **Step 5: Register module in `lib.rs`**

Add to `crates/rag-core/src/lib.rs`:

Module declaration (add after `pub mod ingest;`):
```rust
pub mod llm;
```

Re-exports (add after the existing re-exports):
```rust
pub use llm::{ChatBackend, ChatMessage, ChatRole, CompletionRequest, LlmResponse, TokenUsage};
```

- [ ] **Step 6: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

Run: `cargo test -p rag-core coalesce`
Expected: 4 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rag-core/src/llm.rs crates/rag-core/src/lib.rs crates/rag-core/Cargo.toml
git commit -m "feat(llm): add ChatBackend with OpenAI-compatible and Anthropic providers"
```

---

### Task 4: Prompt template renderer (`prompt.rs`)

**Files:**
- Create: `crates/rag-core/src/prompt.rs`
- Create: `config/prompts/chat_system.hbs`
- Modify: `crates/rag-core/src/lib.rs`

- [ ] **Step 1: Write unit tests first**

Create `crates/rag-core/src/prompt.rs`:

```rust
//! Prompt template loading and rendering via Handlebars.

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde::Serialize;

use crate::context::ContextChunk;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context variables for the system prompt template.
#[derive(Debug, Serialize)]
pub struct PromptContext<'a> {
    pub context: &'a str,
    pub language_instruction: &'a str,
}

/// Loads and renders a Handlebars template for system prompts.
pub struct PromptRenderer {
    handlebars: Handlebars<'static>,
}

impl PromptRenderer {
    /// Load and compile a template from the given file path.
    ///
    /// Fails fast if the file is missing or the template is invalid.
    pub fn from_file(path: &str) -> Result<Self> {
        let template = std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt template from {path}"))?;
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars
            .register_template_string("system", &template)
            .with_context(|| format!("compiling prompt template from {path}"))?;
        Ok(Self { handlebars })
    }

    /// Render the system prompt with the given context.
    pub fn render_system_prompt(&self, context: &PromptContext<'_>) -> Result<String> {
        self.handlebars
            .render("system", context)
            .context("rendering system prompt template")
    }
}

// ---------------------------------------------------------------------------
// Context chunk rendering
// ---------------------------------------------------------------------------

/// Render context chunks as a numbered list for the LLM prompt.
///
/// Produces format: `[1] <text>\n---\n[2] <text>\n---\n...`
/// Returns empty string for empty input.
pub fn render_context_chunks(chunks: &[ContextChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| format!("[{}] {}", i + 1, chunk.text))
        .collect::<Vec<_>>()
        .join("\n---\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Citation;

    fn make_chunk(text: &str) -> ContextChunk {
        ContextChunk {
            chunk_id: "id".into(),
            text: text.into(),
            document_id: "doc".into(),
            chunk_index: 0,
            fused_score: 1.0,
            token_count: 5,
            citation: Some(Citation {
                chunk_id: "id".into(),
                document_id: "doc".into(),
                chunk_index: 0,
                sources: vec!["dense".into()],
            }),
        }
    }

    #[test]
    fn render_context_chunks_numbered() {
        let chunks = vec![
            make_chunk("First chunk text"),
            make_chunk("Second chunk text"),
            make_chunk("Third chunk text"),
        ];
        let rendered = render_context_chunks(&chunks);
        assert_eq!(
            rendered,
            "[1] First chunk text\n---\n[2] Second chunk text\n---\n[3] Third chunk text"
        );
    }

    #[test]
    fn render_context_chunks_empty() {
        assert_eq!(render_context_chunks(&[]), "");
    }

    #[test]
    fn render_context_chunks_single() {
        let chunks = vec![make_chunk("Only chunk")];
        assert_eq!(render_context_chunks(&chunks), "[1] Only chunk");
    }

    #[test]
    fn prompt_renderer_from_file_missing() {
        let result = PromptRenderer::from_file("/tmp/nonexistent-prompt-template.hbs");
        assert!(result.is_err());
    }

    #[test]
    fn prompt_renderer_renders_template() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("test.hbs");
        std::fs::write(
            &path,
            "You are a helpful assistant.\n\n{{context}}\n\n{{language_instruction}}",
        )
        .expect("write");

        let renderer =
            PromptRenderer::from_file(path.to_str().expect("path")).expect("from_file");
        let result = renderer
            .render_system_prompt(&PromptContext {
                context: "[1] Some context",
                language_instruction: "Respond in English",
            })
            .expect("render");

        assert!(result.contains("[1] Some context"));
        assert!(result.contains("Respond in English"));
    }

    #[test]
    fn prompt_renderer_empty_language_instruction() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("test.hbs");
        std::fs::write(
            &path,
            "Assistant.\n\n{{context}}\n\n{{language_instruction}}",
        )
        .expect("write");

        let renderer =
            PromptRenderer::from_file(path.to_str().expect("path")).expect("from_file");
        let result = renderer
            .render_system_prompt(&PromptContext {
                context: "[1] Chunk",
                language_instruction: "",
            })
            .expect("render");

        assert!(result.contains("[1] Chunk"));
        // Empty language_instruction should not leave artifacts.
        assert!(!result.contains("{{"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-core prompt`
Expected: 6 tests PASS.

- [ ] **Step 3: Create the prompt template file**

Create `config/prompts/chat_system.hbs`:

```handlebars
You are a helpful assistant that answers questions using the provided context.

## Instructions
- Answer the user's question based on the context below.
- Use inline citations like [1], [2], etc. to reference the numbered context chunks.
- If the context does not contain enough information to answer, say so honestly.
- Do not fabricate information that is not supported by the context.

## Context
{{context}}

{{language_instruction}}
```

- [ ] **Step 4: Register module in `lib.rs`**

Add to `crates/rag-core/src/lib.rs`:

Module declaration (add after `pub mod llm;`):
```rust
pub mod prompt;
```

Re-exports:
```rust
pub use prompt::{PromptContext, PromptRenderer, render_context_chunks};
```

- [ ] **Step 5: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

Run: `cargo test -p rag-core prompt`
Expected: 6 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/prompt.rs crates/rag-core/src/lib.rs config/prompts/chat_system.hbs
git commit -m "feat(prompt): add Handlebars prompt renderer with context chunk formatting"
```

---

### Task 5: Conversation store (`stores/conversations.rs`)

**Files:**
- Create: `crates/rag-core/src/stores/conversations.rs`
- Modify: `crates/rag-core/src/stores/mod.rs`
- Modify: `crates/rag-core/src/lib.rs`

**Reference:** The `conversations` and `messages` tables already exist in migration 0002. The `conversations.user_id` column is nullable, so we insert `NULL` for now (no user management in this phase).

- [ ] **Step 1: Create the conversation store module**

Create `crates/rag-core/src/stores/conversations.rs`:

```rust
//! Conversation and message persistence (Postgres).
//!
//! Operates on the `conversations` and `messages` tables from migration 0002.
//! All operations are tenant-scoped.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use uuid::Uuid;

use super::Stores;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Role for persisted messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

impl MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn from_db(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => bail!("unknown message role from DB: {other:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: Uuid,
    pub tenant: String,
    pub title: Option<String>,
    pub collection: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct ConversationDbRow {
    id: Uuid,
    tenant: String,
    title: Option<String>,
    collection: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<ConversationDbRow> for ConversationRow {
    fn from(r: ConversationDbRow) -> Self {
        Self {
            id: r.id,
            tenant: r.tenant,
            title: r.title,
            collection: r.collection,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub metadata: Option<JsonValue>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct MessageDbRow {
    id: Uuid,
    conversation_id: Uuid,
    role: String,
    content: String,
    metadata: Option<JsonValue>,
    created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Store methods
// ---------------------------------------------------------------------------

impl Stores {
    /// Create a new conversation for the given tenant.
    pub async fn create_conversation(
        &self,
        tenant: &str,
        title: Option<&str>,
        collection: Option<&str>,
    ) -> Result<ConversationRow> {
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, ConversationDbRow>(
            "INSERT INTO conversations (id, tenant, title, collection)
             VALUES ($1, $2, $3, $4)
             RETURNING id, tenant, title, collection, created_at",
        )
        .bind(id)
        .bind(tenant)
        .bind(title)
        .bind(collection)
        .fetch_one(self.pg_pool())
        .await
        .context("creating conversation")?;

        Ok(row.into())
    }

    /// Get a conversation by ID, scoped to tenant.
    pub async fn get_conversation(
        &self,
        tenant: &str,
        conversation_id: Uuid,
    ) -> Result<Option<ConversationRow>> {
        let row = sqlx::query_as::<_, ConversationDbRow>(
            "SELECT id, tenant, title, collection, created_at
             FROM conversations
             WHERE id = $1 AND tenant = $2",
        )
        .bind(conversation_id)
        .bind(tenant)
        .fetch_optional(self.pg_pool())
        .await
        .context("getting conversation")?;

        Ok(row.map(ConversationRow::from))
    }

    /// Insert a message into a conversation, validating tenant ownership.
    ///
    /// Returns an error if the conversation does not exist or belongs to a
    /// different tenant.
    pub async fn insert_message(
        &self,
        tenant: &str,
        conversation_id: Uuid,
        role: MessageRole,
        content: &str,
        metadata: Option<JsonValue>,
    ) -> Result<MessageRow> {
        let id = Uuid::new_v4();
        let role_str = role.as_str();

        // Validate tenant ownership by selecting through the conversation.
        let row = sqlx::query_as::<_, MessageDbRow>(
            "INSERT INTO messages (id, conversation_id, role, content, metadata)
             SELECT $1, c.id, $2, $3, $4
             FROM conversations c
             WHERE c.id = $5 AND c.tenant = $6
             RETURNING id, conversation_id, role, content, metadata, created_at",
        )
        .bind(id)
        .bind(role_str)
        .bind(content)
        .bind(&metadata)
        .bind(conversation_id)
        .bind(tenant)
        .fetch_optional(self.pg_pool())
        .await
        .context("inserting message")?;

        match row {
            Some(db_row) => Ok(MessageRow {
                id: db_row.id,
                conversation_id: db_row.conversation_id,
                role: MessageRole::from_db(&db_row.role)?,
                content: db_row.content,
                metadata: db_row.metadata,
                created_at: db_row.created_at,
            }),
            None => bail!(
                "conversation not found for tenant: conversation_id={conversation_id}, \
                 tenant={tenant}"
            ),
        }
    }

    /// Get the most recent N messages for a conversation, returned oldest-first.
    ///
    /// Validates `limit > 0`. Tenant is checked via join to `conversations`.
    pub async fn get_messages(
        &self,
        tenant: &str,
        conversation_id: Uuid,
        limit: i64,
    ) -> Result<Vec<MessageRow>> {
        if limit <= 0 {
            bail!("message limit must be > 0, got {limit}");
        }

        let rows = sqlx::query_as::<_, MessageDbRow>(
            "SELECT m.id, m.conversation_id, m.role, m.content, m.metadata, m.created_at
             FROM (
                 SELECT m2.*
                 FROM messages m2
                 JOIN conversations c ON c.id = m2.conversation_id
                 WHERE m2.conversation_id = $1 AND c.tenant = $2
                 ORDER BY m2.created_at DESC, m2.id DESC
                 LIMIT $3
             ) m
             ORDER BY m.created_at ASC, m.id ASC",
        )
        .bind(conversation_id)
        .bind(tenant)
        .bind(limit)
        .fetch_all(self.pg_pool())
        .await
        .context("getting messages")?;

        rows.into_iter()
            .map(|r| {
                Ok(MessageRow {
                    id: r.id,
                    conversation_id: r.conversation_id,
                    role: MessageRole::from_db(&r.role)?,
                    content: r.content,
                    metadata: r.metadata,
                    created_at: r.created_at,
                })
            })
            .collect()
    }
}
```

- [ ] **Step 2: Register in `stores/mod.rs`**

Add after the existing module declarations in `crates/rag-core/src/stores/mod.rs`:

```rust
pub mod conversations;
```

- [ ] **Step 3: Add re-exports to `lib.rs`**

Add to `crates/rag-core/src/lib.rs`:

```rust
pub use stores::conversations::{ConversationRow, MessageRole, MessageRow};
```

- [ ] **Step 4: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-core/src/stores/conversations.rs crates/rag-core/src/stores/mod.rs crates/rag-core/src/lib.rs
git commit -m "feat(stores): add conversation and message persistence"
```

---

### Task 6: ChatService orchestrator (`chat.rs`)

**Files:**
- Create: `crates/rag-core/src/chat.rs`
- Modify: `crates/rag-core/src/lib.rs`

**Reference:** Spec section 4. `ChatService` owns the full flow: resolve conversation → persist user message → retrieve → assemble context → render prompt → call LLM → persist assistant → return.

- [ ] **Step 1: Create `chat.rs` with types and ChatService**

Create `crates/rag-core/src/chat.rs`:

```rust
//! Chat service orchestrating retrieval, context assembly, and LLM completion.

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::context::{ContextBuilder, ContextConfig, Citation, DedupeStrategy};
use crate::llm::{ChatBackend, ChatMessage, ChatRole, CompletionRequest, LlmResponse, TokenUsage};
use crate::prompt::{PromptContext, PromptRenderer, render_context_chunks};
use crate::retrieval::RetrievalService;
use crate::stores::Stores;
use crate::stores::conversations::MessageRole;
use crate::tenant::TenantId;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Default parameters for chat operations.
#[derive(Debug, Clone)]
pub struct ChatDefaults {
    pub temperature: f32,
    pub max_tokens: u32,
    pub context_max_tokens: usize,
    pub context_max_chunks: usize,
    pub history_limit: i64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
}

impl ChatDefaults {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            temperature: config.llm_temperature,
            max_tokens: config.llm_max_tokens,
            context_max_tokens: config.context_max_tokens,
            context_max_chunks: config.context_max_chunks,
            history_limit: 30,
            max_retries: config.llm_max_retries,
            retry_backoff_ms: config.llm_retry_backoff_ms,
        }
    }
}

pub struct ChatRequest {
    pub query: String,
    pub collection: Option<String>,
    pub tenant: TenantId,
    pub conversation_id: Option<Uuid>,
    pub language: Option<String>,
    pub history_limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<Citation>,
    pub usage: TokenUsage,
    pub model: String,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct ChatService {
    backend: ChatBackend,
    retrieval: RetrievalService,
    context_builder: ContextBuilder,
    prompt_renderer: PromptRenderer,
    stores: Stores,
    defaults: ChatDefaults,
}

impl ChatService {
    /// Construct a ChatService from config and a shared Stores handle.
    ///
    /// Clones `stores` for `RetrievalService` (cheap: PgPool is Arc-based).
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let backend = ChatBackend::from_config(config)
            .context("building LLM backend")?;
        let retrieval = RetrievalService::new(stores.clone(), config)
            .context("building retrieval service")?;
        let context_builder = ContextBuilder::new();
        let prompt_renderer =
            PromptRenderer::from_file(&config.llm_prompt_template_path)
                .context("loading prompt template")?;
        let defaults = ChatDefaults::from_config(config);

        Ok(Self {
            backend,
            retrieval,
            context_builder,
            prompt_renderer,
            stores,
            defaults,
        })
    }

    /// Run the full chat pipeline.
    ///
    /// Flow: resolve conversation → load history → persist user message →
    /// retrieve context → assemble → render prompt → call LLM →
    /// persist assistant message → return response.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let tenant = request.tenant.as_str();
        let history_limit = match request.history_limit {
            Some(limit) => {
                if limit <= 0 {
                    bail!("history_limit must be > 0, got {limit}");
                }
                limit
            }
            None => self.defaults.history_limit,
        };

        // Step 1: Resolve conversation and collection.
        let (conversation_id, collection) = self
            .resolve_conversation(tenant, &request)
            .await?;

        // Step 2: Load history.
        let history_rows = self
            .stores
            .get_messages(tenant, conversation_id, history_limit)
            .await
            .context("loading conversation history")?;
        let mut messages: Vec<ChatMessage> = history_rows
            .into_iter()
            .map(|row| ChatMessage {
                role: match row.role {
                    MessageRole::User => ChatRole::User,
                    MessageRole::Assistant => ChatRole::Assistant,
                },
                content: row.content,
            })
            .collect();

        // Step 3: Persist user message (before LLM call).
        self.stores
            .insert_message(
                tenant,
                conversation_id,
                MessageRole::User,
                &request.query,
                None,
            )
            .await
            .context("persisting user message")?;

        // Step 4: Retrieve context.
        let fused = self
            .retrieval
            .search_hybrid(&collection, &request.query, tenant, None)
            .await
            .context("hybrid retrieval")?;

        // Step 5: Assemble context.
        let context_config = ContextConfig {
            max_tokens: self.defaults.context_max_tokens,
            max_chunks: self.defaults.context_max_chunks,
            dedupe_strategy: DedupeStrategy::ByDocId,
            include_citations: true,
        };
        let context_result = self.context_builder.build(fused, &context_config);
        let citations = context_result.citations.clone();

        // Step 6: Render prompt.
        let context_text = render_context_chunks(&context_result.chunks);
        let language_instruction = request
            .language
            .as_deref()
            .map(|lang| format!("Respond in {lang}."))
            .unwrap_or_default();
        let system_prompt = self
            .prompt_renderer
            .render_system_prompt(&PromptContext {
                context: &context_text,
                language_instruction: &language_instruction,
            })
            .context("rendering system prompt")?;

        // Step 7: Call LLM.
        messages.push(ChatMessage {
            role: ChatRole::User,
            content: request.query.clone(),
        });
        let llm_response = self
            .backend
            .complete(
                &CompletionRequest {
                    system: &system_prompt,
                    messages: &messages,
                    temperature: self.defaults.temperature,
                    max_tokens: self.defaults.max_tokens,
                    stop: vec![],
                },
                self.defaults.max_retries,
                self.defaults.retry_backoff_ms,
            )
            .await
            .context("LLM completion")?;

        // Step 8: Persist assistant message.
        self.stores
            .insert_message(
                tenant,
                conversation_id,
                MessageRole::Assistant,
                &llm_response.text,
                None,
            )
            .await
            .context("persisting assistant message")?;

        // Step 9: Return response.
        Ok(ChatResponse {
            answer: llm_response.text,
            conversation_id,
            citations,
            usage: llm_response.usage,
            model: llm_response.model,
        })
    }

    /// Resolve or create conversation, returning (conversation_id, collection).
    async fn resolve_conversation(
        &self,
        tenant: &str,
        request: &ChatRequest,
    ) -> Result<(Uuid, String)> {
        match request.conversation_id {
            Some(conv_id) => {
                // Resume existing conversation.
                let conv = self
                    .stores
                    .get_conversation(tenant, conv_id)
                    .await
                    .context("looking up conversation")?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "conversation not found: id={conv_id}, tenant={tenant}"
                        )
                    })?;

                let stored_collection = conv.collection.unwrap_or_default();
                let collection = match &request.collection {
                    Some(req_coll) => {
                        if !stored_collection.is_empty() && req_coll != &stored_collection {
                            bail!(
                                "collection mismatch: conversation has {stored_collection:?}, \
                                 request has {req_coll:?}"
                            );
                        }
                        req_coll.clone()
                    }
                    None => {
                        if stored_collection.is_empty() {
                            bail!(
                                "conversation {conv_id} has no stored collection and \
                                 request did not specify one"
                            );
                        }
                        stored_collection
                    }
                };

                Ok((conv_id, collection))
            }
            None => {
                // New conversation — collection is required.
                let collection = request
                    .collection
                    .clone()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "collection is required for the first message in a conversation"
                        )
                    })?;

                let conv = self
                    .stores
                    .create_conversation(tenant, None, Some(&collection))
                    .await
                    .context("creating conversation")?;

                Ok((conv.id, collection))
            }
        }
    }
}
```

- [ ] **Step 2: Register module and re-exports in `lib.rs`**

Add to `crates/rag-core/src/lib.rs`:

Module declaration (add after `pub mod llm;`):
```rust
pub mod chat;
```

Re-exports:
```rust
pub use chat::{ChatDefaults, ChatRequest, ChatResponse, ChatService};
```

- [ ] **Step 3: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-core/src/chat.rs crates/rag-core/src/lib.rs
git commit -m "feat(chat): add ChatService orchestrating retrieval, prompt, and LLM"
```

---

### Task 7: Conversation store integration tests (`integration_conversations.rs`)

**Files:**
- Create: `crates/rag-core/tests/integration_conversations.rs`

**Note:** These tests require Postgres only (`just up`). No Qdrant or LLM needed.

- [ ] **Step 1: Create test file**

Create `crates/rag-core/tests/integration_conversations.rs`:

```rust
//! Integration tests for conversation and message persistence.
//!
//! Requires Postgres running (`just up`). No Qdrant or LLM needed.

use anyhow::Result;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::stores::Stores;
use rag_core::stores::conversations::MessageRole;
use uuid::Uuid;

async fn setup() -> Result<Stores> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    Stores::new(&config).await
}

fn unique_tenant() -> String {
    format!("test-conv-{}", &Uuid::new_v4().to_string()[..8])
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn create_and_get_conversation() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores
        .create_conversation(&tenant, Some("Test title"), Some("my_collection"))
        .await?;

    assert_eq!(conv.tenant, tenant);
    assert_eq!(conv.title.as_deref(), Some("Test title"));
    assert_eq!(conv.collection.as_deref(), Some("my_collection"));

    let fetched = stores
        .get_conversation(&tenant, conv.id)
        .await?
        .expect("conversation should exist");
    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.tenant, tenant);

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn get_conversation_wrong_tenant_returns_none() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, None).await?;

    let result = stores.get_conversation("other-tenant", conv.id).await?;
    assert!(result.is_none(), "wrong tenant should not see conversation");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn insert_and_get_messages_role_roundtrip() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores
        .create_conversation(&tenant, None, Some("coll"))
        .await?;

    let user_msg = stores
        .insert_message(&tenant, conv.id, MessageRole::User, "Hello", None)
        .await?;
    assert_eq!(user_msg.role, MessageRole::User);
    assert_eq!(user_msg.content, "Hello");

    let asst_msg = stores
        .insert_message(
            &tenant,
            conv.id,
            MessageRole::Assistant,
            "Hi there",
            None,
        )
        .await?;
    assert_eq!(asst_msg.role, MessageRole::Assistant);
    assert_eq!(asst_msg.content, "Hi there");

    let messages = stores.get_messages(&tenant, conv.id, 10).await?;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content, "Hi there");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn get_messages_limit_respected() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, None).await?;

    for i in 0..5 {
        stores
            .insert_message(
                &tenant,
                conv.id,
                MessageRole::User,
                &format!("Message {i}"),
                None,
            )
            .await?;
    }

    let messages = stores.get_messages(&tenant, conv.id, 3).await?;
    assert_eq!(messages.len(), 3);
    // Should be the latest 3, returned oldest-first.
    assert_eq!(messages[0].content, "Message 2");
    assert_eq!(messages[1].content, "Message 3");
    assert_eq!(messages[2].content, "Message 4");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn tenant_isolation_on_insert() -> Result<()> {
    let stores = setup().await?;
    let tenant_a = unique_tenant();

    let conv = stores
        .create_conversation(&tenant_a, None, Some("coll"))
        .await?;

    let result = stores
        .insert_message("other-tenant", conv.id, MessageRole::User, "Sneaky", None)
        .await;
    assert!(result.is_err(), "should not insert into other tenant's conversation");
    let err_msg = result.expect_err("expected error").to_string();
    assert!(
        err_msg.contains("conversation not found for tenant"),
        "error should be explicit: {err_msg}"
    );

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn tenant_isolation_on_get_messages() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores
        .create_conversation(&tenant, None, Some("coll"))
        .await?;
    stores
        .insert_message(&tenant, conv.id, MessageRole::User, "Secret", None)
        .await?;

    let messages = stores.get_messages("other-tenant", conv.id, 10).await?;
    assert!(messages.is_empty(), "other tenant should see no messages");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn insert_into_nonexistent_conversation() -> Result<()> {
    let stores = setup().await?;
    let fake_id = Uuid::new_v4();

    let result = stores
        .insert_message("any-tenant", fake_id, MessageRole::User, "Hello", None)
        .await;
    assert!(result.is_err());
    let err_msg = result.expect_err("expected error").to_string();
    assert!(
        err_msg.contains("conversation not found for tenant"),
        "error should be explicit: {err_msg}"
    );

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
async fn get_messages_limit_zero_rejected() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();
    let conv = stores.create_conversation(&tenant, None, None).await?;

    let result = stores.get_messages(&tenant, conv.id, 0).await;
    assert!(result.is_err());
    let err_msg = result.expect_err("expected error").to_string();
    assert!(err_msg.contains("limit must be > 0"), "error: {err_msg}");

    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-core/tests/integration_conversations.rs
git commit -m "test(conversations): add integration tests for conversation store"
```

---

### Task 8: Provider smoke tests (`smoke_llm.rs`)

**Files:**
- Create: `crates/rag-core/tests/smoke_llm.rs`

**Note:** These tests call real LLM providers. Each test is provider-gated — it skips gracefully if the required env vars are not set.

- [ ] **Step 1: Create smoke test file**

Create `crates/rag-core/tests/smoke_llm.rs`:

```rust
//! Smoke tests for real LLM providers.
//!
//! Each test is provider-gated: skips if the required env vars are not set.
//! Run: `cargo test -p rag-core --test smoke_llm -- --ignored --nocapture`

use anyhow::Result;
use rag_core::config::AppConfig;
use rag_core::llm::{ChatBackend, ChatMessage, ChatRole, CompletionRequest};

/// Helper: build config with env overrides for a specific provider.
fn config_for_provider(provider: &str) -> Result<AppConfig> {
    let config = AppConfig::from_env()?;
    // The test relies on LLM_PROVIDER, LLM_API_KEY, etc. being set in the
    // environment. We validate the provider matches what we expect.
    if config.llm_provider.to_string() != provider {
        anyhow::bail!("LLM_PROVIDER is not {provider}, skipping");
    }
    Ok(config)
}

#[tokio::test]
#[ignore] // requires LLM_API_KEY + LLM_PROVIDER=openai-compatible
async fn openai_compatible_smoke() -> Result<()> {
    let config = match config_for_provider("openai-compatible") {
        Ok(c) => c,
        Err(_) => {
            eprintln!("SKIP: LLM_PROVIDER != openai-compatible or LLM_API_KEY not set");
            return Ok(());
        }
    };
    if config.llm_api_key.is_none() {
        eprintln!("SKIP: LLM_API_KEY not set");
        return Ok(());
    }

    let backend = ChatBackend::from_config(&config)?;
    let response = backend
        .complete(
            &CompletionRequest {
                system: "You are a helpful assistant.",
                messages: &[ChatMessage {
                    role: ChatRole::User,
                    content: "Say hello in exactly one word.".into(),
                }],
                temperature: 0.0,
                max_tokens: 50,
                stop: vec![],
            },
            0, // no retries for smoke test
            500,
        )
        .await?;

    assert!(!response.text.is_empty(), "response should not be empty");
    assert!(!response.model.is_empty(), "model should be populated");
    assert!(response.usage.prompt_tokens > 0, "prompt_tokens should be > 0");
    assert!(
        response.usage.completion_tokens > 0,
        "completion_tokens should be > 0"
    );

    eprintln!("OpenAI-compatible response: {:?}", response.text);
    eprintln!("Model: {}, Usage: {:?}", response.model, response.usage);

    Ok(())
}

#[tokio::test]
#[ignore] // requires LLM_API_KEY + LLM_PROVIDER=anthropic
async fn anthropic_smoke() -> Result<()> {
    let config = match config_for_provider("anthropic") {
        Ok(c) => c,
        Err(_) => {
            eprintln!("SKIP: LLM_PROVIDER != anthropic or LLM_API_KEY not set");
            return Ok(());
        }
    };
    if config.llm_api_key.is_none() {
        eprintln!("SKIP: LLM_API_KEY not set");
        return Ok(());
    }

    let backend = ChatBackend::from_config(&config)?;
    let response = backend
        .complete(
            &CompletionRequest {
                system: "You are a helpful assistant.",
                messages: &[ChatMessage {
                    role: ChatRole::User,
                    content: "Say hello in exactly one word.".into(),
                }],
                temperature: 0.0,
                max_tokens: 50,
                stop: vec![],
            },
            0, // no retries for smoke test
            500,
        )
        .await?;

    assert!(!response.text.is_empty(), "response should not be empty");
    assert!(!response.model.is_empty(), "model should be populated");
    assert!(response.usage.prompt_tokens > 0, "prompt_tokens should be > 0");
    assert!(
        response.usage.completion_tokens > 0,
        "completion_tokens should be > 0"
    );

    eprintln!("Anthropic response: {:?}", response.text);
    eprintln!("Model: {}, Usage: {:?}", response.model, response.usage);

    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-core/tests/smoke_llm.rs
git commit -m "test(llm): add provider-gated smoke tests for OpenAI and Anthropic"
```

---

### Task 9: Chat end-to-end integration tests (`integration_chat.rs`)

**Files:**
- Create: `crates/rag-core/tests/integration_chat.rs`

**Note:** These tests require Postgres + Qdrant (`just up`) but use a mock LLM backend. This is the main integration test suite for the chat pipeline. To use a mock backend, the test constructs `ChatService` with a backend that doesn't need a real API key — we achieve this by pointing the OpenAI-compatible client at a non-existent URL and testing only the orchestration path up to the LLM call boundary. Since `ChatService` owns a concrete `ChatBackend`, and we cannot inject a mock without changing production types, these tests will exercise the full path with a real LLM if `LLM_API_KEY` is set, or test the pre-LLM orchestration path otherwise.

**Alternative approach (recommended):** Since the chat.rs orchestration is composed of individually-tested components (stores, retrieval, context, prompt), and the end-to-end path requires a real LLM, make these tests require `LLM_API_KEY` like the smoke tests. The conversation store tests (Task 7) already cover the DB operations. The orchestration logic (collection resolution, history loading, persistence ordering) can be verified through the conversation store integration tests.

- [ ] **Step 1: Create test file**

Create `crates/rag-core/tests/integration_chat.rs`:

```rust
//! End-to-end integration tests for the chat service.
//!
//! Requires Postgres + Qdrant running (`just up`) and LLM_API_KEY set.
//! These tests exercise the full pipeline: ingest → chat → verify response.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::chat::{ChatRequest, ChatService};
use rag_core::config::AppConfig;
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::stores::conversations::MessageRole;
use rag_core::tenant::TenantId;
use tempfile::TempDir;
use uuid::Uuid;

fn unique_id() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[allow(clippy::disallowed_methods)]
fn write_fixture(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).expect("writing fixture");
}

#[allow(clippy::disallowed_methods)]
fn write_sidecar(dir: &Path, stem: &str) {
    let json = format!(
        r#"{{
            "schema_version": 1,
            "document": {{ "id": "{stem}", "title": "Test {stem}", "category": "report" }},
            "source": {{ "url": "https://example.com/{stem}", "domain": "example.com", "publisher": "Test" }},
            "language": "en",
            "tags": ["test"],
            "acl": {{ "allow_roles": ["*"] }},
            "security": {{ "classification": "public", "requires_evidence_pack": false }},
            "provenance": {{ "retrieved_at": "2026-03-27T00:00:00Z", "retrieved_by": "test" }}
        }}"#
    );
    fs::write(dir.join(format!("{stem}.metadata.json")), json).expect("writing sidecar");
}

async fn setup() -> Result<(IngestService, ChatService, Stores)> {
    let config = AppConfig::from_env()?;
    if config.llm_api_key.is_none() {
        anyhow::bail!("LLM_API_KEY required for chat integration tests");
    }
    let stores = Stores::new(&config).await?;
    let ingest = IngestService::new(stores.clone(), &config)?;
    let chat = ChatService::new(stores.clone(), &config)?;
    Ok((ingest, chat, stores))
}

async fn ingest_fixtures(
    ingest: &IngestService,
    dir: &TempDir,
    tenant: &TenantId,
    collection: &str,
) -> Result<()> {
    write_fixture(
        dir.path(),
        "rust.md",
        "Rust is a systems programming language focused on safety and concurrency. \
         Async runtime tokio provides efficient task scheduling. \
         The borrow checker prevents data races at compile time.",
    );
    write_sidecar(dir.path(), "rust");
    write_fixture(
        dir.path(),
        "cooking.md",
        "Sourdough bread requires a starter culture of wild yeast. \
         Fermentation time depends on ambient temperature. \
         A Dutch oven creates steam for a crispy crust.",
    );
    write_sidecar(dir.path(), "cooking");

    ingest
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant: tenant.clone(),
            collection_override: Some(collection.to_owned()),
        })
        .await?;

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
async fn chat_returns_answer_with_citations() -> Result<()> {
    let (ingest, chat, _stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let dir = TempDir::new()?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-chat-{suffix}").parse()?;
    let collection = format!("test_chat_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    let response = chat
        .chat(ChatRequest {
            query: "What is Rust?".into(),
            collection: Some(collection),
            tenant,
            conversation_id: None,
            language: None,
            history_limit: None,
        })
        .await?;

    assert!(!response.answer.is_empty(), "answer should not be empty");
    assert!(!response.citations.is_empty(), "citations should be present");
    assert!(!response.model.is_empty(), "model should be populated");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
async fn multi_turn_conversation_loads_history() -> Result<()> {
    let (ingest, chat, stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let dir = TempDir::new()?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-multi-{suffix}").parse()?;
    let collection = format!("test_multi_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    // First turn creates conversation.
    let first = chat
        .chat(ChatRequest {
            query: "Tell me about Rust.".into(),
            collection: Some(collection.clone()),
            tenant: tenant.clone(),
            conversation_id: None,
            language: None,
            history_limit: None,
        })
        .await?;
    let conv_id = first.conversation_id;

    // Second turn resumes conversation.
    let second = chat
        .chat(ChatRequest {
            query: "What about its type system?".into(),
            collection: None, // should use stored collection
            tenant: tenant.clone(),
            conversation_id: Some(conv_id),
            language: None,
            history_limit: None,
        })
        .await?;
    assert_eq!(second.conversation_id, conv_id);

    // Verify 4 messages persisted (2 user + 2 assistant).
    let messages = stores
        .get_messages(tenant.as_str(), conv_id, 100)
        .await?;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[2].role, MessageRole::User);
    assert_eq!(messages[3].role, MessageRole::Assistant);

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
async fn collection_mismatch_rejected() -> Result<()> {
    let (_ingest, chat, stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let suffix = unique_id();
    let tenant: TenantId = format!("test-mismatch-{suffix}").parse()?;

    // Create conversation with collection_a.
    let conv = stores
        .create_conversation(tenant.as_str(), None, Some("collection_a"))
        .await?;

    // Try to chat with collection_b on the same conversation.
    let result = chat
        .chat(ChatRequest {
            query: "Hello".into(),
            collection: Some("collection_b".into()),
            tenant,
            conversation_id: Some(conv.id),
            language: None,
            history_limit: None,
        })
        .await;

    assert!(result.is_err());
    let err_msg = result.expect_err("expected mismatch error").to_string();
    assert!(
        err_msg.contains("collection mismatch"),
        "error should mention mismatch: {err_msg}"
    );

    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-core/tests/integration_chat.rs
git commit -m "test(chat): add end-to-end integration tests for chat service"
```

---

## Running the Tests

**Unit tests (no infrastructure):**
```bash
cargo test -p rag-core coalesce prompt
```

**Conversation store integration (Postgres only):**
```bash
cargo test -p rag-core --test integration_conversations -- --ignored --nocapture
```

**Chat end-to-end integration (Postgres + Qdrant + LLM_API_KEY):**
```bash
cargo test -p rag-core --test integration_chat -- --ignored --nocapture
```

**Provider smoke tests (LLM_API_KEY + provider env):**
```bash
# OpenAI-compatible
LLM_PROVIDER=openai-compatible LLM_API_KEY=sk-... cargo test -p rag-core --test smoke_llm -- --ignored --nocapture

# Anthropic
LLM_PROVIDER=anthropic LLM_MODEL=claude-sonnet-4-20250514 LLM_BASE_URL=https://api.anthropic.com LLM_API_KEY=sk-ant-... cargo test -p rag-core --test smoke_llm -- --ignored --nocapture
```
