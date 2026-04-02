# Routed Agentic Search Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement milestone 1 of routed agentic search in Apex by reusing the existing `/agents/:id/execute` API, extracting a reusable single-shot baseline answer path, adding explicit retrieval tools and richer evidence payloads, and shipping an offline eval harness without changing `/chat`.

**Architecture:** The work splits into six slices. First, extract a conversation-free baseline answer path from `ChatService`. Second, enrich retrieval and storage so evidence is useful to an agent. Third, add agent-core routing types and explicit tool ports. Fourth, wire a new runtime branch and `agentic_search_v1` spec. Fifth, expose the routed path through the existing agent API with enriched run output. Sixth, add a reproducible offline eval harness and a dedicated `just` entry point.

**Tech Stack:** Rust 2024, Axum, utoipa, sqlx/Postgres, Qdrant, graph-flow, tokio, serde/serde_json, reqwest, test-support, just

**Spec:** `docs/superpowers/specs/2026-04-02-routed-agentic-search-foundation-design.md`

---

## File Structure

### New files

| File | Responsibility |
|------|---------------|
| `crates/agent-core/src/route.rs` | Deterministic routing heuristics and `RouteDecision` construction |
| `crates/rag-core/migrations/0004_agentic_search_foundation.sql` | Postgres FTS index for chunk text |
| `crates/rag-core/tests/integration_chat_single_shot.rs` | Mock-LLM integration test for the extracted baseline answer path |
| `config/agents/agentic_search_v1.yaml` | New synchronous routed-search agent spec |
| `crates/rag-server/tests/agentic_eval.rs` | Offline benchmark harness that compares `/chat` with `agentic_search_v1` |
| `data/evals/agentic_search_v1/benchmark.json` | Benchmark cases with expected routes and evidence hints |
| `data/evals/agentic_search_v1/corpus/rust-guide.md` | Eval corpus document for simple fact queries |
| `data/evals/agentic_search_v1/corpus/python-tradeoffs.md` | Eval corpus document for comparison and research queries |
| `data/evals/agentic_search_v1/corpus/auth-runbook.md` | Eval corpus document for procedural queries |
| `data/evals/agentic_search_v1/corpus/deployment-notes.md` | Eval corpus document for exploratory search queries |
| `data/evals/agentic_search_v1/corpus/disambiguation-notes.md` | Eval corpus document for ambiguity/disambiguation queries |

### Modified files

| File | Change |
|------|--------|
| `crates/rag-core/src/chat.rs` | Extract `answer_single_shot`, shared grounded-answer helper, and reusable answer artifact |
| `crates/rag-core/src/fusion.rs` | Enrich retrieved/fused chunk types with metadata and score provenance |
| `crates/rag-core/src/retrieval.rs` | Add `search_fts`, `expand_chunk_neighbors`, and enriched payload mapping |
| `crates/rag-core/src/ingest.rs` | Copy hot metadata and section heading into Qdrant payloads |
| `crates/rag-core/src/stores/chunks.rs` | Add FTS query and chunk-neighbor lookup helpers |
| `crates/agent-core/src/lib.rs` | Export the new route module and updated domain types |
| `crates/agent-core/src/ports.rs` | Expand retrieval tools and add `BaselineAnswerPort` |
| `crates/agent-core/src/types.rs` | Add `RouteDecision`, richer `ScoredChunk`, and baseline answer domain types |
| `crates/agent-core/src/spec.rs` | Update built-in tool registry names to match milestone-1 contracts |
| `crates/agent-core/src/runtime/graph_flow/keys.rs` | Add route and grounded-answer context keys |
| `crates/agent-core/src/runtime/graph_flow/tasks.rs` | Add `route_query`, `baseline_answer`, `retrieve_evidence`, and `compose_answer` tasks |
| `crates/agent-core/src/runtime/graph_flow/mod.rs` | Wire new ports/tasks, branch extraction, and route decision in run results |
| `crates/rag-server/src/agents.rs` | Implement expanded retrieval adapter and baseline-answer adapter |
| `crates/rag-server/src/routes/agents.rs` | Return `route_decision` and richer search results |
| `crates/rag-server/src/openapi.rs` | Register new agent response schemas |
| `crates/rag-core/tests/integration_retrieval.rs` | Cover FTS, neighbor expansion, and enriched evidence metadata |
| `crates/rag-server/tests/agents.rs` | Cover `agentic_search_v1` baseline and agentic branches |
| `Justfile` | Add a dedicated eval command |
| `docs/howto/testing.md` | Document how to run the new benchmark harness |

---

## Task 1: Extract the Reusable Single-Shot Baseline Answer Path

**Files:**
- Create: `crates/rag-core/tests/integration_chat_single_shot.rs`
- Modify: `crates/rag-core/src/chat.rs`

- [ ] **Step 1: Write the failing integration test for the new baseline path**

Create `crates/rag-core/tests/integration_chat_single_shot.rs` with this content:

```rust
#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::chat::ChatService;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::stores::Stores;
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

async fn setup(ingest_root: &Path) -> Result<(IngestService, ChatService)> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    config.ingest_allowed_roots = vec![ingest_root.to_path_buf()];
    let stores = Stores::new(&config).await?;
    let ingest = IngestService::new(stores.clone(), &config)?;
    let chat = ChatService::with_mock_llm(stores, &config, "Mock LLM response.".into())?;
    Ok((ingest, chat))
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
         Tokio provides async task scheduling. \
         Ownership and borrowing prevent data races.",
    );
    write_sidecar(dir.path(), "rust");

    ingest
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant: tenant.clone(),
            collection_override: Some(collection.to_owned()),
            dry_run: false,
        })
        .await?;

    Ok(())
}

#[tokio::test]
#[ignore] // requires `just up`
async fn answer_single_shot_returns_grounded_answer() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, chat) = setup(dir.path()).await?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-single-shot-{suffix}").parse()?;
    let collection = format!("test_single_shot_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    let response = chat
        .answer_single_shot("What is Rust?", &collection, tenant.as_str(), None)
        .await?;

    assert_eq!(response.answer, "Mock LLM response.");
    assert!(!response.citations.is_empty(), "citations should be present");
    assert!(!response.evidence.is_empty(), "evidence should be present");
    assert!(!response.model.is_empty(), "model should be populated");

    Ok(())
}
```

