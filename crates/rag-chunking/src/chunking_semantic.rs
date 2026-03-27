//! LLM-guided semantic chunker.
//!
//! Sends text to an OpenAI-compatible model to suggest chunk boundaries.
//! Falls back to the token chunker on error, timeout, or when the input
//! exceeds the configured character limit.

use crate::{chunking_token::chunk_text_tokens, settings};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    sync::OnceLock,
};
use tokio::sync::{OnceCell, Semaphore};
use tokio::time::timeout;
use tracing::{info, warn};

static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static CLIENT: OnceLock<async_openai::Client<async_openai::config::OpenAIConfig>> = OnceLock::new();
static SEMANTIC_PROMPT_TEMPLATE: OnceCell<String> = OnceCell::const_new();

const MAX_SEMANTIC_CHUNKS: usize = 128;
const DEFAULT_SEMANTIC_PROMPT_PATH: &str = "prompts/semantic_chunking.txt";
const SEMANTIC_PROMPT_PATH_ENV: &str = "RAG_SEMANTIC_CHUNKING_PROMPT_PATH";

fn semantic_model_is_unset_or_blank(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unset")
}

/// Semantic chunking using the configured OpenAI model.
/// Falls back to token chunking on error or large input.
#[allow(clippy::disallowed_methods)]
pub async fn chunk_text_semantic(
    text: &str,
    max_tokens: usize,
    overlap_ratio: f32,
    semantic_max_chars: usize,
) -> Vec<String> {
    let cfg = settings::settings();
    chunk_text_semantic_with_settings(text, max_tokens, overlap_ratio, semantic_max_chars, cfg)
        .await
}

async fn chunk_text_semantic_with_settings(
    text: &str,
    max_tokens: usize,
    overlap_ratio: f32,
    semantic_max_chars: usize,
    cfg: settings::ChunkingSettings,
) -> Vec<String> {
    if !cfg.semantic_enabled || semantic_globally_disabled() {
        info!("semantic chunking disabled via config or env; falling back to token chunker");
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }
    if text.len() > semantic_max_chars {
        info!(
            "semantic chunking skipped: text length {} exceeds limit {}, falling back",
            text.len(),
            semantic_max_chars
        );
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }
    let model = cfg.semantic_model;
    if semantic_model_is_unset_or_blank(&model) {
        info!("semantic chunking skipped: semantic_model unset/blank; falling back");
        return chunk_text_tokens(text, max_tokens, overlap_ratio);
    }
    let prompt_path = semantic_prompt_path();
    let prompt_template = match load_semantic_prompt_template().await {
        Ok(template) => template,
        Err(err) => {
            warn!(
                "semantic chunking prompt load failed from {}: {err}; falling back",
                prompt_path.display()
            );
            return chunk_text_tokens(text, max_tokens, overlap_ratio);
        }
    };
    let semaphore =
        SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(cfg.semantic_concurrency.max(1)))).clone();
    let _permit = match semaphore.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            warn!("semantic semaphore closed; falling back to token chunker");
            return chunk_text_tokens(text, max_tokens, overlap_ratio);
        }
    };
    let semantic_timeout = cfg.semantic_timeout_secs;
    let client = CLIENT.get_or_init(async_openai::Client::new);
    let safe_text = text.replace("]]>", "]]]]><![CDATA[>");
    let prompt =
        render_semantic_prompt(&prompt_template, max_tokens, MAX_SEMANTIC_CHUNKS, &safe_text);

    let req = async_openai::types::chat::CreateChatCompletionRequestArgs::default()
        .model(model)
        .messages({
            let message =
                async_openai::types::chat::ChatCompletionRequestUserMessageArgs::default()
                    .content(prompt)
                    .build();
            match message {
                Ok(msg) => vec![async_openai::types::chat::ChatCompletionRequestMessage::User(msg)],
                Err(err) => {
                    warn!("semantic chunking request build failed: {err}");
                    return chunk_text_tokens(text, max_tokens, overlap_ratio);
                }
            }
        })
        .build();

    let response_content = match req {
        Ok(req) => {
            match timeout(
                std::time::Duration::from_secs(semantic_timeout),
                client.chat().create(req),
            )
            .await
            {
                Ok(Ok(resp)) => {
                    resp.choices.first().and_then(|choice| choice.message.content.clone())
                }
                Ok(Err(err)) => {
                    warn!("semantic chunking API call failed: {err}");
                    None
                }
                Err(_) => {
                    warn!("semantic chunking timed out after {}s", semantic_timeout);
                    None
                }
            }
        }
        Err(err) => {
            warn!("semantic chunking request build failed: {err}");
            None
        }
    };

    if let Some(chunks) = response_content.and_then(|c| parse_semantic_response(&c)) {
        return chunks;
    }

    warn!("semantic chunking failed or returned empty; falling back to token chunker");
    chunk_text_tokens(text, max_tokens, overlap_ratio)
}

