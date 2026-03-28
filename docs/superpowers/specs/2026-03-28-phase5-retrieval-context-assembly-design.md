# Phase 5: Retrieval and Context Assembly

**Date:** 2026-03-28
**Status:** Approved
**Scope:** Single-collection hybrid search (dense + BM25 sparse) with RRF fusion and token-budget-aware context assembly.
**Approach:** Named vectors migration (Approach A) — Qdrant collections use named `"dense"` and `"bm25_sparse"` vectors.

---

## 1. Infrastructure Changes

### 1.1 Qdrant Collection Schema (`stores/vectors.rs`)

Current `ensure_collection` creates unnamed single-vector collections and rejects `ParamsMap`. This must change to support named vectors.

**New collection schema:**
- **Dense vectors:** `vectors_config` with `ParamsMap` containing `"dense"` (cosine distance, dimension from embedder)
- **Sparse vectors:** `sparse_vectors_config` containing `"bm25_sparse"`

These are two separate Qdrant configuration sections, not a single config with a flag.

**`ensure_collection` changes:**
- Accepts an explicit schema shape: dense vector name + size, and whether sparse `"bm25_sparse"` is required
- Creates collection with both `vectors_config` (ParamsMap) and `sparse_vectors_config`

**`validate_collection_config` changes:**
- Validates `ParamsMap["dense"]` exists with correct size + cosine distance
- Validates `sparse_vectors_config["bm25_sparse"]` exists when hybrid retrieval is enabled
- No longer rejects `ParamsMap` — that is now the expected config shape

**`search_dense` changes:**
- Sets `vector_name = "dense"` on the `SearchPointsBuilder` request
- Otherwise same signature and behavior

**New `search_sparse` method:**
- Uses Qdrant's query API (different request path from dense search) because sparse queries are represented differently from dense vectors
- Accepts collection, sparse vector (indices + values), tenant filter, limit
- Returns `Vec<ScoredPoint>` like `search_dense`

### 1.2 Point Construction (`ingest.rs`)

**`build_qdrant_points` changes:**
- Validates both `dense_vectors.len() == chunks.len()` and `sparse_vectors.len() == chunks.len()`
- Constructs points with `NamedVectors` containing both `"dense"` and `"bm25_sparse"` vectors
- Payload shape stays the same (tenant, document_id, chunk_index, text)

### 1.3 Data Impact

All existing Qdrant collections are incompatible with the new schema and must be re-created (`just down-v && just up` + re-ingest). No migration path — per project policy, backward compatibility is not required.

---

## 2. New Modules

Three new files in `crates/rag-core/src/`:

### 2.1 `retrieval.rs` — RetrievalService

Orchestrates search operations. Owns the embedder references needed to convert query text into vectors.

```rust
pub struct RetrievalService {
    stores: Stores,
    embedder: AnyEmbedder,
    bm25: Bm25Embedder,
    defaults: RetrievalDefaults,
}

pub struct RetrievalDefaults {
    pub rrf_k: u32,          // default 60
    pub dense_top_k: u64,    // default 20
    pub sparse_top_k: u64,   // default 20
}
```

`RetrievalDefaults` holds retrieval-specific config only. Token budget (`max_tokens`) belongs to `ContextConfig`, not here.

**Public methods:**

- `search_dense(collection, query, tenant, limit) -> Result<Vec<RetrievedChunk>>`
  Embeds query text, calls `stores.search_dense`, converts `ScoredPoint` to `RetrievedChunk`.

- `search_sparse(collection, query, tenant, limit) -> Result<Vec<RetrievedChunk>>`
  BM25-embeds query text, calls `stores.search_sparse`, converts to `RetrievedChunk`.

- `search_hybrid(collection, query, tenant, overrides: Option<HybridOverrides>) -> Result<Vec<FusedChunk>>`
  Runs dense + sparse in parallel via `tokio::join!`, passes both result sets to `rrf_fusion`. Accepts optional per-call overrides for `dense_top_k`, `sparse_top_k`, and `rrf_k`, defaulting from `RetrievalDefaults`.

```rust
pub struct HybridOverrides {
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}
```

### 2.2 `fusion.rs` — RRF Fusion (Pure Logic)

No IO, no collection-specific concerns. Pure ranking and merge logic.

```rust
pub fn rrf_fusion(
    results: &[(&str, Vec<RetrievedChunk>)],  // [("dense", [...]), ("sparse", [...])]
    k: u32,
) -> Vec<FusedChunk>
```

**RRF formula:** `score = sum(1 / (k + rank + 1))` for each source where chunk appears (0-indexed rank).

**Deterministic tie-breaking:** fused_score descending, then chunk_id ascending (lexicographic).

**Payload merge:** First occurrence wins for scalar fields. This is safe because all sources read from the same Qdrant point — payload fields (document_id, chunk_index, text) are identical for a given chunk_id. This assumption is explicitly documented in the code.

**Source tracking:** Each `FusedChunk` records which sources contributed and their original scores.

### 2.3 `context.rs` — ContextBuilder

Transforms fused retrieval results into LLM-ready context. Owns all presentation/formatting decisions.

```rust
pub struct ContextBuilder { /* tokenizer held internally */ }
```

