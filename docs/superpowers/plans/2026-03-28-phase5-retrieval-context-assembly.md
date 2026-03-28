# Phase 5: Retrieval and Context Assembly — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add hybrid search (dense + BM25 sparse via RRF fusion) and token-budget-aware context assembly to `rag-core`.

**Architecture:** Migrate Qdrant collections to named vectors (`"dense"` + `"bm25_sparse"`), add three new modules (`fusion.rs`, `context.rs`, `retrieval.rs`) to `rag-core`. Fusion and context are pure logic (TDD first). Retrieval orchestrates search and delegates to fusion. Infrastructure changes to `vectors.rs` and `ingest.rs` wire up the new Qdrant schema.

**Tech Stack:** Rust (edition 2024), qdrant-client 1.17, tiktoken-rs (cl100k_base), tokio (parallel search), anyhow (errors).

**Design spec:** `docs/superpowers/specs/2026-03-28-phase5-retrieval-context-assembly-design.md`

---

### Task 1: Add retrieval and context config fields

**Files:**
- Modify: `crates/rag-core/src/config.rs`
- Modify: `config/app.toml`

- [ ] **Step 1: Add `RetrievalSection` and `ContextSection` TOML deserialization structs**

In `crates/rag-core/src/config.rs`, add two new deserialization structs after `AppSection` and update `AppSettings` to parse separate TOML tables:

```rust
#[derive(Deserialize, Default)]
struct AppSettings {
    app: Option<AppSection>,
    retrieval: Option<RetrievalSection>,
    context: Option<ContextSection>,
}

#[derive(Deserialize, Default)]
struct RetrievalSection {
    rrf_k: Option<u32>,
    dense_top_k: Option<u64>,
    sparse_top_k: Option<u64>,
}

#[derive(Deserialize, Default)]
struct ContextSection {
    max_tokens: Option<usize>,
    max_chunks: Option<usize>,
}
```

- [ ] **Step 2: Add fields to `AppConfig`**

```rust
pub struct AppConfig {
    // ... existing fields ...
    // Retrieval
    pub rrf_k: u32,
    pub dense_top_k: u64,
    pub sparse_top_k: u64,
    // Context assembly
    pub context_max_tokens: usize,
    pub context_max_chunks: usize,
}
```

- [ ] **Step 3: Update `from_current_env()` to parse the new sections**

Replace the `let file_settings = if ... { ... };` block with tuple destructuring. Keep the existing `let f = &file_settings;` line — it still works because `file_settings` is the first tuple element:

```rust
let (file_settings, retrieval_settings, context_settings) =
    if std::path::Path::new(&config_path).exists() {
        let contents = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading config file {config_path}"))?;
        let settings: AppSettings = toml::from_str(&contents)
            .with_context(|| format!("parsing config file {config_path}"))?;
        (
            settings.app.unwrap_or_default(),
            settings.retrieval.unwrap_or_default(),
            settings.context.unwrap_or_default(),
        )
    } else {
        (AppSection::default(), RetrievalSection::default(), ContextSection::default())
    };

let f = &file_settings;  // existing line — keep as-is
```

Then after the `request_id_header` resolution block, add:

```rust
let r = &retrieval_settings;
let ctx = &context_settings;

let rrf_k = env_parsed("RRF_K")?.or(r.rrf_k).unwrap_or(60);
let dense_top_k = env_parsed("DENSE_TOP_K")?.or(r.dense_top_k).unwrap_or(20);
let sparse_top_k = env_parsed("SPARSE_TOP_K")?.or(r.sparse_top_k).unwrap_or(20);
let context_max_tokens =
    env_parsed("CONTEXT_MAX_TOKENS")?.or(ctx.max_tokens).unwrap_or(8000);
let context_max_chunks =
    env_parsed("CONTEXT_MAX_CHUNKS")?.or(ctx.max_chunks).unwrap_or(50);
```

Add these fields to the `Ok(Self { ... })` return block.

- [ ] **Step 4: Update `app.toml`**

Append to `config/app.toml` as **separate TOML tables** (not under `[app]`):

```toml
[retrieval]
rrf_k = 60                                      # RRF fusion k parameter
dense_top_k = 20                                 # Dense search result limit
sparse_top_k = 20                                # Sparse/BM25 search result limit

[context]
max_tokens = 8000                                # Token budget for assembled context
max_chunks = 50                                  # Max chunks in assembled context
```

- [ ] **Step 5: Update config test**

In the `from_env_defaults_and_override` test, add assertions after the existing ones:

```rust
assert_eq!(cfg.rrf_k, 60);
assert_eq!(cfg.dense_top_k, 20);
assert_eq!(cfg.sparse_top_k, 20);
assert_eq!(cfg.context_max_tokens, 8000);
assert_eq!(cfg.context_max_chunks, 50);
```

Add `"RRF_K"`, `"DENSE_TOP_K"`, `"SPARSE_TOP_K"`, `"CONTEXT_MAX_TOKENS"`, `"CONTEXT_MAX_CHUNKS"` to the `clear_config_env()` function.

- [ ] **Step 6: Run tests**

Run: `cargo test -p rag-core config -- --nocapture`
Expected: all config tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rag-core/src/config.rs config/app.toml
git commit -m "feat(config): add retrieval and context assembly settings"
```

---

### Task 2: RRF fusion module (TDD)

**Files:**
- Create: `crates/rag-core/src/fusion.rs`
- Modify: `crates/rag-core/src/lib.rs` (add `pub mod fusion;`)

- [ ] **Step 1: Create `fusion.rs` with types and function stub**

Create `crates/rag-core/src/fusion.rs`:

```rust
//! Reciprocal Rank Fusion (RRF) for combining retrieval results.
//!
//! Pure ranking logic with no IO. Merges results from multiple search
//! sources (e.g. dense + sparse) into a single ranked list.

use std::collections::HashMap;

/// A single search result from one source (dense or sparse).
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

/// A chunk after RRF fusion across multiple sources.
#[derive(Debug, Clone)]
pub struct FusedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
    /// Which sources contributed (e.g. `["dense", "sparse"]`).
    pub sources: Vec<String>,
    /// Original score from each contributing source.
    pub source_scores: HashMap<String, f32>,
}