- [ ] **Step 2: Run the new test and confirm the current code fails**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_chat_single_shot -- --ignored --nocapture
```

Expected: compile failure because `ChatService::answer_single_shot` and the returned answer artifact do not exist yet.

- [ ] **Step 3: Extract the grounded single-shot helper in `chat.rs`**

In `crates/rag-core/src/chat.rs`, add a reusable answer artifact and helper:

```rust
#[derive(Debug, Clone)]
pub struct GroundedAnswer {
    pub answer: String,
    pub citations: Vec<Citation>,
    pub evidence: Vec<FusedChunk>,
    pub usage: TokenUsage,
    pub model: String,
}

impl ChatService {
    pub async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> Result<GroundedAnswer> {
        self.answer_with_retrieval(query, collection, tenant, &[], language).await
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
        let context_text = render_context_chunks(&context_result.chunks);
        let language_instruction = language
            .map(|lang| format!("Respond in {lang}."))
            .unwrap_or_default();
        let system_prompt = self.prompt_renderer.render_system_prompt(&PromptContext {
            context: &context_text,
            language_instruction: &language_instruction,
        })?;

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
            evidence: fused,
            usage: llm_response.usage,
            model: llm_response.model,
        })
    }
}
```

Then refactor `ChatService::chat` to replace the current retrieval/context/LLM block with:

```rust
let grounded = self
    .answer_with_retrieval(
        &request.query,
        &collection,
        tenant,
        &messages,
        request.language.as_deref(),
    )
    .await?;

self.stores
    .insert_chat_turn(tenant, conversation_id, &request.query, &grounded.answer)
    .await
    .context("persisting chat turn")?;

Ok(ChatResponse {
    answer: grounded.answer,
    conversation_id,
    citations: grounded.citations,
    usage: grounded.usage,
    model: grounded.model,
})
```

- [ ] **Step 4: Run the new integration test and existing unit suite**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_chat_single_shot -- --ignored --nocapture
CARGO_TARGET_DIR=target/test cargo test -p rag-core --lib
```

Expected: the new ignored integration test passes, and existing `rag-core` unit tests still pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/rag-core/src/chat.rs crates/rag-core/tests/integration_chat_single_shot.rs
git commit -m "feat(rag-core): extract single-shot grounded answer path"
```

---

## Task 2: Add FTS, Neighbor Expansion, and Enriched Retrieval Metadata

**Files:**
- Create: `crates/rag-core/migrations/0004_agentic_search_foundation.sql`
- Modify: `crates/rag-core/src/stores/chunks.rs`
- Modify: `crates/rag-core/src/fusion.rs`
- Modify: `crates/rag-core/src/retrieval.rs`
- Modify: `crates/rag-core/src/ingest.rs`
- Modify: `crates/rag-core/tests/integration_retrieval.rs`

- [ ] **Step 1: Add failing retrieval integration tests**

In `crates/rag-core/tests/integration_retrieval.rs`, append these tests:

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn fts_search_returns_lexical_hits() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, retrieval, _config) = setup(dir.path()).await?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let results = retrieval.search_fts(&collection, "\"starter culture\"", tenant.as_str(), 10).await?;

    assert!(!results.is_empty(), "fts search should return results");
    assert_eq!(results[0].document_id, "cooking");
    assert_eq!(results[0].source_domain.as_deref(), Some("example.com"));
    assert_eq!(results[0].title.as_deref(), Some("Test cooking"));

    Ok(())
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn expand_chunk_neighbors_returns_adjacent_chunks() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, retrieval, _config) = setup(dir.path()).await?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval.search_hybrid(&collection, "borrow checker", tenant.as_str(), None).await?;
    let anchor = fused.first().expect("anchor chunk");

    let neighbors = retrieval
        .expand_chunk_neighbors(tenant.as_str(), &anchor.document_id, anchor.chunk_index, 1, 1)
        .await?;

    assert!(!neighbors.is_empty(), "neighbor expansion should return results");
    assert!(neighbors.iter().any(|chunk| chunk.chunk_index == anchor.chunk_index));

    Ok(())
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn hybrid_search_carries_metadata_and_provenance() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, retrieval, _config) = setup(dir.path()).await?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval.search_hybrid(&collection, "rust programming", tenant.as_str(), None).await?;

    let first = fused.first().expect("fused result");
    assert_eq!(first.source_domain.as_deref(), Some("example.com"));
    assert!(!first.sources.is_empty(), "rrf provenance should be preserved");
    assert!(first.source_scores.keys().all(|source| first.sources.contains(source)));
    assert!(!first.tags.is_empty(), "tags should be preserved");

    Ok(())
}
```

- [ ] **Step 2: Run the retrieval tests and confirm they fail**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_retrieval -- --ignored --nocapture
```

Expected: compile failure because `search_fts`, `expand_chunk_neighbors`, and the enriched metadata fields do not exist yet.

- [ ] **Step 3: Add the FTS migration and chunk-store helpers**

Create `crates/rag-core/migrations/0004_agentic_search_foundation.sql`:

```sql
CREATE INDEX IF NOT EXISTS idx_chunks_text_fts
    ON chunks
    USING GIN (to_tsvector('simple', text));
```

In `crates/rag-core/src/stores/chunks.rs`, add these methods:

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChunkSearchRow {
    pub tenant: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: String,
    pub language: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub collection: Option<String>,
    pub score: f32,
}

pub async fn search_chunks_fts(
    &self,
    tenant: &str,
    collection: &str,
    query: &str,
    limit: u64,
) -> Result<Vec<ChunkSearchRow>> {
    let rows = sqlx::query_as::<_, ChunkSearchRow>(
        r#"
        SELECT
            c.tenant,
            c.document_id,
            c.chunk_index,
            c.text,
            d.title,
            d.language,
            d.metadata,
            d.collection,
            ts_rank_cd(to_tsvector('simple', c.text), websearch_to_tsquery('simple', $3))::float4 AS score
        FROM chunks c
        JOIN documents d
          ON d.tenant = c.tenant
         AND d.id = c.document_id
        WHERE c.tenant = $1
          AND d.collection = $2
          AND to_tsvector('simple', c.text) @@ websearch_to_tsquery('simple', $3)
        ORDER BY score DESC, c.document_id ASC, c.chunk_index ASC
        LIMIT $4
        "#,
    )
    .bind(tenant)
    .bind(collection)
    .bind(query)
    .bind(limit as i64)
    .fetch_all(&self.pool)
    .await
    .context("fts searching chunks")?;

    Ok(rows)
}

pub async fn get_chunk_neighbors(
    &self,
    tenant: &str,
    document_id: &str,
    center_chunk_index: i32,
    before: i32,
    after: i32,
) -> Result<Vec<ChunkRecord>> {
    let start = center_chunk_index.saturating_sub(before);
    let end = center_chunk_index.saturating_add(after);

    let rows = sqlx::query_as::<_, ChunkRecord>(
        r#"
        SELECT id, tenant, document_id, chunk_index, text
        FROM chunks
        WHERE tenant = $1
          AND document_id = $2
          AND chunk_index BETWEEN $3 AND $4
        ORDER BY chunk_index
        "#,
    )
    .bind(tenant)
    .bind(document_id)
    .bind(start)
    .bind(end)
    .fetch_all(&self.pool)
    .await
    .context("fetching chunk neighbors")?;

    Ok(rows)
}
```

