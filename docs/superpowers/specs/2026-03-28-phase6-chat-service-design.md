# Phase 6: Chat Service

**Date:** 2026-03-28
**Status:** Approved
**Scope:** Provider-agnostic LLM chat with context-grounded responses, conversation persistence (messages only), and Handlebars prompt templating.
**Approach:** `ChatBackend` enum with OpenAI-compatible and native Anthropic variants. Single `ChatService` orchestrator owns the full flow.

---

## 1. LLM Backend (`llm.rs`)

### 1.1 ChatBackend Enum

```rust
pub enum ChatBackend {
    OpenAiCompatible {
        client: async_openai::Client<OpenAIConfig>,
        model: String,
    },
    Anthropic {
        client: reqwest::Client,  // owns timeout, default headers (x-api-key, anthropic-version)
        model: String,
        base_url: String,
    },
}
```

Both variants own their HTTP client, constructed once at startup with all connection settings baked in.

**OpenAI-compatible:** `async_openai::Client` built with `OpenAIConfig::new().with_api_key(...).with_api_base(...)`. The base URL is baked into the client — this is what makes it truly "compatible" with xAI, Groq, Together, Ollama, vLLM, etc. No separate `base_url` field on the variant.

**Anthropic:** `reqwest::Client` built with timeout, default headers (`x-api-key`, `anthropic-version: 2023-06-01`). POST to `{base_url}/v1/messages`. Response JSON parsed manually.

### 1.2 CompletionRequest and LlmResponse

```rust
pub struct CompletionRequest<'a> {
    pub system: &'a str,              // separate, authoritative system prompt
    pub messages: &'a [ChatMessage],  // user/assistant only
    pub temperature: f32,
    pub max_tokens: u32,
    pub stop: Vec<String>,
}

pub struct ChatMessage {
    pub role: ChatRole,  // User or Assistant only
    pub content: String,
}

pub enum ChatRole { User, Assistant }

pub struct LlmResponse {
    pub text: String,
    pub usage: TokenUsage,
    pub model: String,  // actual model ID returned by provider
}

pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}
```

### 1.3 complete Method

```rust
impl ChatBackend {
    pub async fn complete(&self, request: &CompletionRequest<'_>) -> Result<LlmResponse>
}
```

**Provider mapping:**
- OpenAI-compatible: `system` → `ChatCompletionRequestSystemMessage`, messages → user/assistant messages in order.
- Anthropic: `system` → top-level `system` field in request body, messages → `messages` array. Consecutive same-role messages coalesced before dispatch (Anthropic requires strict alternation).

### 1.4 Message Validation (Pure Function)

Applied before provider dispatch:
- Reject roles other than `User`/`Assistant` (structurally enforced by `ChatRole` enum, but validated if constructed from external input).
- Coalesce consecutive same-role messages: join content with newline (`\n`). This is required for Anthropic and harmless for OpenAI-compatible.

### 1.5 Retry

Exponential backoff with configurable `max_retries` and `retry_backoff_ms`. Retries on transient HTTP errors (429, 500, 502, 503) only. Non-retryable errors (400, 401, 403) propagate immediately.

---

## 2. Prompt Template (`prompt.rs`)

### 2.1 PromptRenderer

```rust
pub struct PromptRenderer {
    handlebars: Handlebars<'static>,
}
```

Constructed with a path to the template file. Loads and compiles the template once at startup. Fails fast if the file is missing or the template is invalid.

### 2.2 Rendering

```rust
pub fn render_system_prompt(&self, context: &PromptContext) -> Result<String>

pub struct PromptContext<'a> {
    pub context_chunks: &'a str,        // pre-rendered numbered chunks
    pub language_instruction: &'a str,  // e.g. "Respond in English" or ""
}
```

### 2.3 Context Chunk Rendering

Standalone function, not part of `PromptRenderer`:

```rust
pub fn render_context_chunks(chunks: &[ContextChunk]) -> String
```

Produces numbered format: `[1] <text>\n---\n[2] <text>\n---\n...`. Empty input produces empty string.

This format corresponds to the `[N]` inline citation markers the prompt template instructs the LLM to use.

### 2.4 Template File

`config/prompts/chat_system.hbs`:
- Instructs LLM to answer using provided context
- Citation rules: use `[N]` inline references matching numbered chunks
- Fallback: if context is empty, state honestly that no relevant context was found
- `{{context}}` placeholder for numbered chunks
- `{{language_instruction}}` placeholder

---

## 3. Conversation Store (`stores/conversations.rs`)

### 3.1 Types