/// Fuse results from multiple retrieval sources using Reciprocal Rank Fusion.
///
/// Formula: `score = sum(1 / (k + rank + 1))` for each source containing the chunk.
/// Rank is 0-indexed within each source's result list.
///
/// Results are sorted by fused_score descending, then chunk_id ascending for
/// deterministic tie-breaking.
///
/// # Payload identity assumption
///
/// All sources read from the same Qdrant points, so payload fields (document_id,
/// chunk_index, text) are identical for a given chunk_id. First occurrence wins.
pub fn rrf_fusion(
    results: &[(&str, Vec<RetrievedChunk>)],
    k: u32,
) -> Vec<FusedChunk> {
    Vec::new() // stub
}
```

Add to `crates/rag-core/src/lib.rs`:

```rust
pub mod fusion;
```

And add to the re-exports:

```rust
pub use fusion::{FusedChunk, RetrievedChunk, rrf_fusion};
```

- [ ] **Step 2: Write failing test — single source**

Add at the bottom of `crates/rag-core/src/fusion.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, doc: &str, index: i32, score: f32) -> RetrievedChunk {
        RetrievedChunk {
            chunk_id: id.to_string(),
            document_id: doc.to_string(),
            chunk_index: index,
            text: format!("text of {id}"),
            score,
        }
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn single_source_ranks_by_rrf_score() {
        let dense = vec![
            chunk("c1", "doc1", 0, 0.9),
            chunk("c2", "doc1", 1, 0.8),
        ];

        let fused = rrf_fusion(&[("dense", dense)], 60);

        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].chunk_id, "c1");
        assert_eq!(fused[1].chunk_id, "c2");
        // RRF score for rank 0: 1/(60+0+1) = 1/61
        let expected_score_rank0 = 1.0 / 61.0;
        assert!((fused[0].fused_score - expected_score_rank0).abs() < 1e-6);
        // RRF score for rank 1: 1/(60+1+1) = 1/62
        let expected_score_rank1 = 1.0 / 62.0;
        assert!((fused[1].fused_score - expected_score_rank1).abs() < 1e-6);
        assert_eq!(fused[0].sources, vec!["dense"]);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rag-core fusion -- --nocapture`
Expected: FAIL — `assert_eq!(fused.len(), 2)` fails because stub returns empty vec.

- [ ] **Step 4: Implement `rrf_fusion`**

Replace the stub in `crates/rag-core/src/fusion.rs`:

```rust
pub fn rrf_fusion(
    results: &[(&str, Vec<RetrievedChunk>)],
    k: u32,
) -> Vec<FusedChunk> {
    // Accumulate RRF scores per chunk_id.
    let mut scores: HashMap<String, FusedChunk> = HashMap::new();

    for (source_name, chunks) in results {
        for (rank, chunk) in chunks.iter().enumerate() {
            let rrf_score = 1.0 / (k as f32 + rank as f32 + 1.0);

            let entry = scores.entry(chunk.chunk_id.clone()).or_insert_with(|| FusedChunk {
                chunk_id: chunk.chunk_id.clone(),
                document_id: chunk.document_id.clone(),
                chunk_index: chunk.chunk_index,
                text: chunk.text.clone(),
                fused_score: 0.0,
                sources: Vec::new(),
                source_scores: HashMap::new(),
            });

            entry.fused_score += rrf_score;
            entry.sources.push((*source_name).to_string());
            entry.source_scores.insert((*source_name).to_string(), chunk.score);
        }
    }

    let mut fused: Vec<FusedChunk> = scores.into_values().collect();
    fused.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });
    fused
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rag-core fusion -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Write test — dual source, chunk in both gets higher score**

Add to the `tests` module:

```rust
#[test]
#[allow(clippy::disallowed_methods)]
fn dual_source_chunk_in_both_scores_higher() {
    // c1 appears in both dense (rank 0) and sparse (rank 1)
    // c2 appears only in dense (rank 1)
    // c3 appears only in sparse (rank 0)
    let dense = vec![
        chunk("c1", "doc1", 0, 0.9),
        chunk("c2", "doc1", 1, 0.8),
    ];
    let sparse = vec![
        chunk("c3", "doc2", 0, 0.7),
        chunk("c1", "doc1", 0, 0.6),
    ];

    let fused = rrf_fusion(&[("dense", dense), ("sparse", sparse)], 60);

    assert_eq!(fused.len(), 3);
    // c1 should rank first: 1/61 (dense rank 0) + 1/62 (sparse rank 1)
    assert_eq!(fused[0].chunk_id, "c1");
    let expected_c1 = 1.0 / 61.0 + 1.0 / 62.0;
    assert!((fused[0].fused_score - expected_c1).abs() < 1e-6);
    assert_eq!(fused[0].sources.len(), 2);
    assert!(fused[0].source_scores.contains_key("dense"));
    assert!(fused[0].source_scores.contains_key("sparse"));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn tie_breaking_by_chunk_id() {
    // Two chunks at same rank in different sources => same RRF score
    let dense = vec![chunk("b_chunk", "doc1", 0, 0.9)];
    let sparse = vec![chunk("a_chunk", "doc2", 0, 0.8)];

    let fused = rrf_fusion(&[("dense", dense), ("sparse", sparse)], 60);

    assert_eq!(fused.len(), 2);
    // Same fused score (1/61 each), break by chunk_id ascending
    assert_eq!(fused[0].chunk_id, "a_chunk");
    assert_eq!(fused[1].chunk_id, "b_chunk");
}

#[test]
fn empty_inputs() {
    let fused = rrf_fusion(&[], 60);
    assert!(fused.is_empty());

    let fused = rrf_fusion(&[("dense", vec![])], 60);
    assert!(fused.is_empty());
}
```

- [ ] **Step 7: Run all fusion tests**