- [ ] **Step 4: Enrich retrieval types, Qdrant payload mapping, and retrieval service APIs**

In `crates/rag-core/src/fusion.rs`, replace the current result shapes with:

```rust
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub score: f32,
    pub score_type: String,
}

#[derive(Debug, Clone)]
pub struct FusedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub fused_score: f32,
    pub score_type: String,
    pub sources: Vec<String>,
    pub source_scores: std::collections::HashMap<String, f32>,
}
```

In `crates/rag-core/src/ingest.rs`, enrich the Qdrant payload in `build_qdrant_points` with:

```rust
("title".to_string(), resolve_title(doc.sidecar.as_ref(), doc.native_metadata.as_ref()).to_string().into()),
("source_url".to_string(), doc.sidecar.as_ref().map(|s| s.source.url.clone()).unwrap_or_default().into()),
("source_domain".to_string(), doc.sidecar.as_ref().map(|s| s.source.domain.clone()).unwrap_or_default().into()),
("language".to_string(), doc.sidecar.as_ref().map(|s| s.language.clone()).unwrap_or_default().into()),
("collection".to_string(), doc.collection.clone().into()),
("section_heading".to_string(), chunk.section.section_title.clone().unwrap_or_default().into()),
("tags".to_string(), serde_json::to_string(&doc.sidecar.as_ref().map(|s| s.tags.clone()).unwrap_or_default())?.into()),
```

In `crates/rag-core/src/retrieval.rs`, add:

```rust
fn stable_chunk_id(tenant: &str, document_id: &str, chunk_index: i32) -> String {
    let input = format!("{tenant}:{document_id}:{chunk_index}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, input.as_bytes()).to_string()
}

pub async fn search_fts(
    &self,
    collection: &str,
    query: &str,
    tenant: &str,
    limit: u64,
) -> Result<Vec<RetrievedChunk>> {
    let rows = self
        .stores
        .search_chunks_fts(tenant, collection, query, limit)
        .await
        .context("fts search")?;

    Ok(rows
        .into_iter()
        .map(|row| RetrievedChunk {
            chunk_id: stable_chunk_id(tenant, &row.document_id, row.chunk_index),
            document_id: row.document_id,
            chunk_index: row.chunk_index,
            text: row.text,
            title: Some(row.title),
            source_url: row.metadata.as_ref().and_then(|m| m.pointer("/source/url")).and_then(|v| v.as_str()).map(ToOwned::to_owned),
            source_domain: row.metadata.as_ref().and_then(|m| m.pointer("/source/domain")).and_then(|v| v.as_str()).map(ToOwned::to_owned),
            language: row.language,
            tags: row.metadata.as_ref().and_then(|m| m.get("tags")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
            section_heading: None,
            collection: row.collection,
            score: row.score,
            score_type: "fts".to_string(),
        })
        .collect())
}

pub async fn expand_chunk_neighbors(
    &self,
    tenant: &str,
    document_id: &str,
    chunk_index: i32,
    before: i32,
    after: i32,
) -> Result<Vec<RetrievedChunk>> {
    let rows = self
        .stores
        .get_chunk_neighbors(tenant, document_id, chunk_index, before, after)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| RetrievedChunk {
            chunk_id: stable_chunk_id(tenant, &row.document_id, row.chunk_index),
            document_id: row.document_id,
            chunk_index: row.chunk_index,
            text: row.text,
            title: None,
            source_url: None,
            source_domain: None,
            language: None,
            tags: Vec::new(),
            section_heading: None,
            collection: None,
            score: 0.0,
            score_type: "neighbor".to_string(),
        })
        .collect())
}
```

Also update `scored_points_to_chunks` to read the new Qdrant payload keys, and set:

```rust
score_type: "dense".to_string()
```

for dense results and:

```rust
score_type: "sparse".to_string()
```

for sparse results. In `rrf_fusion`, set:

```rust
score_type: "rrf_fused".to_string()
```

and carry `sources` plus `source_scores` forward.

- [ ] **Step 5: Run retrieval integration tests**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_retrieval -- --ignored --nocapture
```

Expected: the new FTS, neighbor, and metadata/provenance tests pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/rag-core/migrations/0004_agentic_search_foundation.sql crates/rag-core/src/stores/chunks.rs crates/rag-core/src/fusion.rs crates/rag-core/src/retrieval.rs crates/rag-core/src/ingest.rs crates/rag-core/tests/integration_retrieval.rs
git commit -m "feat(rag-core): add agentic retrieval tools and enriched evidence"
```

---

## Task 3: Add Agent-Core Routing Types, Explicit Tool Ports, and Richer Results

**Files:**
- Create: `crates/agent-core/src/route.rs`
- Modify: `crates/agent-core/src/lib.rs`
- Modify: `crates/agent-core/src/ports.rs`
- Modify: `crates/agent-core/src/types.rs`
- Modify: `crates/agent-core/src/spec.rs`

- [ ] **Step 1: Write failing routing and tool-registry tests**

Create `crates/agent-core/src/route.rs` with tests first:

