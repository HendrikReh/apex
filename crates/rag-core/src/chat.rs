//! Chat service orchestrating retrieval, context assembly, and LLM completion.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::context::{Citation, ContextBuilder, ContextConfig, DedupeStrategy};
use crate::fusion::FusedChunk;
use crate::llm::{ChatBackend, ChatMessage, ChatRole, CompletionRequest, TokenUsage};
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

#[derive(Debug, Clone)]
pub struct GroundedAnswer {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub evidence: Vec<FusedChunk>,
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

fn build_summary_context(
    context_builder: &ContextBuilder,
    defaults: &ChatDefaults,
    chunks: Vec<FusedChunk>,
) -> String {
    let context = context_builder.build(
        chunks,
        &ContextConfig {
            max_tokens: defaults.context_max_tokens,
            max_chunks: defaults.context_max_chunks,
            // Agentic retrieval may intentionally expand multiple chunks from
            // the same document, so preserve chunk-level evidence here.
            dedupe_strategy: DedupeStrategy::ByChunkId,
            include_citations: false,
        },
    );
    render_context_chunks(&context.chunks)
}

impl ChatService {
    /// Construct a ChatService from config and a shared Stores handle.
    ///
    /// Clones `stores` for `RetrievalService` (cheap: PgPool is Arc-based).
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let backend = ChatBackend::from_config(config).context("building LLM backend")?;
        let retrieval =
            RetrievalService::new(stores.clone(), config).context("building retrieval service")?;
        let context_builder = ContextBuilder::new();
        let prompt_renderer = PromptRenderer::from_file(&config.llm_prompt_template_path)
            .context("loading prompt template")?;
        let defaults = ChatDefaults::from_config(config);

        Ok(Self { backend, retrieval, context_builder, prompt_renderer, stores, defaults })
    }

    /// Construct a ChatService with a mock LLM backend.
    ///
    /// Intended for integration tests in downstream crates. The mock backend
    /// returns `response` verbatim on every chat call, with no network IO.
    pub fn with_mock_llm(stores: Stores, config: &AppConfig, response: String) -> Result<Self> {
        let backend = ChatBackend::Mock { response };
        let retrieval =
            RetrievalService::new(stores.clone(), config).context("building retrieval service")?;
        let context_builder = ContextBuilder::new();
        let prompt_renderer = PromptRenderer::from_file(&config.llm_prompt_template_path)
            .context("loading prompt template")?;
        let defaults = ChatDefaults::from_config(config);

        Ok(Self { backend, retrieval, context_builder, prompt_renderer, stores, defaults })
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
        let (conversation_id, collection) = self.resolve_conversation(tenant, &request).await?;

        // Step 2: Load history.
        let history_rows = self
            .stores
            .get_messages(tenant, conversation_id, history_limit)
            .await
            .context("loading conversation history")?;
        let messages: Vec<ChatMessage> = history_rows
            .into_iter()
            .filter_map(|row| match row.role {
                MessageRole::User => {
                    Some(ChatMessage { role: ChatRole::User, content: row.content })
                }
                MessageRole::Assistant => {
                    Some(ChatMessage { role: ChatRole::Assistant, content: row.content })
                }
                // Persisted system rows are valid per the schema, but the LLM
                // request carries the authoritative system prompt separately.
                MessageRole::System => None,
            })
            .collect();

        let grounded = self
            .answer_with_retrieval(
                &request.query,
                &collection,
                tenant,
                &messages,
                request.language.as_deref(),
            )
            .await?;

        // Step 7: Persist user + assistant messages atomically (after LLM
        // success to avoid orphaned user messages on failure).
        self.stores
            .insert_chat_turn(tenant, conversation_id, &request.query, &grounded.answer)
            .await
            .context("persisting chat turn")?;

        // Step 8: Return response.
        Ok(ChatResponse {
            answer: grounded.answer,
            conversation_id,
            citations: grounded.citations,
            usage: grounded.usage,
            model: grounded.model,
        })
    }

    pub async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> Result<GroundedAnswer> {
        self.answer_with_retrieval(query, collection, tenant, &[], language).await
    }

    /// Summarize a set of already-retrieved chunks for agent orchestration.
    ///
    /// This avoids re-running retrieval and reuses the configured LLM backend,
    /// including the mock backend used by integration tests.
    pub async fn summarize_chunks(&self, query: &str, chunks: Vec<FusedChunk>) -> Result<String> {
        let context_text = build_summary_context(&self.context_builder, &self.defaults, chunks);
        let system_prompt = format!(
            "You are an analyst summarizing retrieved context for an agent workflow.\n\
             Summarize only what is supported by the provided context.\n\
             If the context is empty, say so plainly.\n\n\
             Context:\n{context_text}"
        );
        let messages = [ChatMessage { role: ChatRole::User, content: query.to_string() }];
        let response = self
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
            .context("agent chunk summarization")?;
        Ok(response.text)
    }