Run: `cargo test -p rag-core fusion -- --nocapture`
Expected: all 4 tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/rag-core/src/fusion.rs crates/rag-core/src/lib.rs
git commit -m "feat(fusion): add RRF fusion module with tests"
```

---

### Task 3: Context assembly module (TDD)

**Files:**
- Create: `crates/rag-core/src/context.rs`
- Modify: `crates/rag-core/src/lib.rs` (add `pub mod context;`)

- [ ] **Step 1: Create `context.rs` with types and builder stub**

Create `crates/rag-core/src/context.rs`:

```rust
//! Context assembly for LLM prompts.
//!
//! Transforms ranked retrieval results into a token-budget-aware context
//! string with optional citations and deduplication.

use std::sync::{Arc, LazyLock};

use tiktoken_rs::CoreBPE;

use crate::fusion::FusedChunk;

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[allow(clippy::disallowed_methods)] // LazyLock init requires expect
static CL100K_TOKENIZER: LazyLock<Arc<CoreBPE>> =
    LazyLock::new(|| Arc::new(tiktoken_rs::cl100k_base().expect("cl100k_base tokenizer must load")));

fn count_tokens(tokenizer: &CoreBPE, text: &str) -> usize {
    tokenizer.encode_ordinary(text).len()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Deduplication strategy for context assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupeStrategy {
    /// Keep one chunk per document_id (highest-scored survives).
    ByDocId,
    /// Keep one chunk per chunk_id (different chunks from same doc kept).
    ByChunkId,
    /// No deduplication.
    None,
}

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub max_tokens: usize,
    pub max_chunks: usize,
    pub dedupe_strategy: DedupeStrategy,
    pub include_citations: bool,
}

#[derive(Debug, Clone)]
pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextChunk {
    pub chunk_id: String,
    pub text: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub fused_score: f32,
    pub token_count: usize,
    pub citation: Option<Citation>,
}

#[derive(Debug, Clone)]
pub struct ContextResult {
    /// Assembled context string ready for LLM prompt.
    pub text: String,
    /// Structured access to surviving chunks.
    pub chunks: Vec<ContextChunk>,
    /// Top-level citation list (flattened from per-chunk citations).
    pub citations: Vec<Citation>,
    pub stats: ContextStats,
}

#[derive(Debug, Clone, Default)]
pub struct ContextStats {
    pub input_count: usize,
    pub after_dedupe: usize,
    pub final_count: usize,
    pub dedupe_dropped: usize,
    pub budget_dropped: usize,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Assembles fused retrieval results into LLM-ready context.
pub struct ContextBuilder {
    tokenizer: Arc<CoreBPE>,
}

impl ContextBuilder {
    /// Create a builder with the default `cl100k_base` tokenizer.
    pub fn new() -> Self {
        Self {
            tokenizer: Arc::clone(&CL100K_TOKENIZER),
        }
    }

    /// Create a builder with a custom tokenizer.
    pub fn with_tokenizer(tokenizer: Arc<CoreBPE>) -> Self {
        Self { tokenizer }
    }

    /// Assemble context from fused chunks.
    ///
    /// Pipeline: sort -> deduplicate -> token budget truncation -> citations -> text assembly.
    pub fn build(&self, chunks: Vec<FusedChunk>, config: &ContextConfig) -> ContextResult {
        ContextResult {
            text: String::new(),
            chunks: Vec::new(),
            citations: Vec::new(),
            stats: ContextStats::default(),
        } // stub
    }
}
```

Add to `crates/rag-core/src/lib.rs`:

```rust
pub mod context;
```

And add re-exports:

```rust
pub use context::{
    Citation, ContextBuilder, ContextChunk, ContextConfig, ContextResult, ContextStats,
    DedupeStrategy,
};
```

- [ ] **Step 2: Write failing test — ByDocId dedup**

Add at the bottom of `crates/rag-core/src/context.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::fusion::FusedChunk;

    fn fused(id: &str, doc: &str, index: i32, score: f32, text: &str) -> FusedChunk {
        FusedChunk {
            chunk_id: id.to_string(),
            document_id: doc.to_string(),
            chunk_index: index,
            text: text.to_string(),
            fused_score: score,
            sources: vec!["dense".to_string()],
            source_scores: HashMap::from([("dense".to_string(), score)]),
        }
    }

    fn default_config() -> ContextConfig {
        ContextConfig {
            max_tokens: 10_000,
            max_chunks: 50,
            dedupe_strategy: DedupeStrategy::None,
            include_citations: false,
        }
    }

