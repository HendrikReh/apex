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
#[allow(clippy::disallowed_methods)] // .expect() used intentionally in test assertions
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
    assert!(response.usage.completion_tokens > 0, "completion_tokens should be > 0");

    eprintln!("OpenAI-compatible response: {:?}", response.text);
    eprintln!("Model: {}, Usage: {:?}", response.model, response.usage);

    Ok(())
}

#[tokio::test]
#[ignore] // requires LLM_API_KEY + LLM_PROVIDER=anthropic
#[allow(clippy::disallowed_methods)] // .expect() used intentionally in test assertions
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
    assert!(response.usage.completion_tokens > 0, "completion_tokens should be > 0");

    eprintln!("Anthropic response: {:?}", response.text);
    eprintln!("Model: {}, Usage: {:?}", response.model, response.usage);

    Ok(())
}