```rust
use crate::types::{QueryClass, RetrievalProfileId, RoutePath};

pub fn route_query(_query: &str) -> crate::types::RouteDecision {
    unimplemented!("write tests before implementation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_fact_routes_to_single_pass_rag() {
        let decision = route_query("What is Rust?");
        assert_eq!(decision.selected_path, RoutePath::SinglePassRag);
        assert_eq!(decision.query_class, QueryClass::SimpleFact);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::SimpleHybrid);
    }

    #[test]
    fn multi_hop_query_routes_to_agentic_search() {
        let decision = route_query("Compare Rust and Python tradeoffs for async services");
        assert_eq!(decision.selected_path, RoutePath::AgenticSearch);
        assert_eq!(decision.query_class, QueryClass::MultiHopResearch);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::BroadThenExpand);
        assert!(decision.needs_multi_hop);
    }

    #[test]
    fn procedural_query_prefers_lexical_first() {
        let decision = route_query("How do I rotate API keys in the auth runbook?");
        assert_eq!(decision.selected_path, RoutePath::AgenticSearch);
        assert_eq!(decision.query_class, QueryClass::Procedural);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::LexicalFirst);
    }
}
```

In `crates/agent-core/src/spec.rs`, add this test near the existing `required_tools` cases:

```rust
#[test]
fn default_tool_registry_includes_agentic_search_tools() {
    let tools = DefaultToolRegistry.known_tools();

    assert!(tools.contains("retrieval.dense"));
    assert!(tools.contains("retrieval.sparse"));
    assert!(tools.contains("retrieval.hybrid"));
    assert!(tools.contains("retrieval.fts"));
    assert!(tools.contains("retrieval.expand_chunk_neighbors"));
    assert!(tools.contains("retrieval.fetch_document"));
}
```

- [ ] **Step 2: Run agent-core tests and confirm they fail**

Run:

```bash
CARGO_TARGET_DIR=target/test cargo test -p agent-core --lib -- --nocapture
```

Expected: compile failure because the new route and type definitions do not exist yet.

- [ ] **Step 3: Add route/result types and expand the ports**

In `crates/agent-core/src/types.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePath {
    SinglePassRag,
    AgenticSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryClass {
    SimpleFact,
    AmbiguityDisambiguation,
    ExploratorySearch,
    Procedural,
    MultiHopResearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalProfileId {
    SimpleHybrid,
    LexicalFirst,
    BroadThenExpand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub selected_path: RoutePath,
    pub query_class: QueryClass,
    pub retrieval_profile: RetrievalProfileId,
    pub ambiguity: bool,
    pub needs_multi_hop: bool,
    pub needs_high_evidence: bool,
    pub time_sensitive: bool,
    pub normalized_filters: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub score: f32,
    pub score_type: String,
    pub sources: Vec<String>,
    pub source_scores: std::collections::HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundedAnswer {
    pub answer: String,
    pub search_results: Vec<ScoredChunk>,
    pub citations: Vec<String>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub run_id: RunId,
    pub state: AgentState,
    pub answer: Option<String>,
    pub summary: Option<String>,
    pub search_results: Option<Vec<ScoredChunk>>,
    pub query_type: Option<QueryType>,
    pub route_decision: Option<RouteDecision>,
    pub pending_checkpoint: Option<PendingCheckpoint>,
    pub steps: Vec<StepRecord>,
}
```

In `crates/agent-core/src/ports.rs`, replace the current retrieval-only trait with:

```rust
#[async_trait::async_trait]
pub trait RetrievalPort: Send + Sync {
    async fn search_dense(&self, collection: &str, query: &str, tenant: &str, limit: u64)
        -> anyhow::Result<Vec<ScoredChunk>>;
    async fn search_sparse(&self, collection: &str, query: &str, tenant: &str, limit: u64)
        -> anyhow::Result<Vec<ScoredChunk>>;
    async fn search_hybrid(&self, collection: &str, query: &str, tenant: &str)
        -> anyhow::Result<Vec<ScoredChunk>>;
    async fn search_fts(&self, collection: &str, query: &str, tenant: &str, limit: u64)
        -> anyhow::Result<Vec<ScoredChunk>>;
    async fn expand_chunk_neighbors(
        &self,
        tenant: &str,
        document_id: &str,
        chunk_index: i32,
        before: i32,
        after: i32,
    ) -> anyhow::Result<Vec<ScoredChunk>>;
    async fn fetch_document(
        &self,
        tenant: &str,
        document_id: &str,
    ) -> anyhow::Result<serde_json::Value>;
}

#[async_trait::async_trait]
pub trait BaselineAnswerPort: Send + Sync {
    async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> anyhow::Result<GroundedAnswer>;
}
```

In `crates/agent-core/src/route.rs`, implement the deterministic router:

```rust
use crate::types::{QueryClass, RetrievalProfileId, RouteDecision, RoutePath};

pub fn route_query(query: &str) -> RouteDecision {
    let lower = query.to_ascii_lowercase();

    let has_compare = lower.contains("compare ") || lower.contains("tradeoff") || lower.contains("vs ");
    let has_how_to = lower.contains("how do i") || lower.contains("runbook") || lower.contains("steps");
    let has_ambiguity = lower.contains("which ") || lower.contains("difference between");
    let has_time = lower.contains("today") || lower.contains("latest") || lower.contains("current");

    let (selected_path, query_class, retrieval_profile, needs_multi_hop) = if has_compare {
        (RoutePath::AgenticSearch, QueryClass::MultiHopResearch, RetrievalProfileId::BroadThenExpand, true)
    } else if has_how_to {
        (RoutePath::AgenticSearch, QueryClass::Procedural, RetrievalProfileId::LexicalFirst, false)
    } else if has_ambiguity {
        (RoutePath::AgenticSearch, QueryClass::AmbiguityDisambiguation, RetrievalProfileId::LexicalFirst, false)
    } else {
        (RoutePath::SinglePassRag, QueryClass::SimpleFact, RetrievalProfileId::SimpleHybrid, false)
    };

    RouteDecision {
        selected_path,
        query_class,
        retrieval_profile,
        ambiguity: has_ambiguity,
        needs_multi_hop,
        needs_high_evidence: has_compare || has_how_to,
        time_sensitive: has_time,
        normalized_filters: Vec::new(),
        reasons: vec![
            if has_compare { "compare".to_string() } else { "default".to_string() },
            if has_how_to { "procedural".to_string() } else { "non_procedural".to_string() },
        ],
    }
}
```