    #[test]
    fn by_doc_id_keeps_highest_scored_per_document() {
        let chunks = vec![
            fused("c1", "doc1", 0, 0.9, "first chunk of doc1"),
            fused("c2", "doc1", 1, 0.5, "second chunk of doc1"),
            fused("c3", "doc2", 0, 0.7, "first chunk of doc2"),
        ];
        let config = ContextConfig {
            dedupe_strategy: DedupeStrategy::ByDocId,
            ..default_config()
        };

        let result = ContextBuilder::new().build(chunks, &config);

        assert_eq!(result.stats.input_count, 3);
        assert_eq!(result.stats.after_dedupe, 2);
        assert_eq!(result.stats.dedupe_dropped, 1);
        assert_eq!(result.chunks.len(), 2);
        // c1 (doc1, score 0.9) kept, c2 (doc1, score 0.5) dropped
        let ids: Vec<&str> = result.chunks.iter().map(|c| c.document_id.as_str()).collect();
        assert!(ids.contains(&"doc1"));
        assert!(ids.contains(&"doc2"));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rag-core context -- --nocapture`
Expected: FAIL — stats are all 0 from stub.

- [ ] **Step 4: Implement `ContextBuilder::build`**

Replace the stub:

```rust
pub fn build(&self, chunks: Vec<FusedChunk>, config: &ContextConfig) -> ContextResult {
    let input_count = chunks.len();

    // Step 1: Sort by fused_score desc, chunk_id asc.
    let mut sorted = chunks;
    sorted.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chunk_id.cmp(&b.chunk_id))
    });

    // Step 2: Deduplicate.
    let mut seen = std::collections::HashSet::new();
    let mut dedupe_dropped = 0usize;
    let deduped: Vec<FusedChunk> = sorted
        .into_iter()
        .filter(|chunk| {
            let key = match config.dedupe_strategy {
                DedupeStrategy::ByDocId => Some(chunk.document_id.clone()),
                DedupeStrategy::ByChunkId => Some(chunk.chunk_id.clone()),
                DedupeStrategy::None => return true,
            };
            if let Some(k) = key {
                if seen.insert(k) {
                    true
                } else {
                    dedupe_dropped += 1;
                    false
                }
            } else {
                true
            }
        })
        .collect();

    let after_dedupe = deduped.len();

    // Step 3: Token budget truncation.
    let mut total_tokens = 0usize;
    let mut budget_dropped = 0usize;
    let mut final_chunks: Vec<ContextChunk> = Vec::new();

    for chunk in deduped {
        if final_chunks.len() >= config.max_chunks {
            budget_dropped += 1;
            continue;
        }
        let token_count = count_tokens(&self.tokenizer, &chunk.text);
        if total_tokens + token_count > config.max_tokens {
            budget_dropped += 1;
            continue;
        }

        total_tokens += token_count;

        let citation = if config.include_citations {
            Some(Citation {
                chunk_id: chunk.chunk_id.clone(),
                document_id: chunk.document_id.clone(),
                chunk_index: chunk.chunk_index,
                sources: chunk.sources.clone(),
            })
        } else {
            None
        };

        final_chunks.push(ContextChunk {
            chunk_id: chunk.chunk_id,
            text: chunk.text,
            document_id: chunk.document_id,
            chunk_index: chunk.chunk_index,
            fused_score: chunk.fused_score,
            token_count,
            citation,
        });
    }

    // Step 4: Build top-level citations.
    let citations: Vec<Citation> = final_chunks
        .iter()
        .filter_map(|c| c.citation.clone())
        .collect();

    // Step 5: Assemble context string.
    let text = final_chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n\n");

    let final_count = final_chunks.len();

    ContextResult {
        text,
        chunks: final_chunks,
        citations,
        stats: ContextStats {
            input_count,
            after_dedupe,
            final_count,
            dedupe_dropped,
            budget_dropped,
        },
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rag-core context -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Write remaining tests**

Add to the `tests` module:

```rust
#[test]
fn by_chunk_id_allows_multiple_chunks_from_same_doc() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "first chunk"),
        fused("c1", "doc1", 0, 0.5, "duplicate c1"),
        fused("c2", "doc1", 1, 0.7, "second chunk same doc"),
    ];
    let config = ContextConfig {
        dedupe_strategy: DedupeStrategy::ByChunkId,
        ..default_config()
    };

    let result = ContextBuilder::new().build(chunks, &config);

    assert_eq!(result.stats.dedupe_dropped, 1);
    assert_eq!(result.chunks.len(), 2);
}

#[test]
fn none_dedupe_keeps_all() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "first"),
        fused("c2", "doc1", 1, 0.5, "second"),
    ];

    let result = ContextBuilder::new().build(chunks, &default_config());

    assert_eq!(result.stats.dedupe_dropped, 0);
    assert_eq!(result.chunks.len(), 2);
}

#[test]
fn token_budget_truncates() {
    // Each "word " is ~1 token. Create chunks that exceed budget.
    let big_text = "word ".repeat(500); // ~500 tokens
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, &big_text),
        fused("c2", "doc1", 1, 0.8, &big_text),
        fused("c3", "doc2", 0, 0.7, &big_text),
    ];
    let config = ContextConfig {
        max_tokens: 1100, // enough for ~2 chunks
        ..default_config()
    };

    let result = ContextBuilder::new().build(chunks, &config);

    assert!(result.stats.final_count <= 2);
    assert!(result.stats.budget_dropped >= 1);
}

#[test]
fn first_chunk_exceeding_remaining_budget_is_dropped() {
    let small = "hello world";
    let big = "word ".repeat(5000); // way over budget alone
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, small),
        fused("c2", "doc2", 0, 0.8, &big),
        fused("c3", "doc3", 0, 0.7, small),
    ];
    let config = ContextConfig {
        max_tokens: 100,
        ..default_config()
    };

    let result = ContextBuilder::new().build(chunks, &config);

    // c1 fits, c2 exceeds remaining budget and is dropped, c3 fits
    assert_eq!(result.stats.budget_dropped, 1);
    let ids: Vec<&str> = result.chunks.iter().map(|c| c.chunk_id.as_str()).collect();
    assert!(ids.contains(&"c1"));
    assert!(!ids.contains(&"c2"));
    assert!(ids.contains(&"c3"));
}

#[test]
fn max_chunks_limit_enforced() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "a"),
        fused("c2", "doc2", 0, 0.8, "b"),
        fused("c3", "doc3", 0, 0.7, "c"),
    ];
    let config = ContextConfig {
        max_chunks: 2,
        ..default_config()
    };

    let result = ContextBuilder::new().build(chunks, &config);

    assert_eq!(result.stats.final_count, 2);
    assert_eq!(result.stats.budget_dropped, 1);
}

#[test]
fn citations_included_when_enabled() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "text"),
    ];
    let config = ContextConfig {
        include_citations: true,
        ..default_config()
    };

    let result = ContextBuilder::new().build(chunks, &config);

    assert_eq!(result.citations.len(), 1);
    assert_eq!(result.citations[0].chunk_id, "c1");
    assert_eq!(result.citations[0].document_id, "doc1");
    assert_eq!(result.citations[0].chunk_index, 0);
}

#[test]
fn citations_absent_when_disabled() {
    let chunks = vec![fused("c1", "doc1", 0, 0.9, "text")];

    let result = ContextBuilder::new().build(chunks, &default_config());

    assert!(result.citations.is_empty());
}

#[test]
fn stats_are_accurate() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "hello"),
        fused("c2", "doc1", 1, 0.8, "world"),
        fused("c3", "doc2", 0, 0.7, "foo"),
    ];
    let config = ContextConfig {
        max_tokens: 10_000,
        max_chunks: 50,
        dedupe_strategy: DedupeStrategy::ByDocId,
        include_citations: false,
    };

    let result = ContextBuilder::new().build(chunks, &config);

    assert_eq!(result.stats.input_count, 3);
    assert_eq!(result.stats.after_dedupe, 2);   // c2 dropped (same doc as c1)
    assert_eq!(result.stats.final_count, 2);
    assert_eq!(result.stats.dedupe_dropped, 1);
    assert_eq!(result.stats.budget_dropped, 0);
}

#[test]
fn assembled_text_joins_with_double_newline() {
    let chunks = vec![
        fused("c1", "doc1", 0, 0.9, "first"),
        fused("c2", "doc2", 0, 0.8, "second"),
    ];

    let result = ContextBuilder::new().build(chunks, &default_config());

    assert_eq!(result.text, "first\n\nsecond");
}
```

- [ ] **Step 7: Run all context tests**

Run: `cargo test -p rag-core context -- --nocapture`
Expected: all tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/rag-core/src/context.rs crates/rag-core/src/lib.rs
git commit -m "feat(context): add context assembly module with tests"
```

---

### Task 4: Qdrant named vectors migration

**Files:**
- Modify: `crates/rag-core/src/stores/vectors.rs`

**Reference:** The Qdrant client uses `CreateCollection` with `VectorsConfig::ParamsMap` for named dense vectors and `SparseVectorConfig` for sparse vectors. Dense search uses `SearchPoints` with `vector_name`. Sparse search uses `SearchPoints` with `vector_name` + `sparse_indices`.

- [ ] **Step 1: Update imports**

Replace the imports at the top of `crates/rag-core/src/stores/vectors.rs`:

```rust
use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use qdrant_client::qdrant::{
    Condition, CreateCollection, DeletePointsBuilder, Distance, Filter, Modifier, PointStruct,
    ScoredPoint, SearchPoints, SparseIndices, SparseVectorConfig, SparseVectorParams,
    UpsertPointsBuilder, VectorParams, VectorParamsMap, VectorsConfig, WithPayloadSelector,
    vectors_config::Config as VectorsConfigVariant,
    with_payload_selector::SelectorOptions,
};

use super::Stores;

/// Named vector key for dense embeddings.
pub const DENSE_VECTOR_NAME: &str = "dense";
/// Named vector key for BM25 sparse embeddings.
pub const SPARSE_VECTOR_NAME: &str = "bm25_sparse";
```

- [ ] **Step 2: Rewrite `validate_collection_config`**

Replace the existing `validate_collection_config` method:

```rust
async fn validate_collection_config(
    &self,
    name: &str,
    dense_size: u64,
    distance: Distance,
) -> Result<()> {
    let info = self
        .qdrant
        .collection_info(name)
        .await
        .with_context(|| format!("fetching collection info for '{name}'"))?;

    let params = info
        .result
        .as_ref()
        .and_then(|r| r.config.as_ref())
        .and_then(|c| c.params.as_ref())
        .ok_or_else(|| anyhow!("collection '{name}' is missing configuration details"))?;

    // Validate dense named vector.
    let vectors_config = params
        .vectors_config
        .as_ref()
        .ok_or_else(|| anyhow!("collection '{name}' is missing vectors_config"))?;

    match &vectors_config.config {
        Some(VectorsConfigVariant::ParamsMap(map)) => {
            let dense_params = map.map.get(DENSE_VECTOR_NAME).ok_or_else(|| {
                anyhow!("collection '{name}' is missing named vector '{DENSE_VECTOR_NAME}'")
            })?;
            if dense_params.size != dense_size {
                return Err(anyhow!(
                    "collection '{name}' dense vector size {}, expected {dense_size}",
                    dense_params.size,
                ));
            }
            if dense_params.distance() != distance {
                return Err(anyhow!(
                    "collection '{name}' dense distance {:?}, expected {distance:?}",
                    dense_params.distance(),
                ));
            }
        }
        _ => {
            return Err(anyhow!(
                "collection '{name}' does not use named vectors (expected ParamsMap)"
            ));
        }
    }

    // Validate sparse vector config.
    let sparse_config = params
        .sparse_vectors_config
        .as_ref()
        .ok_or_else(|| anyhow!("collection '{name}' is missing sparse_vectors_config"))?;
    if !sparse_config.map.contains_key(SPARSE_VECTOR_NAME) {
        return Err(anyhow!(
            "collection '{name}' is missing sparse vector '{SPARSE_VECTOR_NAME}'"
        ));
    }

    Ok(())
}
```

- [ ] **Step 3: Rewrite `ensure_collection`**

Replace the existing `ensure_collection` method. The new version always creates collections with both dense and sparse named vectors:

```rust
pub async fn ensure_collection(
    &self,
    name: &str,
    dense_size: u64,
    distance: Distance,
) -> Result<()> {
    if self
        .qdrant
        .collection_exists(name)
        .await
        .context("checking if Qdrant collection exists")?
    {
        self.validate_collection_config(name, dense_size, distance).await
    } else {
        let mut dense_map = HashMap::new();
        dense_map.insert(
            DENSE_VECTOR_NAME.to_string(),
            VectorParams {
                size: dense_size,
                distance: distance.into(),
                ..Default::default()
            },
        );

        let mut sparse_map = HashMap::new();
        sparse_map.insert(
            SPARSE_VECTOR_NAME.to_string(),
            SparseVectorParams {
                index: None,
                modifier: Some(Modifier::Idf.into()),
            },
        );

        let request = CreateCollection {
            collection_name: name.to_string(),
            vectors_config: Some(VectorsConfig {
                config: Some(VectorsConfigVariant::ParamsMap(VectorParamsMap {
                    map: dense_map,
                })),
            }),
            sparse_vectors_config: Some(SparseVectorConfig { map: sparse_map }),
            ..Default::default()
        };

        match self.qdrant.create_collection(request).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Handle TOCTOU race.
                if self
                    .qdrant
                    .collection_exists(name)
                    .await
                    .context("rechecking Qdrant collection after create race")?
                {
                    self.validate_collection_config(name, dense_size, distance).await
                } else {
                    Err(e).with_context(|| format!("creating Qdrant collection '{name}'"))
                }
            }
        }
    }
}
```

- [ ] **Step 4: Update `search_dense` to use named vector**

Replace the existing `search_dense` method:

```rust
pub async fn search_dense(
    &self,
    collection: &str,
    vector: Vec<f32>,
    tenant: &str,
    limit: u64,
) -> Result<Vec<ScoredPoint>> {
    let request = SearchPoints {
        collection_name: collection.to_string(),
        vector: vector,
        filter: Some(Filter::must([Condition::matches(
            "tenant",
            tenant.to_string(),
        )])),
        limit,
        vector_name: Some(DENSE_VECTOR_NAME.to_string()),
        with_payload: Some(WithPayloadSelector {
            selector_options: Some(SelectorOptions::Enable(true)),
        }),
        ..Default::default()
    };

    let response = self.qdrant.search_points(request).await.with_context(|| {
        format!("searching dense vectors in collection '{collection}' for tenant '{tenant}'")
    })?;

    Ok(response.result)
}
```

- [ ] **Step 5: Add `search_sparse` method**

Add after `search_dense`:

```rust
/// Sparse (BM25) vector search with a tenant filter.
///
/// Returns up to `limit` scored points ordered by descending relevance.
pub async fn search_sparse(
    &self,
    collection: &str,
    indices: Vec<u32>,
    values: Vec<f32>,
    tenant: &str,
    limit: u64,
) -> Result<Vec<ScoredPoint>> {
    if indices.is_empty() {
        return Ok(Vec::new());
    }

    let request = SearchPoints {
        collection_name: collection.to_string(),
        vector: values,
        sparse_indices: Some(SparseIndices { data: indices }),
        filter: Some(Filter::must([Condition::matches(
            "tenant",
            tenant.to_string(),
        )])),
        limit,
        vector_name: Some(SPARSE_VECTOR_NAME.to_string()),
        with_payload: Some(WithPayloadSelector {
            selector_options: Some(SelectorOptions::Enable(true)),
        }),
        ..Default::default()
    };

    let response = self.qdrant.search_points(request).await.with_context(|| {
        format!("searching sparse vectors in collection '{collection}' for tenant '{tenant}'")
    })?;

    Ok(response.result)
}
```

- [ ] **Step 6: Run `cargo check -p rag-core`**

Run: `cargo check -p rag-core`
Expected: compiles. Fix any type mismatches (e.g. `distance.into()` vs `distance as i32`).

- [ ] **Step 7: Commit**

```bash
git add crates/rag-core/src/stores/vectors.rs
git commit -m "feat(vectors): migrate to named vectors with sparse support"
```

---

### Task 5: Update ingest point construction

**Files:**
- Modify: `crates/rag-core/src/ingest.rs`

- [ ] **Step 1: Update imports in `ingest.rs`**

Add to the existing imports at the top of `crates/rag-core/src/ingest.rs`:

```rust
use qdrant_client::qdrant::{DenseVector, NamedVectors, PointStruct, Vectors, Vector as QdrantVector};
use qdrant_client::qdrant::SparseVector as QdrantSparseVector;
```

Remove `PointStruct` from the existing `use qdrant_client::qdrant::{Distance, PointStruct};` line if it was there, and adjust so `Distance` stays imported but `PointStruct` comes from the expanded import above. Also import the vectors constant:

```rust
use crate::stores::vectors::DENSE_VECTOR_NAME;
use crate::stores::vectors::SPARSE_VECTOR_NAME;
```

- [ ] **Step 2: Rewrite `build_qdrant_points`**

Replace the existing `build_qdrant_points` function:

```rust
fn build_qdrant_points(tenant: &str, doc: &EmbeddedDocument) -> Result<Vec<PointStruct>> {
    if doc.dense_vectors.len() != doc.chunks.len() {
        bail!(
            "embedder returned {} dense vectors for {} chunks",
            doc.dense_vectors.len(),
            doc.chunks.len()
        );
    }
    if doc.sparse_vectors.len() != doc.chunks.len() {
        bail!(
            "BM25 embedder returned {} sparse vectors for {} chunks",
            doc.sparse_vectors.len(),
            doc.chunks.len()
        );
    }

    Ok(doc
        .chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let point_id = stable_chunk_uuid(tenant, &doc.document_id, i);

            let payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
                ("tenant".to_string(), tenant.to_string().into()),
                ("document_id".to_string(), doc.document_id.clone().into()),
                ("chunk_index".to_string(), (i as i64).into()),
                ("text".to_string(), chunk.text.clone().into()),
            ]
            .into();

            // Build named vectors with both dense and sparse.
            let dense = QdrantVector::from(DenseVector {
                data: doc.dense_vectors[i].clone(),
            });
            let sparse = QdrantVector::from(QdrantSparseVector {
                indices: doc.sparse_vectors[i].indices.clone(),
                values: doc.sparse_vectors[i].values.clone(),
            });

            let mut named = std::collections::HashMap::new();
            named.insert(DENSE_VECTOR_NAME.to_string(), dense);
            named.insert(SPARSE_VECTOR_NAME.to_string(), sparse);

            PointStruct {
                id: Some(point_id.to_string().into()),
                payload,
                vectors: Some(Vectors::from(NamedVectors { vectors: named })),
            }
        })
        .collect())
}
```

- [ ] **Step 3: Run `cargo check -p rag-core`**

Run: `cargo check -p rag-core`
Expected: compiles. Fix any import or type issues.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-core/src/ingest.rs
git commit -m "feat(ingest): store named dense + sparse vectors in Qdrant points"
```