Constructor takes tokenizer choice as a parameter (default: `cl100k_base`). This avoids storing tokenizer config in every `ContextConfig` while keeping it swappable if the generation model changes.

**Primary method:**

```rust
pub fn build(&self, chunks: Vec<FusedChunk>, config: &ContextConfig) -> ContextResult
```

Consumes `Vec<FusedChunk>` (ownership transferred — callers won't need the input after assembly).

**Assembly pipeline:**
1. Sort by fused_score descending, chunk_id ascending
2. Deduplicate per strategy
3. Token budget truncation (max_tokens and max_chunks both enforced independently)
4. Build citations for surviving chunks
5. Assemble context string

---

## 3. Types

### 3.1 Retrieval Types (`retrieval.rs`)

```rust
pub struct RetrievedChunk {
    pub chunk_id: String,       // Qdrant point ID
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,             // Original source score
}
```

Minimal and retrieval-focused. No `collection` field unless callers need it (single-collection scope makes it redundant).

### 3.2 Fusion Types (`fusion.rs`)

```rust
pub struct FusedChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
    pub sources: Vec<String>,
    pub source_scores: HashMap<String, f32>,
}
```

Enriched retrieval result, not a presentation type. `ContextBuilder` owns the transformation to `ContextChunk`.

### 3.3 Context Types (`context.rs`)

```rust
pub enum DedupeStrategy { ByDocId, ByChunkId, None }

pub struct ContextConfig {
    pub max_tokens: usize,       // default 8000
    pub max_chunks: usize,
    pub dedupe_strategy: DedupeStrategy,
    pub include_citations: bool,
}

pub struct ContextChunk {
    pub text: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub fused_score: f32,
    pub token_count: usize,
    pub citation: Option<Citation>,
}

pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

pub struct ContextResult {
    pub text: String,                   // Assembled context string for LLM
    pub chunks: Vec<ContextChunk>,      // Structured access to surviving chunks
    pub citations: Vec<Citation>,       // Top-level citation list
    pub stats: ContextStats,
}

pub struct ContextStats {
    pub input_count: usize,
    pub after_dedupe: usize,
    pub final_count: usize,
    pub dedupe_dropped: usize,
    pub budget_dropped: usize,
}
```

`Citation` includes `chunk_id` alongside `document_id` and `chunk_index` for easier downstream correlation without reconstructing identity from a pair.

`ContextResult.citations` is a top-level artifact (flattened from per-chunk citations) for consumers that need the full citation list without iterating chunks.

`ContextStats.final_count` (not `after_truncate`) for clarity.

---

## 4. Configuration

New fields in `AppConfig` (`config/app.toml`):

```toml
[retrieval]
rrf_k = 60
dense_top_k = 20
sparse_top_k = 20

[context]
max_tokens = 8000
max_chunks = 50
```

Precedence: per-call overrides > `app.toml` > hardcoded defaults.

---

## 5. Testing Strategy

### 5.1 Unit Tests (no IO)

**`fusion.rs`:**
- Single source: chunks ranked by original score, RRF formula verified
- Dual source: chunk appearing in both dense + sparse gets higher fused score reflecting both ranks (not just presence)
- Tie-breaking: equal fused scores broken by chunk_id ascending
- Empty inputs: no sources, empty result sets, single-chunk sets
- Score math: manually computed RRF scores verified against function output

**`context.rs`:**
- `ByDocId` dedup: second chunk from same document dropped
- `ByChunkId` dedup: duplicate chunk_id dropped, different chunks from same doc kept
- `None` dedup: all chunks kept
- Token budget: budget exceeded mid-list, remaining chunks dropped
- Token budget edge case: first chunk alone exceeds remaining budget — verify it is dropped cleanly
- Max chunks limit enforced independently of token budget
- Citation generation: citations present when enabled, absent when disabled
- Stats accuracy: input_count, after_dedupe, final_count, dedupe_dropped, budget_dropped all correct

### 5.2 Integration Tests (real Postgres + Qdrant)

**Test data:** 3-5 small markdown files with distinct content (e.g., "rust async programming", "python data science", "cooking recipes") so search relevance is predictable.

**Tests:**
- Dense search: query "rust async" returns rust document chunks ranked highest, with tenant isolation
- Sparse search: BM25 query returns keyword-relevant results, with tenant isolation (tested explicitly, not just "search")
- Hybrid search: chunk appearing in both dense and sparse results has fused score reflecting both ranks
- Tenant isolation (dense): tenant A search returns zero tenant B documents
- Tenant isolation (sparse): same verification for sparse path
- Context budget: ingest enough chunks to exceed 8000 tokens, verify truncation respects budget
- Context budget edge case: verify behavior when a single chunk exceeds remaining budget

---

## 6. Out of Scope (Deferred)

- Multi-collection search
- Semantic deduplication (requires embeddings at context-assembly time)
- Reranking (cross-encoder, LLM-based)
- MMR diversity sampling
- Query normalization, rewriting, intent classification
- Language-aware BM25 (stemming, stopwords beyond lowercase+alphanumeric)
- Layered/template-based context assembly
- Caching (embedding cache, query cache)