Update `crates/agent-core/src/lib.rs` exports:

```rust
pub mod route;
pub use ports::{ApprovalPort, BaselineAnswerPort, ChatPort, RetrievalPort};
pub use types::{AgentRunResult, AgentState, QueryType, RouteDecision, RoutePath};
```

And update `DefaultToolRegistry::BUILT_IN` in `crates/agent-core/src/spec.rs` to:

```rust
const BUILT_IN: &[&str] = &[
    "retrieval.dense",
    "retrieval.sparse",
    "retrieval.hybrid",
    "retrieval.fts",
    "retrieval.expand_chunk_neighbors",
    "retrieval.fetch_document",
    "sql_allowlist",
];
```

- [ ] **Step 4: Run agent-core tests**

Run:

```bash
CARGO_TARGET_DIR=target/test cargo test -p agent-core --lib
```

Expected: the new routing tests and the updated tool-registry tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/agent-core/src/route.rs crates/agent-core/src/lib.rs crates/agent-core/src/ports.rs crates/agent-core/src/types.rs crates/agent-core/src/spec.rs
git commit -m "feat(agent-core): add routed-search types and explicit tool ports"
```

---

## Task 4: Wire the Runtime Branch and Add `agentic_search_v1`

**Files:**
- Modify: `crates/agent-core/src/runtime/graph_flow/keys.rs`
- Modify: `crates/agent-core/src/runtime/graph_flow/tasks.rs`
- Modify: `crates/agent-core/src/runtime/graph_flow/mod.rs`
- Create: `config/agents/agentic_search_v1.yaml`

- [ ] **Step 1: Add failing runtime tests for the new branch**

At the bottom of `crates/agent-core/src/runtime/graph_flow/mod.rs`, add a `#[cfg(test)]` module with stub ports and these two tests:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ports::{BaselineAnswerPort, ChatPort, RetrievalPort};
    use crate::types::{GroundedAnswer, RoutePath, ScoredChunk};

    struct StubRetrieval;
    struct StubChat;
    struct StubApproval;
    struct StubBaseline;

    #[async_trait::async_trait]
    impl RetrievalPort for StubRetrieval {
        async fn search_dense(&self, _: &str, _: &str, _: &str, _: u64) -> anyhow::Result<Vec<ScoredChunk>> { Ok(Vec::new()) }
        async fn search_sparse(&self, _: &str, _: &str, _: &str, _: u64) -> anyhow::Result<Vec<ScoredChunk>> { Ok(Vec::new()) }
        async fn search_hybrid(&self, _: &str, _: &str, _: &str) -> anyhow::Result<Vec<ScoredChunk>> { Ok(Vec::new()) }
        async fn search_fts(&self, _: &str, _: &str, _: &str, _: u64) -> anyhow::Result<Vec<ScoredChunk>> { Ok(Vec::new()) }
        async fn expand_chunk_neighbors(&self, _: &str, _: &str, _: i32, _: i32, _: i32) -> anyhow::Result<Vec<ScoredChunk>> { Ok(Vec::new()) }
        async fn fetch_document(&self, _: &str, _: &str) -> anyhow::Result<serde_json::Value> { Ok(serde_json::json!({})) }
    }

    #[async_trait::async_trait]
    impl ChatPort for StubChat {
        async fn summarize(&self, _: &str, _: &[ScoredChunk], _: &str) -> anyhow::Result<String> {
            Ok("agentic summary".to_string())
        }
    }

    #[async_trait::async_trait]
    impl ApprovalPort for StubApproval {
        async fn request_approval(&self, _: &PendingCheckpoint) -> anyhow::Result<Option<CheckpointDecision>> {
            Ok(Some(CheckpointDecision { approved: true, reason: None }))
        }
    }

    #[async_trait::async_trait]
    impl BaselineAnswerPort for StubBaseline {
        async fn answer_single_shot(&self, _: &str, _: &str, _: &str, _: Option<&str>) -> anyhow::Result<GroundedAnswer> {
            Ok(GroundedAnswer {
                answer: "baseline answer".to_string(),
                search_results: Vec::new(),
                citations: Vec::new(),
                model: "mock".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn simple_query_uses_baseline_branch() {
        let spec = AgentSpec::from_yaml_str(include_str!("../../../../../config/agents/agentic_search_v1.yaml")).unwrap();
        let runtime = GraphFlowRuntime::from_spec(
            spec,
            Arc::new(StubRetrieval),
            Arc::new(StubChat),
            Arc::new(StubApproval),
            Arc::new(StubBaseline),
        );

        let result = runtime
            .start(AgentRunConfig {
                query: "What is Rust?".into(),
                collection: "docs".into(),
                tenant: "default".into(),
                max_steps: 10,
            })
            .await
            .unwrap();

        assert_eq!(result.route_decision.unwrap().selected_path, RoutePath::SinglePassRag);
        assert_eq!(result.answer.as_deref(), Some("baseline answer"));
    }

    #[tokio::test]
    async fn comparison_query_uses_agentic_branch() {
        let spec = AgentSpec::from_yaml_str(include_str!("../../../../../config/agents/agentic_search_v1.yaml")).unwrap();
        let runtime = GraphFlowRuntime::from_spec(
            spec,
            Arc::new(StubRetrieval),
            Arc::new(StubChat),
            Arc::new(StubApproval),
            Arc::new(StubBaseline),
        );

        let result = runtime
            .start(AgentRunConfig {
                query: "Compare Rust and Python tradeoffs".into(),
                collection: "docs".into(),
                tenant: "default".into(),
                max_steps: 10,
            })
            .await
            .unwrap();

        assert_eq!(result.route_decision.unwrap().selected_path, RoutePath::AgenticSearch);
        assert_eq!(result.answer.as_deref(), Some("agentic summary"));
    }
}
```

- [ ] **Step 2: Run agent-core runtime tests and confirm they fail**

Run:

```bash
CARGO_TARGET_DIR=target/test cargo test -p agent-core --lib -- --nocapture
```

Expected: compile failure because `GraphFlowRuntime` does not yet accept the baseline port or expose route decisions.

- [ ] **Step 3: Add keys, tasks, and runtime plumbing**

In `crates/agent-core/src/runtime/graph_flow/keys.rs`, add:

```rust
pub const ROUTE_DECISION: &str = "route_decision";
pub const ROUTE_TO_AGENTIC_SEARCH: &str = "route_to_agentic_search";
pub const RETRIEVAL_PROFILE: &str = "retrieval_profile";
```

In `crates/agent-core/src/runtime/graph_flow/tasks.rs`, add task IDs and implementations:

```rust
pub const ROUTE_QUERY_TASK: &str = "route_query";
pub const BASELINE_ANSWER_TASK: &str = "baseline_answer";
pub const RETRIEVE_EVIDENCE_TASK: &str = "retrieve_evidence";
pub const COMPOSE_ANSWER_TASK: &str = "compose_answer";
```

The new tasks should follow this structure:

```rust
pub struct RouteQueryTask;

#[async_trait::async_trait]
impl Task for RouteQueryTask {
    fn id(&self) -> &str { ROUTE_QUERY_TASK }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let query: String = context.get(keys::QUERY).await.ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed("missing query".into())
        })?;
        let decision = crate::route::route_query(&query);
        let route_to_agentic = decision.selected_path == crate::types::RoutePath::AgenticSearch;
        context.set(keys::ROUTE_DECISION, &decision).await;
        context.set(keys::ROUTE_TO_AGENTIC_SEARCH, &route_to_agentic).await;
        context.set(keys::RETRIEVAL_PROFILE, &decision.retrieval_profile).await;
        Ok(TaskResult::new(Some(format!("{:?}", decision.selected_path)), NextAction::Continue))
    }
}