---

### Task 6: Retrieval service

**Files:**
- Create: `crates/rag-core/src/retrieval.rs`
- Modify: `crates/rag-core/src/lib.rs` (add `pub mod retrieval;` and re-exports)

- [ ] **Step 1: Create `retrieval.rs` with types and service**

Create `crates/rag-core/src/retrieval.rs`:

```rust
//! Retrieval service orchestrating dense, sparse, and hybrid search.

use anyhow::{Context, Result};

use crate::bm25::Bm25Embedder;
use crate::config::AppConfig;
use crate::embed::{AnyEmbedder, EmbedService};
use crate::fusion::{FusedChunk, RetrievedChunk, rrf_fusion};
use crate::stores::Stores;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Default retrieval parameters, loaded from config.
#[derive(Debug, Clone)]
pub struct RetrievalDefaults {
    pub rrf_k: u32,
    pub dense_top_k: u64,
    pub sparse_top_k: u64,
}

impl RetrievalDefaults {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            rrf_k: config.rrf_k,
            dense_top_k: config.dense_top_k,
            sparse_top_k: config.sparse_top_k,
        }
    }
}

/// Optional per-call overrides for hybrid search parameters.
#[derive(Debug, Clone, Default)]
pub struct HybridOverrides {
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct RetrievalService {
    stores: Stores,
    embedder: AnyEmbedder,
    bm25: Bm25Embedder,
    defaults: RetrievalDefaults,
}

impl RetrievalService {
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let embedder = AnyEmbedder::from_config(config).context("building embedder")?;
        let bm25 = Bm25Embedder::from_app_config(config).context("building BM25 embedder")?;
        let defaults = RetrievalDefaults::from_config(config);

        Ok(Self { stores, embedder, bm25, defaults })
    }

    /// Dense vector search: embed query, search Qdrant, return ranked chunks.
    pub async fn search_dense(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<RetrievedChunk>> {
        let query_vec = self
            .embedder
            .embed_batch(&[query.to_string()])
            .await
            .context("embedding query for dense search")?
            .into_iter()
            .next()
            .context("embedder returned empty result")?;

        let scored = self
            .stores
            .search_dense(collection, query_vec, tenant, limit)
            .await
            .context("dense search")?;

        Ok(scored_points_to_chunks(scored))
    }

    /// Sparse BM25 search: BM25-embed query, search Qdrant, return ranked chunks.
    pub async fn search_sparse(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        limit: u64,
    ) -> Result<Vec<RetrievedChunk>> {
        let sparse = self.bm25.embed_query(query, None);

        if sparse.indices.is_empty() {
            return Ok(Vec::new());
        }

        let scored = self
            .stores
            .search_sparse(collection, sparse.indices, sparse.values, tenant, limit)
            .await
            .context("sparse search")?;

        Ok(scored_points_to_chunks(scored))
    }

    /// Hybrid search: run dense + sparse in parallel, fuse with RRF.
    pub async fn search_hybrid(
        &self,
        collection: &str,
        query: &str,
        tenant: &str,
        overrides: Option<HybridOverrides>,
    ) -> Result<Vec<FusedChunk>> {
        let ov = overrides.unwrap_or_default();
        let dense_k = ov.dense_top_k.unwrap_or(self.defaults.dense_top_k);
        let sparse_k = ov.sparse_top_k.unwrap_or(self.defaults.sparse_top_k);
        let rrf_k = ov.rrf_k.unwrap_or(self.defaults.rrf_k);

        let (dense_result, sparse_result) = tokio::join!(
            self.search_dense(collection, query, tenant, dense_k),
            self.search_sparse(collection, query, tenant, sparse_k),
        );

        let dense = dense_result.context("hybrid dense leg")?;
        let sparse = sparse_result.context("hybrid sparse leg")?;

        Ok(rrf_fusion(&[("dense", dense), ("sparse", sparse)], rrf_k))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_point_id(id: &qdrant_client::qdrant::PointId) -> String {
    use qdrant_client::qdrant::point_id::PointIdOptions;
    match &id.point_id_options {
        Some(PointIdOptions::Uuid(s)) => s.clone(),
        Some(PointIdOptions::Num(n)) => n.to_string(),
        None => String::new(),
    }
}

fn scored_points_to_chunks(
    scored: Vec<qdrant_client::qdrant::ScoredPoint>,
) -> Vec<RetrievedChunk> {
    scored
        .into_iter()
        .filter_map(|point| {
            let payload = &point.payload;
            let chunk_id = point.id.as_ref().map(extract_point_id).unwrap_or_default();
            let document_id = payload
                .get("document_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let chunk_index = payload
                .get("chunk_index")
                .and_then(|v| v.as_integer())
                .unwrap_or(0) as i32;
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            Some(RetrievedChunk {
                chunk_id,
                document_id,
                chunk_index,
                text,
                score: point.score,
            })
        })
        .collect()
}
```