    async fn answer_with_retrieval(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        messages: &[ChatMessage],
        language: Option<&str>,
    ) -> Result<GroundedAnswer> {
        let fused = self
            .retrieval
            .search_hybrid(collection, query, tenant, None)
            .await
            .context("hybrid retrieval")?;

        let context_config = ContextConfig {
            max_tokens: self.defaults.context_max_tokens,
            max_chunks: self.defaults.context_max_chunks,
            dedupe_strategy: DedupeStrategy::ByDocId,
            include_citations: true,
        };
        let context_result = self.context_builder.build(fused.clone(), &context_config);
        let citations = context_result.citations.clone();
        let fused_by_chunk_id: HashMap<String, FusedChunk> =
            fused.into_iter().map(|chunk| (chunk.chunk_id.clone(), chunk)).collect();
        let evidence = context_result
            .chunks
            .iter()
            .filter_map(|chunk| fused_by_chunk_id.get(&chunk.chunk_id).cloned())
            .collect();
        let context_text = render_context_chunks(&context_result.chunks);
        let language_instruction =
            language.map(|lang| format!("Respond in {lang}.")).unwrap_or_default();
        let system_prompt = self
            .prompt_renderer
            .render_system_prompt(&PromptContext {
                context: &context_text,
                language_instruction: &language_instruction,
            })
            .context("rendering system prompt")?;

        let mut llm_messages = messages.to_vec();
        llm_messages.push(ChatMessage { role: ChatRole::User, content: query.to_string() });

        let llm_response = self
            .backend
            .complete(
                &CompletionRequest {
                    system: &system_prompt,
                    messages: &llm_messages,
                    temperature: self.defaults.temperature,
                    max_tokens: self.defaults.max_tokens,
                    stop: vec![],
                },
                self.defaults.max_retries,
                self.defaults.retry_backoff_ms,
            )
            .await
            .context("LLM completion")?;

        Ok(GroundedAnswer {
            answer: llm_response.text,
            citations,
            evidence,
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
                        anyhow::anyhow!("conversation not found: id={conv_id}, tenant={tenant}")
                    })?;

                let stored_collection = conv.collection.unwrap_or_default();
                let collection = match &request.collection {
                    Some(req_coll) => {
                        if req_coll.is_empty() {
                            bail!("collection must not be empty");
                        }
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
                let collection = request.collection.clone().ok_or_else(|| {
                    anyhow::anyhow!(
                        "collection is required for the first message in a conversation"
                    )
                })?;
                if collection.is_empty() {
                    bail!("collection must not be empty for the first message in a conversation");
                }

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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn summary_defaults() -> ChatDefaults {
        ChatDefaults {
            temperature: 0.1,
            max_tokens: 4096,
            context_max_tokens: 8_000,
            context_max_chunks: 50,
            history_limit: 30,
            max_retries: 3,
            retry_backoff_ms: 500,
        }
    }

    fn fused_chunk(id: &str, doc: &str, index: i32, score: f32, text: &str) -> FusedChunk {
        FusedChunk {
            chunk_id: id.to_string(),
            document_id: doc.to_string(),
            chunk_index: index,
            text: text.to_string(),
            title: Some(format!("title-{doc}")),
            source_url: Some(format!("https://example.com/{doc}")),
            source_domain: Some("example.com".to_string()),
            language: Some("en".to_string()),
            tags: vec!["test".to_string()],
            section_heading: Some("Section".to_string()),
            collection: Some("docs".to_string()),
            fused_score: score,
            score_type: "rrf_fused".to_string(),
            sources: vec!["dense".to_string()],
            source_scores: HashMap::from([("dense".to_string(), score)]),
        }
    }

    #[test]
    fn build_summary_context_keeps_same_document_neighbors() {
        let context_text = build_summary_context(
            &ContextBuilder::new(),
            &summary_defaults(),
            vec![
                fused_chunk("chunk-1", "doc-1", 7, 0.9, "anchor chunk"),
                fused_chunk("chunk-2", "doc-1", 8, 0.8, "neighbor chunk"),
            ],
        );

        assert!(context_text.contains("anchor chunk"));
        assert!(context_text.contains("neighbor chunk"));
    }
}