```rust
pub enum MessageRole { User, Assistant }

pub struct ConversationRow {
    pub id: Uuid,
    pub tenant: String,
    pub title: Option<String>,
    pub collection: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<Utc>,
}
```

`MessageRole` serializes to/from `"user"`/`"assistant"` in the DB text column. `MessageRole` (store layer) and `ChatRole` (LLM layer) are separate enums with the same variants. The conversion happens in `chat.rs` when loading history: `MessageRow` → `ChatMessage`.

### 3.2 Store Methods

```rust
create_conversation(tenant, title, collection) -> Result<ConversationRow>
get_conversation(tenant, conversation_id) -> Result<Option<ConversationRow>>
insert_message(tenant, conversation_id, role: MessageRole, content, metadata) -> Result<MessageRow>
get_messages(tenant, conversation_id, limit: i64) -> Result<Vec<MessageRow>>
```

**`insert_message`:** Validates tenant ownership via `INSERT INTO messages (...) SELECT ... FROM conversations WHERE tenant = $1 AND id = $2`. Returns an explicit error ("conversation not found for tenant") if the conversation doesn't exist or belongs to a different tenant. Validates `limit > 0` at the API boundary.

**`get_messages`:** Inner query selects latest N by `created_at DESC, id DESC`, outer query re-orders ascending. Stable tie-breaking, chronological output.

### 3.3 Scope

No user, session, or run management — deferred to Phase 10 (Auth and RBAC).

---

## 4. ChatService (`chat.rs`)

### 4.1 Struct

```rust
pub struct ChatService {
    backend: ChatBackend,
    retrieval: RetrievalService,
    context_builder: ContextBuilder,
    prompt_renderer: PromptRenderer,
    stores: Stores,
    defaults: ChatDefaults,
}

pub struct ChatDefaults {
    pub temperature: f32,
    pub max_tokens: u32,
    pub context_max_tokens: usize,
    pub context_max_chunks: usize,
    pub history_limit: i64,  // default 30
}
```

**Stores sharing:** `ChatService` is constructed with one `Stores` handle. `RetrievalService` is built from that same handle — not from a fresh `Stores::new()`. Only one `Stores::new()` call happens at application startup.

### 4.2 Request/Response

```rust
pub struct ChatRequest {
    pub query: String,
    pub collection: Option<String>,
    pub tenant: TenantId,
    pub conversation_id: Option<Uuid>,
    pub language: Option<String>,
    pub history_limit: Option<i64>,
}

pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<Citation>,  // passthrough from ContextResult
    pub usage: TokenUsage,
    pub model: String,
}
```

### 4.3 Collection Resolution

- If `collection` is provided and this is a new conversation: use it.
- If `collection` is provided and resuming a conversation: must match the conversation's stored collection. Mismatch → error. No silent switching.
- If `collection` is `None` and resuming: use the conversation's stored collection.
- If `collection` is `None` and new conversation: error — first turn must specify a collection.

### 4.4 Title

New conversations get `title: None`. Title management deferred.

### 4.5 Orchestration Flow — `chat(request: ChatRequest) -> Result<ChatResponse>`

1. Validate `history_limit > 0` if provided.
2. **Resolve conversation** — if `conversation_id` provided, load and verify tenant, resolve collection (with mismatch check). Otherwise create new conversation with the provided collection.
3. **Load history** — `get_messages(tenant, conversation_id, limit)` → convert `MessageRow` to `ChatMessage`.
4. **Persist user message** — `insert_message(tenant, conversation_id, User, query, None)`.
5. **Retrieve context** — `retrieval.search_hybrid(collection, query, tenant, None)`.
6. **Assemble context** — `context_builder.build(fused_chunks, &context_config)`.
7. **Render prompt** — `prompt_renderer.render_system_prompt(...)`.
8. **Call LLM** — `backend.complete(&CompletionRequest { system, messages: &[...history, user_query], temperature, max_tokens, stop: vec![] })`.
9. **Persist assistant message** — `insert_message(tenant, conversation_id, Assistant, answer, None)`.
10. **Return** — `ChatResponse` with answer, conversation_id, citations from step 6, usage from step 8.

**Persistence ordering:** User message persisted before the LLM call (step 4) so it survives backend failures. If the LLM call fails, the conversation still reflects the user's question.

**Accepted MVP trade-off:** If the LLM call succeeds but assistant-message persistence fails (step 9), `chat()` returns an error. Tokens are spent and the answer is lost. A more robust approach (returning the answer alongside the persistence error) can be added later.

Errors at any step propagate via `?`. No streaming, no guardrails, no retry-with-budget-bump.

---

## 5. Configuration