- [ ] **Step 2: Update `lib.rs` exports**

In `crates/rag-core/src/lib.rs`, add the module declaration:

```rust
pub mod retrieval;
```

And add re-exports:

```rust
pub use retrieval::{HybridOverrides, RetrievalDefaults, RetrievalService};
```

- [ ] **Step 3: Run `cargo check -p rag-core`**

Run: `cargo check -p rag-core`
Expected: compiles. The `qdrant_client::qdrant::Value` type provides `as_str() -> Option<&String>` and `as_integer() -> Option<i64>` via its `value_extract_impl` macros.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-core/src/retrieval.rs crates/rag-core/src/lib.rs
git commit -m "feat(retrieval): add RetrievalService with dense, sparse, and hybrid search"
```

---

### Task 7: Integration tests

**Files:**
- Create: `crates/rag-core/tests/integration_retrieval.rs`

**Prerequisite:** Postgres + Qdrant running (`just up`). Existing Qdrant collections must be re-created (incompatible schema): `just down-v && just up`.

- [ ] **Step 1: Create integration test file with setup helpers**

Create `crates/rag-core/tests/integration_retrieval.rs`:

```rust
//! Integration tests for retrieval and context assembly.
//!
//! Requires Postgres and Qdrant running (`just up`).
//! Uses UUID-scoped tenant and collection per test for isolation.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::context::{ContextBuilder, ContextConfig, DedupeStrategy};
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::retrieval::RetrievalService;
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