pub struct BaselineAnswerTask {
    pub baseline: Arc<dyn crate::ports::BaselineAnswerPort>,
}

pub struct RetrieveEvidenceTask {
    pub retrieval: Arc<dyn RetrievalPort>,
}

pub struct ComposeAnswerTask {
    pub chat: Arc<dyn ChatPort>,
}
```

Inside `RetrieveEvidenceTask`, interpret `RetrievalProfileId` exactly like this:

```rust
match profile {
    crate::types::RetrievalProfileId::SimpleHybrid => {
        self.retrieval.search_hybrid(&collection, &query, &tenant).await?
    }
    crate::types::RetrievalProfileId::LexicalFirst => {
        let mut results = self.retrieval.search_fts(&collection, &query, &tenant, 10).await?;
        let mut hybrid = self.retrieval.search_hybrid(&collection, &query, &tenant).await?;
        results.append(&mut hybrid);
        results
    }
    crate::types::RetrievalProfileId::BroadThenExpand => {
        let mut results = self.retrieval.search_hybrid(&collection, &query, &tenant).await?;
        if let Some(anchor) = results.first() {
            let mut neighbors = self
                .retrieval
                .expand_chunk_neighbors(&tenant, &anchor.document_id, anchor.chunk_index, 1, 1)
                .await?;
            results.append(&mut neighbors);
        }
        results
    }
}
```

In `crates/agent-core/src/runtime/graph_flow/mod.rs`:

- add `baseline: Arc<dyn BaselineAnswerPort>` to `GraphFlowRuntime`
- update `new` and `from_spec` signatures to accept the new port
- update `make_task()` to map `route_query`, `baseline_answer`, `retrieve_evidence`, and `compose_answer`
- update `extract_results()` to read `RouteDecision` from `keys::ROUTE_DECISION`
- update `AgentRunResult` construction to include the route decision

- [ ] **Step 4: Add the new agent spec**

Create `config/agents/agentic_search_v1.yaml`:

```yaml
agent_id: agentic_search_v1
description: >
  Routed search agent that chooses between the baseline single-pass path and
  a fixed-profile agentic evidence path.
spec_version: "1.0"

required_tools:
  - retrieval.dense
  - retrieval.sparse
  - retrieval.hybrid
  - retrieval.fts
  - retrieval.expand_chunk_neighbors
  - retrieval.fetch_document

tasks:
  - route_query
  - baseline_answer
  - retrieve_evidence
  - compose_answer
  - final_answer

graph:
  start_task: route_query
  tasks:
    - route_query
    - baseline_answer
    - retrieve_evidence
    - compose_answer
    - final_answer
  edges:
    - { from: route_query, to: retrieve_evidence, condition_key: route_to_agentic_search }
    - { from: route_query, to: baseline_answer }
    - { from: retrieve_evidence, to: compose_answer }
    - { from: compose_answer, to: final_answer }
    - { from: baseline_answer, to: final_answer }
```

- [ ] **Step 5: Run the agent-core test suite**

Run:

```bash
CARGO_TARGET_DIR=target/test cargo test -p agent-core --lib
```

Expected: the new runtime tests pass, and the existing `rag_spike` code path still compiles and passes its existing tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/agent-core/src/runtime/graph_flow/keys.rs crates/agent-core/src/runtime/graph_flow/tasks.rs crates/agent-core/src/runtime/graph_flow/mod.rs config/agents/agentic_search_v1.yaml
git commit -m "feat(agent-core): add routed search runtime branch"
```

---

## Task 5: Wire Server Adapters and Enrich the Agent Run API

**Files:**
- Modify: `crates/rag-server/src/agents.rs`
- Modify: `crates/rag-server/src/routes/agents.rs`
- Modify: `crates/rag-server/src/openapi.rs`
- Modify: `crates/rag-server/tests/agents.rs`

- [ ] **Step 1: Add failing server integration tests**

In `crates/rag-server/tests/agents.rs`, append:

```rust
#[tokio::test]
#[ignore] // requires `just up`
async fn agentic_search_v1_simple_query_routes_to_baseline() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agentic-simple-{suffix}");
    let tenant = format!("test-agentic-simple-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let body: serde_json::Value = execute.json().await.expect("json");
    assert_eq!(body["state"], "completed");
    assert_eq!(body["route_decision"]["selected_path"], "single_pass_rag");
    assert_eq!(body["answer"], "Mock LLM response.");
}

#[tokio::test]
#[ignore] // requires `just up`
async fn agentic_search_v1_comparison_query_routes_to_agentic() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agentic-complex-{suffix}");
    let tenant = format!("test-agentic-complex-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "Compare the main ideas in the document and explain the tradeoffs",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let body: serde_json::Value = execute.json().await.expect("json");
    assert_eq!(body["state"], "completed");
    assert_eq!(body["route_decision"]["selected_path"], "agentic_search");
    assert!(body["search_results"].as_array().expect("search_results").len() > 0);
    assert!(body["search_results"][0]["sources"].is_array());
}
```