### 5.1 New `[llm]` TOML Section

```toml
[llm]
provider = "openai-compatible"  # or "anthropic"
model = "gpt-4o"
# base_url = "https://api.openai.com/v1"  # optional, defaults per provider
temperature = 0.1
max_tokens = 4096
timeout_secs = 60
max_retries = 3
retry_backoff_ms = 500
prompt_template_path = "config/prompts/chat_system.hbs"
```

### 5.2 Environment Variables

`LLM_API_KEY` — from environment only, never in TOML (it is a secret).

### 5.3 Provider Defaults for base_url

- `openai-compatible`: `https://api.openai.com/v1`
- `anthropic`: `https://api.anthropic.com`

### 5.4 Validation

- `model` must be non-empty.
- `base_url` must be non-empty if explicitly provided.
- `max_tokens > 0`.
- `temperature >= 0.0 && temperature.is_finite()` (reject NaN and infinity).
- `timeout_secs > 0`.

Precedence: Environment variables > `app.toml` > hardcoded defaults.

---

## 6. New Files

| File | Responsibility |
|------|---------------|
| `crates/rag-core/src/llm.rs` | `ChatBackend` enum, `CompletionRequest`, `LlmResponse`, message validation/coalescing, retry logic |
| `crates/rag-core/src/prompt.rs` | `PromptRenderer`, `PromptContext`, `render_context_chunks` |
| `crates/rag-core/src/chat.rs` | `ChatService`, `ChatRequest`, `ChatResponse`, `ChatDefaults`, orchestration flow |
| `crates/rag-core/src/stores/conversations.rs` | `MessageRole`, `ConversationRow`, `MessageRow`, conversation/message CRUD |
| `config/prompts/chat_system.hbs` | Handlebars system prompt template |

**Modified files:**

| File | Change |
|------|--------|
| `crates/rag-core/src/config.rs` | Add `LlmSection` struct, new `AppConfig` fields, validation |
| `config/app.toml` | Add `[llm]` section |
| `crates/rag-core/src/stores/mod.rs` | Register `conversations` module |
| `crates/rag-core/src/lib.rs` | Add `pub mod llm`, `pub mod prompt`, `pub mod chat`, re-exports |

---

## 7. Testing Strategy

### 7.1 Unit Tests (no IO)

**`llm.rs`:**
- Coalescing: consecutive same-role messages joined with newline
- Coalescing: alternating roles left unchanged
- Empty messages valid (system-only prompt)
- Validate temperature: finite and >= 0
- Validate max_tokens > 0
- Validate empty model rejected

**`prompt.rs`:**
- Render with context + language instruction — placeholders replaced
- Render with empty context — fallback text present
- Render with empty language instruction — placeholder removed
- `render_context_chunks` — numbered format correct for 3 chunks
- `render_context_chunks` — empty input → empty string
- Invalid template path → constructor error

### 7.2 Integration Tests — Conversation Store (`integration_conversations.rs`)

Postgres only, no Qdrant or LLM. `#[ignore]` (requires `just up`).

- Create conversation, verify row with correct tenant/collection
- Insert messages, verify `MessageRole` enum round-trip to/from DB text column
- `get_messages`: correct ordering (latest N, oldest-first)
- `get_messages`: limit respected
- Tenant isolation: tenant B cannot read or insert into tenant A's conversation
- Insert into nonexistent conversation → explicit error

### 7.3 Integration Tests — Chat End-to-End (`integration_chat.rs`)

Real Postgres + Qdrant, mock LLM backend. `#[ignore]` (requires `just up`).

- Ingest fixtures → chat query → response has answer + citations + conversation_id
- Multi-turn: second chat with same conversation_id loads history, both turns persisted
- Empty retrieval results → LLM still called, valid response returned
- LLM failure → user message persisted, assistant message not persisted
- Collection mismatch on resume → error

### 7.4 Provider Smoke Tests (`smoke_llm.rs`)

Real LLM provider, no Qdrant/Postgres. `#[ignore]`, provider-gated: each test checks its own env var and skips gracefully if not configured.

- OpenAI-compatible: simple completion → valid response with usage
- Anthropic: simple completion → valid response with usage
- Invalid API key → clear error

---

## 8. Out of Scope (Deferred)

- Streaming responses
- Guardrails (injection detection, PII redaction)
- User/session/run management (Phase 10)
- DB-backed prompt catalog (Phase 11)
- Suggested questions / follow-up actions
- Reranking or cross-encoder scoring
- Cost tracking / token accounting
- Provider-specific prompt template overrides
- Conversation title derivation
- LLM response retry with budget bump