async fn load_semantic_prompt_template() -> std::io::Result<String> {
    if let Ok(path) = std::env::var(SEMANTIC_PROMPT_PATH_ENV) {
        return tokio::fs::read_to_string(Path::new(&path)).await;
    }

    SEMANTIC_PROMPT_TEMPLATE
        .get_or_try_init(|| async {
            tokio::fs::read_to_string(&default_semantic_prompt_path()).await
        })
        .await
        .cloned()
}

fn semantic_prompt_path() -> PathBuf {
    std::env::var(SEMANTIC_PROMPT_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_semantic_prompt_path())
}

fn default_semantic_prompt_path() -> PathBuf {
    settings::resolve_config_relative_path(Path::new(DEFAULT_SEMANTIC_PROMPT_PATH))
}

fn render_semantic_prompt(
    template: &str,
    max_tokens: usize,
    max_chunks: usize,
    document: &str,
) -> String {
    template
        .replace("{{max_tokens}}", &max_tokens.to_string())
        .replace("{{max_chunks}}", &max_chunks.to_string())
        .replace("{{document}}", document)
}

fn parse_semantic_response(content: &str) -> Option<Vec<String>> {
    let parsed: Vec<String> = serde_json::from_str(content).ok()?;
    let filtered: Vec<String> =
        parsed.into_iter().filter(|c| !c.trim().is_empty()).take(MAX_SEMANTIC_CHUNKS).collect();
    if filtered.is_empty() { None } else { Some(filtered) }
}