async fn setup() -> Result<(IngestService, RetrievalService, AppConfig)> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    // Create separate Stores instances (Stores doesn't implement Clone;
    // PgPool and Arc<Qdrant> are cheap to construct from the same config).
    let ingest_stores = Stores::new(&config).await?;
    let retrieval_stores = Stores::new(&config).await?;
    let ingest = IngestService::new(ingest_stores, &config)?;
    let retrieval = RetrievalService::new(retrieval_stores, &config)?;
    Ok((ingest, retrieval, config))
}

/// Ingest test fixtures into a unique collection, return (tenant, collection).
async fn ingest_fixtures(
    ingest: &IngestService,
    dir: &TempDir,
) -> Result<(TenantId, String)> {
    let suffix = unique_id();
    let tenant: TenantId = format!("test-{suffix}").parse()?;
    let collection = format!("test-coll-{suffix}");

    // Three documents with distinct content for predictable retrieval.
    // Sidecars are optional but provide document IDs for stable assertions.
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
        "python.md",
        "Python is a high-level programming language for data science and scripting. \
         NumPy and pandas provide efficient data manipulation. \
         The GIL limits true parallelism in CPython.",
    );
    write_sidecar(dir.path(), "python");
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
            collection_override: Some(collection.clone()),
        })
        .await?;

    Ok((tenant, collection))
}
```

- [ ] **Step 2: Write dense search test with tenant isolation**

Add to the same file:

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn dense_search_returns_relevant_results_with_tenant_isolation() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    // Search for Rust-related content.
    let results = retrieval
        .search_dense(&collection, "rust async programming", tenant.as_str(), 10)
        .await?;

    assert!(!results.is_empty(), "dense search should return results");
    // Note: mock embedder produces SHA-256-seeded pseudo-random vectors,
    // so dense ranking is non-semantic. We can only verify that results
    // come back, not their relevance order.
    // Verify all returned chunks have populated fields.
    for chunk in &results {
        assert!(!chunk.chunk_id.is_empty(), "chunk_id should be populated");
        assert!(!chunk.document_id.is_empty(), "document_id should be populated");
        assert!(!chunk.text.is_empty(), "text should be populated");
    }

    // Tenant isolation: different tenant sees nothing.
    let other_tenant: TenantId = format!("other-{}", unique_id()).parse()?;
    let isolated = retrieval
        .search_dense(&collection, "rust", other_tenant.as_str(), 10)
        .await?;
    assert!(isolated.is_empty(), "other tenant should see no results");

    Ok(())
}
```