- [ ] **Step 2: Run the server agent tests and confirm they fail**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agents -- --ignored --nocapture
```

Expected: compile failure because `agentic_search_v1`, `route_decision`, and the richer run payload are not wired yet.

- [ ] **Step 3: Implement the server-side adapters**

In `crates/rag-server/src/agents.rs`:

- update `AgentManager::load_default()` to pass the new baseline-answer adapter into `GraphFlowRuntime::from_spec`
- expand `ServerRetrievalPort` to implement all milestone-1 retrieval methods
- add `ServerBaselineAnswerPort`
- map `fetch_document()` to `state.stores.get_document()` / `Stores::get_document()` so the port contract is complete even if milestone-1 runtime profiles do not call it yet

Use this adapter shape:

```rust
struct ServerBaselineAnswerPort {
    chat: Arc<ChatService>,
}

#[async_trait::async_trait]
impl agent_core::ports::BaselineAnswerPort for ServerBaselineAnswerPort {
    async fn answer_single_shot(
        &self,
        query: &str,
        collection: &str,
        tenant: &str,
        language: Option<&str>,
    ) -> Result<agent_core::types::GroundedAnswer> {
        let grounded = self.chat.answer_single_shot(query, collection, tenant, language).await?;
        Ok(agent_core::types::GroundedAnswer {
            answer: grounded.answer,
            search_results: grounded
                .evidence
                .into_iter()
                .map(map_fused_chunk)
                .collect(),
            citations: grounded.citations.into_iter().map(|c| c.chunk_id).collect(),
            model: grounded.model,
        })
    }
}
```

Also add helper mappers in the same file:

```rust
fn map_fused_chunk(chunk: FusedChunk) -> ScoredChunk {
    ScoredChunk {
        chunk_id: chunk.chunk_id,
        document_id: chunk.document_id,
        chunk_index: chunk.chunk_index,
        text: chunk.text,
        title: chunk.title,
        source_url: chunk.source_url,
        source_domain: chunk.source_domain,
        language: chunk.language,
        tags: chunk.tags,
        section_heading: chunk.section_heading,
        collection: chunk.collection,
        score: chunk.fused_score,
        score_type: chunk.score_type,
        sources: chunk.sources,
        source_scores: chunk.source_scores,
    }
}
```

- [ ] **Step 4: Enrich the agent run response and OpenAPI**

In `crates/rag-server/src/routes/agents.rs`, add response DTOs:

```rust
#[derive(Serialize, utoipa::ToSchema)]
pub struct RouteDecisionResponse {
    pub selected_path: String,
    pub query_class: String,
    pub retrieval_profile: String,
    pub ambiguity: bool,
    pub needs_multi_hop: bool,
    pub needs_high_evidence: bool,
    pub time_sensitive: bool,
    pub normalized_filters: Vec<String>,
    pub reasons: Vec<String>,
}
```

Update `AgentRunResponse`:

```rust
pub struct AgentRunResponse {
    pub run_id: Uuid,
    pub state: String,
    pub answer: Option<String>,
    pub summary: Option<String>,
    pub query_type: Option<String>,
    pub route_decision: Option<RouteDecisionResponse>,
    pub pending_checkpoint: Option<PendingCheckpointResponse>,
    pub steps: Vec<StepRecordResponse>,
    pub search_results: Option<Vec<ScoredChunkResponse>>,
}
```

Expand `ScoredChunkResponse`:

```rust
pub struct ScoredChunkResponse {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub title: Option<String>,
    pub source_url: Option<String>,
    pub source_domain: Option<String>,
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub section_heading: Option<String>,
    pub collection: Option<String>,
    pub score: f32,
    pub score_type: String,
    pub sources: Vec<String>,
    pub source_scores: std::collections::HashMap<String, f32>,
}
```

Then update `map_run_response()` to populate `route_decision`, and register `RouteDecisionResponse` in `crates/rag-server/src/openapi.rs`:

```rust
agents::RouteDecisionResponse,
```

- [ ] **Step 5: Run the integration tests**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agents -- --ignored --nocapture
```

Expected: both new `agentic_search_v1` tests pass, and existing `rag_spike` approval/rejection tests still pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/rag-server/src/agents.rs crates/rag-server/src/routes/agents.rs crates/rag-server/src/openapi.rs crates/rag-server/tests/agents.rs
git commit -m "feat(rag-server): expose routed agentic search through agent runs"
```

---

## Task 6: Add the Offline Eval Harness, Benchmark Fixtures, and Runner Command

**Files:**
- Create: `crates/rag-server/tests/agentic_eval.rs`
- Create: `data/evals/agentic_search_v1/benchmark.json`
- Create: `data/evals/agentic_search_v1/corpus/rust-guide.md`
- Create: `data/evals/agentic_search_v1/corpus/python-tradeoffs.md`
- Create: `data/evals/agentic_search_v1/corpus/auth-runbook.md`
- Create: `data/evals/agentic_search_v1/corpus/deployment-notes.md`
- Create: `data/evals/agentic_search_v1/corpus/disambiguation-notes.md`
- Modify: `Justfile`
- Modify: `docs/howto/testing.md`

- [ ] **Step 1: Add the benchmark fixture set**

Create `data/evals/agentic_search_v1/benchmark.json`:

```json
[
  {
    "id": "simple-rust",
    "query": "What is Rust?",
    "expected_route": "single_pass_rag",
    "expected_query_class": "simple_fact",
    "expected_evidence": ["rust-guide"]
  },
  {
    "id": "api-key-runbook",
    "query": "How do I rotate API keys in the auth runbook?",
    "expected_route": "agentic_search",
    "expected_query_class": "procedural",
    "expected_evidence": ["auth-runbook"]
  },
  {
    "id": "python-vs-rust",
    "query": "Compare Rust and Python tradeoffs for async services",
    "expected_route": "agentic_search",
    "expected_query_class": "multi_hop_research",
    "expected_evidence": ["rust-guide", "python-tradeoffs"]
  },
  {
    "id": "ambiguous-deployment",
    "query": "Which deployment notes talk about rollback timing?",
    "expected_route": "agentic_search",
    "expected_query_class": "ambiguity_disambiguation",
    "expected_evidence": ["deployment-notes", "disambiguation-notes"]
  }
]
```

Create `data/evals/agentic_search_v1/corpus/rust-guide.md`:

```md
# Rust Guide