fn semantic_globally_disabled() -> bool {
    std::env::var("CHUNKING_SEMANTIC_DISABLED")
        .ok()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)]
    use super::*;
    use std::sync::OnceLock;

    static ENV_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await
    }

    fn set_env_var(key: &str, value: &str) {
        // SAFETY: test-only; env restored via SemanticEnvGuard.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_var(key: &str) {
        // SAFETY: test-only; env restored via SemanticEnvGuard.
        unsafe { std::env::remove_var(key) };
    }

    #[tokio::test]
    async fn semantic_chunker_falls_back_on_error() {
        let long_text = "x".repeat(20_000);
        let expected = chunk_text_tokens(&long_text, 5, 0.0);
        let chunks = chunk_text_semantic(&long_text, 5, 0.0, 12_000).await;
        assert_eq!(chunks, expected);
    }

    struct SemanticEnvGuard {
        prev_disabled: Option<String>,
        prev_prompt_path: Option<String>,
    }

    impl SemanticEnvGuard {
        fn new() -> Self {
            Self {
                prev_disabled: std::env::var("CHUNKING_SEMANTIC_DISABLED").ok(),
                prev_prompt_path: std::env::var("RAG_SEMANTIC_CHUNKING_PROMPT_PATH").ok(),
            }
        }
    }

    impl Drop for SemanticEnvGuard {
        fn drop(&mut self) {
            match &self.prev_disabled {
                Some(val) => set_env_var("CHUNKING_SEMANTIC_DISABLED", val),
                None => remove_env_var("CHUNKING_SEMANTIC_DISABLED"),
            }
            match &self.prev_prompt_path {
                Some(val) => set_env_var("RAG_SEMANTIC_CHUNKING_PROMPT_PATH", val),
                None => remove_env_var("RAG_SEMANTIC_CHUNKING_PROMPT_PATH"),
            }
        }
    }

    #[tokio::test]
    async fn semantic_chunker_respects_disable_flag() {
        let _lock = env_lock().await;
        let _guard = SemanticEnvGuard::new();
        set_env_var("CHUNKING_SEMANTIC_DISABLED", "1");
        let text = "short text that would otherwise trigger semantic chunking";
        let expected = chunk_text_tokens(text, 50, 0.0);
        let chunks = chunk_text_semantic(text, 50, 0.0, 20_000).await;
        assert_eq!(chunks, expected);
    }

    #[test]
    fn build_semantic_prompt_uses_template_placeholders() {
        let template = "Target {{max_tokens}} tokens; max {{max_chunks}} chunks.\n{{document}}";
        let prompt = render_semantic_prompt(template, 512, MAX_SEMANTIC_CHUNKS, "hello world");
        assert_eq!(prompt, "Target 512 tokens; max 128 chunks.\nhello world");
    }

    #[tokio::test]
    async fn semantic_chunker_falls_back_when_prompt_file_is_missing() {
        let _lock = env_lock().await;
        let _guard = SemanticEnvGuard::new();
        set_env_var(
            "RAG_SEMANTIC_CHUNKING_PROMPT_PATH",
            "/definitely/missing/semantic_chunking.txt",
        );

        let text = "short text that would otherwise trigger semantic chunking";
        let expected = chunk_text_tokens(text, 50, 0.0);
        let cfg = crate::settings::ChunkingSettings {
            semantic_model: "gpt-5-mini".to_string(),
            semantic_enabled: true,
            semantic_concurrency: 1,
            semantic_timeout_secs: 1,
            ..Default::default()
        };

        let chunks = chunk_text_semantic_with_settings(text, 50, 0.0, 20_000, cfg).await;
        assert_eq!(chunks, expected);
    }

    #[test]
    fn default_semantic_prompt_path_uses_config_directory() {
        let path = settings::resolve_config_relative_path_from(
            Path::new("/srv/project/config/app.toml"),
            Path::new(DEFAULT_SEMANTIC_PROMPT_PATH),
        );
        assert_eq!(path, Path::new("/srv/project/config/prompts/semantic_chunking.txt"));
    }

    #[test]
    fn parse_semantic_response_caps_chunk_count() {
        let chunks: Vec<String> = (0..300).map(|i| format!("Chunk {i}")).collect();
        let json = serde_json::to_string(&chunks).expect("serialize");
        let result = parse_semantic_response(&json).expect("should parse");
        assert_eq!(result.len(), MAX_SEMANTIC_CHUNKS);
    }

    #[test]
    fn parse_semantic_response_filters_empty() {
        let json = r#"["hello", "", "  ", "world"]"#;
        let result = parse_semantic_response(json).expect("should parse");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn parse_semantic_response_returns_none_on_invalid_json() {
        assert!(parse_semantic_response("not json").is_none());
    }

    #[test]
    fn parse_semantic_response_returns_none_on_all_empty() {
        assert!(parse_semantic_response(r#"["", "  "]"#).is_none());
    }

    #[test]
    fn semantic_model_guard_handles_unset_and_blank_values() {
        assert!(semantic_model_is_unset_or_blank(""));
        assert!(semantic_model_is_unset_or_blank("   "));
        assert!(semantic_model_is_unset_or_blank("unset"));
        assert!(semantic_model_is_unset_or_blank("UnSeT"));
        assert!(!semantic_model_is_unset_or_blank("gpt-5-mini"));
    }
}
