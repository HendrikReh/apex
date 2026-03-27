# Phase 3: Extraction and Embeddings — Design Spec

**Date:** 2026-03-27
**Status:** Implemented
**Commits:** 73d7322, 2370193, 348e951, 8334a0c

---

## Scope

Phase 3 adds text extraction, dense embeddings, BM25 sparse vectors, and checksum
computation to `rag-core`. Together with Phase 1 (chunking) and Phase 2 (storage),
this completes the data-processing primitives needed for the Phase 4 ingest pipeline.

### In scope

- `ExtractorRegistry` with Markdown, plain text, and a PDF placeholder (pdfium wiring is a follow-up)
- `EmbedService` trait with `OpenAiEmbedder` and `MockEmbedder`
- `Bm25Embedder` with core BM25 sparse vector generation
- SHA-256 checksum of extracted text

### Out of scope (deferred)

- OCR fallback (Tesseract) — Phase 13
- Additional extractors (DOCX, PPTX, HTML, etc.) — Phase 13
- BM25 advanced tokenization (CJK bigrams, stopwords, stemming, field boosting) — future hardening
- Circuit breaker for OpenAI — future hardening
- Dynamic avgdl from corpus_stats — wire in Phase 4 or later

---

## Design Decisions

### 1. Async extraction interface

The `FormatExtractor` trait uses `BoxFuture` even though current extractors (MD,
TXT) are synchronous and the PDF extractor is a placeholder. This avoids a breaking
interface change when pdfium and OCR (subprocess-backed) arrive later.

### 2. Bytes-in interface (`&[u8]`)

Extractors take `&[u8]` not `&Path`. Callers handle I/O. This keeps extractors
pure and testable without filesystem access.

### 3. Registry pattern for extraction

`ExtractorRegistry` stores `Vec<Box<dyn FormatExtractor>>` with a
`HashMap<FileType, usize>` index. Callers pass an explicit `FileType` to `extract()`.
Rejects duplicate registrations at construction time. Scales to Phase 13's 15+
extractors without restructuring.

### 4. Single embed trait method

`EmbedService` exposes `embed_batch` only. Retry/backoff is an internal concern of
`OpenAiEmbedder`, not a trait contract. This diverges from projectAlpha which
exposed `embed_batch_with_retry` on the trait.

### 5. Core BM25 only

Minimal tokenizer (lowercase + split on non-alphanumeric) instead of projectAlpha's
full pipeline (CJK bigrams, stopwords, stemming, field boosting). The `Bm25Embedder`
wraps the `bm25` crate but bypasses its default tokenizer to keep behavior explicit
and predictable. Advanced tokenization features can be layered on later without
changing the `SparseVector` output type.

### 6. Checksum on extracted text, not raw bytes

`checksum(text: &str) -> String` hashes the extracted text so that extractor
improvements naturally invalidate stale checksums and trigger re-ingestion.

---

## Module Structure

```
rag-core/src/
  extract.rs       # FormatExtractor trait, ExtractorRegistry, FileType,
                   #   ExtractionResult, checksum(),
                   #   PdfExtractor (placeholder), MarkdownExtractor, TextExtractor
  embed.rs         # EmbedService trait, OpenAiEmbedder, MockEmbedder, AnyEmbedder
  bm25.rs          # SparseVector, Bm25Config, Bm25Embedder
  config.rs        # AppConfig (extended with embed + bm25 params)
  stores/          # (Phase 2, unchanged)
  tenant.rs        # (Phase 2, unchanged)
  lib.rs           # pub mod extract, embed, bm25, config, stores, tenant
```

---

## Key Types

### Extraction

| Type | Purpose |
|------|---------|
| `FileType` | Enum: `Pdf`, `Markdown`, `Text` |
| `ExtractionResult` | Normalized output: `text: String` |
| `FormatExtractor` | Async trait: `supported_types()` + `extract(&[u8]) -> BoxFuture<Result<ExtractionResult>>` |
| `ExtractorRegistry` | `Vec<Box<dyn FormatExtractor>>` + `HashMap<FileType, usize>`, caller passes explicit `FileType` |

### Embedding

| Type | Purpose |
|------|---------|
| `EmbedService` | Async trait: `dim()` + `embed_batch(&[String]) -> BoxFuture<Result<Vec<Vec<f32>>>>` |
| `OpenAiEmbedder` | async-openai wrapper with batching (token + item limits) and retry |
| `MockEmbedder` | Deterministic SHA-256-seeded vectors in [-1.0, 1.0] |
| `AnyEmbedder` | Enum dispatch, constructed from AppConfig |

### BM25

| Type | Purpose |
|------|---------|
| `SparseVector` | `indices: Vec<u32>` (sorted) + `values: Vec<f32>` |
| `Bm25Config` | k1 (1.2), b (0.75), avgdl (300.0) |
| `Bm25Embedder` | `embed_document(text)` + `embed_query(text, query_b)` |

---

## Configuration

All params in `config/app.toml`, overridable by environment variables:

```toml
# Embedding
embedding_model = "text-embedding-3-small"
embedder = "openai"          # "openai" or "mock"
embed_timeout_secs = 30
embed_max_retries = 3
embed_retry_backoff_ms = 500
embed_max_batch_tokens = 8192
embed_max_batch_size = 32

# BM25
bm25_avgdl = 300.0
bm25_k1 = 1.2
bm25_b = 0.75
bm25_query_b = 0.3
```

---

## Divergences from projectAlpha

| Area | projectAlpha | apex | Rationale |
|------|-------------|------|-----------|
| Extractor interface | Async, takes `&Path` | Async, takes `&[u8]` | Testability; caller handles I/O |
| Embed trait | `embed_batch` + `embed_batch_with_retry` | `embed_batch` only | Retry is an implementation detail |
| Circuit breaker | Yes (5 failures open) | No | Hardening concern, not MVP |
| BM25 tokenizer | Full pipeline (CJK, stopwords, stemming, boosting) | Lowercase + non-alphanumeric split | Core only at MVP, layerable |
| OCR | Tesseract fallback | Deferred | Phase 13 |
| PDF heading detection | Font-based | Deferred (placeholder extractor) | Arrives with full pdfium wiring |