- [ ] **Step 3: Write sparse search test with tenant isolation**

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn sparse_search_returns_results_with_tenant_isolation() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let results = retrieval
        .search_sparse(&collection, "sourdough bread fermentation", tenant.as_str(), 10)
        .await?;

    assert!(!results.is_empty(), "sparse search should return results");
    // Top result should be from the cooking document (keyword overlap).
    assert_eq!(
        results[0].document_id, "cooking",
        "top sparse result for 'sourdough bread fermentation' should be from cooking.md"
    );

    // Tenant isolation.
    let other_tenant: TenantId = format!("other-{}", unique_id()).parse()?;
    let isolated = retrieval
        .search_sparse(&collection, "sourdough", other_tenant.as_str(), 10)
        .await?;
    assert!(isolated.is_empty(), "other tenant should see no results");

    Ok(())
}
```

- [ ] **Step 4: Write hybrid search test — fused score reflects both ranks**

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn hybrid_search_fuses_dense_and_sparse() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval
        .search_hybrid(&collection, "rust programming", tenant.as_str(), None)
        .await?;

    assert!(!fused.is_empty(), "hybrid search should return results");
    // Note: mock embedder makes dense ranking non-semantic, so we don't
    // assert top-result relevance. Focus on structural correctness:
    // fused results should have scores, sources, and populated fields.
    for chunk in &fused {
        assert!(!chunk.chunk_id.is_empty());
        assert!(!chunk.document_id.is_empty());
        assert!(chunk.fused_score > 0.0, "fused score should be positive");
    }

    // At least one chunk must appear in both sources to validate RRF fusion.
    let multi_source = fused
        .iter()
        .find(|c| c.sources.len() > 1)
        .expect("hybrid search should produce at least one chunk found by both dense and sparse");
    // A multi-source chunk's fused score must exceed any single-source chunk's score
    // (both contribute 1/(k+rank+1) terms).
    if let Some(single) = fused.iter().find(|c| c.sources.len() == 1) {
        assert!(
            multi_source.fused_score > single.fused_score,
            "multi-source chunk ({}) should score higher than single-source chunk ({})",
            multi_source.fused_score,
            single.fused_score,
        );
    }
    // Verify source tracking.
    for chunk in &fused {
        assert!(!chunk.sources.is_empty());
        for source in &chunk.sources {
            assert!(chunk.source_scores.contains_key(source));
        }
    }

    Ok(())
}
```

- [ ] **Step 5: Write context budget test**

```rust
#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn context_assembly_respects_token_budget() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval
        .search_hybrid(&collection, "programming language", tenant.as_str(), None)
        .await?;

    assert!(!fused.is_empty(), "need results for context test");

    // Use a very small token budget to force truncation.
    let config = ContextConfig {
        max_tokens: 20,
        max_chunks: 50,
        dedupe_strategy: DedupeStrategy::None,
        include_citations: true,
    };

    let result = ContextBuilder::new().build(fused, &config);

    assert!(result.stats.final_count <= result.stats.input_count);
    if result.stats.input_count > result.stats.final_count {
        assert!(result.stats.budget_dropped > 0);
    }
    // Assembled text should not be empty if any chunks fit.
    if result.stats.final_count > 0 {
        assert!(!result.text.is_empty());
        assert_eq!(result.citations.len(), result.stats.final_count);
    }

    Ok(())
}
```

- [ ] **Step 6: Run integration tests**

Run: `cargo test -p rag-core --test integration_retrieval -- --ignored --nocapture`
Expected: all 4 tests PASS. If Qdrant collections have stale schema, run `just down-v && just up` first and re-run.

- [ ] **Step 7: Commit**

```bash
git add crates/rag-core/tests/integration_retrieval.rs
git commit -m "test(retrieval): add integration tests for search and context assembly"
```