Rust is a systems programming language focused on safety and concurrency.
Ownership and borrowing prevent data races.
Tokio is a common async runtime for Rust services.
```

Create `data/evals/agentic_search_v1/corpus/python-tradeoffs.md`:

```md
# Python Tradeoffs

Python is productive for scripting and data work.
CPython's GIL limits true CPU-bound parallelism.
Async services often use asyncio and higher-level frameworks.
```

Create `data/evals/agentic_search_v1/corpus/auth-runbook.md`:

```md
# API Key Rotation Runbook

1. Create a replacement key.
2. Deploy the new key.
3. Verify traffic on the new key.
4. Revoke the old key after verification.
```

Create `data/evals/agentic_search_v1/corpus/deployment-notes.md`:

```md
# Deployment Notes

Rollback timing depends on the release window and incident severity.
Blue-green rollbacks are faster when the previous environment stays warm.
```

Create `data/evals/agentic_search_v1/corpus/disambiguation-notes.md`:

```md
# Disambiguation Notes

Rollback timing can refer either to deployment rollback windows or credential rotation cutovers.
Deployment rollback notes should be preferred for release incidents.
```

- [ ] **Step 2: Write the failing eval harness**

Create `crates/rag-server/tests/agentic_eval.rs`:

```rust
#![allow(clippy::disallowed_methods)]

mod common;

use std::fs;
use std::path::Path;

use reqwest::StatusCode;
use serde::Deserialize;
use test_support::spawn_app;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct BenchmarkCase {
    id: String,
    query: String,
    expected_route: String,
    expected_query_class: String,
    expected_evidence: Vec<String>,
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[tokio::test]
#[ignore] // requires `just up`
async fn benchmark_routes_and_evidence_are_scored() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");
    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("eval-agentic-{suffix}");
    let tenant = format!("eval-agentic-{suffix}");
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("data/evals/agentic_search_v1/corpus");
    let benchmark_path = corpus_dir.parent().expect("benchmark dir").join("benchmark.json");

    let benchmark: Vec<BenchmarkCase> =
        serde_json::from_str(&fs::read_to_string(benchmark_path).expect("benchmark json"))
            .expect("parse benchmark");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [corpus_dir.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let mut route_hits = 0usize;
    let mut evidence_hits = 0usize;

    for case in &benchmark {
        let baseline = client
            .post(format!("{}/chat", server.base_url()))
            .header("x-tenant", &tenant)
            .json(&serde_json::json!({
                "query": case.query,
                "collection": &collection
            }))
            .send()
            .await
            .expect("baseline");
        assert_eq!(baseline.status(), StatusCode::OK);

        let routed = client
            .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
            .header("x-tenant", &tenant)
            .json(&serde_json::json!({
                "query": case.query,
                "collection": &collection
            }))
            .send()
            .await
            .expect("routed");
        assert_eq!(routed.status(), StatusCode::OK);
        let body: serde_json::Value = routed.json().await.expect("routed json");

        if body["route_decision"]["selected_path"] == case.expected_route {
            route_hits += 1;
        }

        let docs: Vec<String> = body["search_results"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|item| item["document_id"].as_str().map(ToOwned::to_owned))
            .collect();

        if case.expected_evidence.iter().all(|doc| docs.iter().any(|seen| seen == doc)) {
            evidence_hits += 1;
        }
    }

    assert!(route_hits >= benchmark.len().saturating_sub(1), "route accuracy too low");
    assert!(evidence_hits >= 2, "evidence recall too low");
}
```

- [ ] **Step 3: Run the eval harness and confirm it fails before implementation**

Run:

```bash
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agentic_eval -- --ignored --nocapture --test-threads=1
```

Expected: failure because the routed path and enriched payload are not fully implemented yet.

- [ ] **Step 4: Add a dedicated runner command and docs**

In `Justfile`, add:

```just
# Offline benchmark for routed agentic search.
agentic-eval: up
    CARGO_TARGET_DIR={{target_dir_itest}} cargo test -p rag-server --test agentic_eval -- --ignored --nocapture --test-threads=1
```

In `docs/howto/testing.md`, add this command to the supported entry points list:

```md
just agentic-eval
```

and add a short section:

```md
### `just agentic-eval`

Use this to compare the baseline `/chat` path against `agentic_search_v1` on the checked-in benchmark set under `data/evals/agentic_search_v1/`.

Runs:

```bash
just up
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agentic_eval -- --ignored --nocapture --test-threads=1
```
```

- [ ] **Step 5: Run the full milestone verification sweep**

Run:

```bash
CARGO_TARGET_DIR=target/test cargo test -p agent-core --lib
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_chat_single_shot -- --ignored --nocapture
CARGO_TARGET_DIR=target/itest cargo test -p rag-core --test integration_retrieval -- --ignored --nocapture
CARGO_TARGET_DIR=target/itest cargo test -p rag-server --test agents -- --ignored --nocapture
just agentic-eval
```

Expected:

- agent-core unit tests pass
- rag-core single-shot and retrieval integration tests pass
- rag-server agent integration tests pass
- the eval harness passes its route/evidence thresholds

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/rag-server/tests/agentic_eval.rs data/evals/agentic_search_v1/ Justfile docs/howto/testing.md
git commit -m "test(eval): add routed agentic search benchmark harness"
```

---

## Final Verification

- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings -D clippy::disallowed_methods`
- [ ] Run `CARGO_TARGET_DIR=target/test cargo test --workspace`
- [ ] Run `just integration-test`
- [ ] Run `just agentic-eval`

## Notes for Execution

- Keep `rag_spike` green while adding `agentic_search_v1`; do not rewrite or remove the existing spike path in the same branch.
- When editing `crates/agent-core/src/runtime/graph_flow/tasks.rs`, prefer additive changes over broad restructuring. The file is already the task registry and runtime entry point.
- Keep `/chat` byte-for-byte compatible at the HTTP contract level. The extraction in `ChatService` is internal reuse, not a behavior rewrite.
- Preserve tenant scoping at every new retrieval/store entry point.
- Do not add live-traffic shadowing, persistence, or memory in this branch. Those belong to later milestones from the approved spec.
