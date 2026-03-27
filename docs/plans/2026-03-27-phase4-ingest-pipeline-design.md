# Phase 4: Ingest Pipeline — Design Spec

**Date:** 2026-03-27
**Status:** Approved
**Dependencies:** Phase 1 (chunking), Phase 2 (storage), Phase 3 (extraction + embedding)

---

## Scope

Phase 4 adds `IngestService`, the end-to-end synchronous document ingestion pipeline
that orchestrates all Phase 1-3 primitives. It also adds sidecar metadata parsing and
directory discovery.

### In scope

- `Sidecar` schema with full projectAlpha field set, parsed from JSON with validation
- `IngestService` orchestrating: extract → checksum → chunk → embed → persist → cleanup → corpus stats
- Directory scanning with `(document, sidecar)` pair discovery
- Checksum-based dedup (skip unchanged, update metadata on early return)
- Stale chunk/vector cleanup after re-ingest
- Corpus stats update (non-fatal)
- Chunking strategy resolution: sidecar override > auto-detect > `"tokens"` fallback

### Out of scope (deferred)

- Async ingest queue / job tracking — Phase 12
- PDF extraction (pdfium placeholder) — follow-up to Phase 3
- Semaphore-bounded document concurrency — hardening/performance optimization
- OCR fallback — Phase 13
- BM25 advanced tokenization (stopwords, stemming, CJK, field boosting) — hardening

---

## Design Decisions

### 1. Full sidecar support from day one

Sidecars are important for per-document configuration (collection routing, chunking
strategy, metadata). The full projectAlpha schema is supported, with permissive
parsing (no `deny_unknown_fields`). Unknown fields are silently ignored
during deserialization — they do not round-trip through the typed structs.
This is acceptable at MVP; if round-tripping becomes needed, add a
`#[serde(flatten)] extra: HashMap<String, Value>` catch-all later.

### 2. Sequential batch processing

Batch ingest processes documents sequentially in MVP; internal embedding calls may
still batch requests. Semaphore-bounded document concurrency is deferred as a
hardening/performance optimization.

### 3. Structured step functions

`IngestService` is a single orchestration surface with private step functions and
intermediate structs (`PreparedDocument`, `ChunkedDocument`, `EmbeddedDocument`).
This keeps `ingest_file()` readable, gives test seams at phase boundaries, and
leaves a clean path to future per-document concurrency.

### 4. Write-before-delete persist ordering

Upsert new data first, then delete stale data. If cleanup fails, extra data remains
(no data loss). Re-running `ingest_file()` for the same document is idempotent via
upserts and stable chunk IDs.

### 5. Postgres/Qdrant failure rule

Postgres is authoritative for document/chunk metadata. Qdrant is authoritative for
vectors. If Postgres upsert succeeds and Qdrant upsert fails, the ingest fails and
retry reconciles by re-upserting vectors for the same stable chunk IDs.

### 6. Collection cache is best-effort

In-memory cache avoids repeated Qdrant API calls. Source of truth is always Qdrant.
On collection-not-found, evict from cache and retry once.

---

## Module Structure

```
rag-core/src/
  sidecar.rs     # Sidecar, DocumentInfo, SourceInfo, AclInfo, SecurityInfo,
                 #   ProvenanceInfo, IngestionConfig, ChunkingOverride
                 #   Sidecar::from_json(), Sidecar::validate()
  ingest.rs      # IngestService, IngestFileRequest, IngestDirectoryRequest,
                 #   IngestOutcome, IngestBatchOutcome, DocumentFailure,
                 #   PreparedDocument, ChunkedDocument, EmbeddedDocument
  config.rs      # AppConfig (extended with chunking_max_tokens, chunking_overlap_ratio)
  extract.rs     # (Phase 3, unchanged)
  embed.rs       # (Phase 3, unchanged)
  bm25.rs        # (Phase 3, unchanged)
  stores/        # (Phase 2, unchanged)
  tenant.rs      # (Phase 2, unchanged)
  lib.rs         # pub mod sidecar, ingest added
```

---

## Sidecar Schema

### Parsing and validation

```rust
impl Sidecar {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let sidecar: Self = serde_json::from_slice(bytes)?;
        sidecar.validate()?;
        Ok(sidecar)
    }

    pub fn validate(&self) -> Result<()> {
        // schema_version == 1
        // required strings non-empty (document.title, document.category, language, etc.)
        // chunking.overlap_ratio in 0.0..1.0 if present
        // chunking.max_tokens > 0 if present
    }
}
```

No `deny_unknown_fields`. Unknown fields are silently ignored (not preserved).

### Required fields

| Field | Type | Constraint |
|-------|------|-----------|
| `schema_version` | `u32` | Must be 1 |
| `document.title` | `String` | Non-empty |
| `document.category` | `String` | Non-empty |
| `source.url` | `String` | Non-empty |
| `source.domain` | `String` | Non-empty |
| `source.publisher` | `String` | Non-empty |
| `language` | `String` | Non-empty, ISO 639-1 |
| `tags` | `Vec<String>` | At least one |
| `acl.allow_roles` | `Vec<String>` | At least one |
| `security.classification` | `String` | Non-empty |
| `security.requires_evidence_pack` | `bool` | — |
| `provenance.retrieved_at` | `String` | Non-empty, ISO 8601 |
| `provenance.retrieved_by` | `String` | Non-empty |

### Optional fields

| Field | Type | Notes |
|-------|------|-------|
| `document.id` | `Option<String>` | Filename-stem fallback applied in ingest, not here |
| `document.version` | `Option<String>` | — |
| `document.authors` | `Option<Vec<String>>` | — |
| `document.summary` | `Option<String>` | — |
| `document.published_at` | `Option<String>` | ISO 8601 string at MVP |
| `document.refresh_cadence` | `Option<String>` | high/medium/low/static |
| `document.content_owner` | `Option<String>` | PII, must not appear in logs |
| `document.last_updated` | `Option<String>` | — |
| `security.compartments` | `Option<Vec<String>>` | — |
| `security.contains_pii` | `Option<bool>` | — |
| `security.export_control` | `Option<String>` | — |
| `source.collection` | `Option<String>` | — |
| `acl.deny_roles` | `Option<Vec<String>>` | — |
| `acl.notes` | `Option<String>` | — |
| `provenance.checksum_sha256` | `Option<String>` | — |
| `ingestion.collection` | `Option<String>` | Collection routing override |
| `ingestion.chunking.strategy` | `Option<String>` | — |
| `ingestion.chunking.max_tokens` | `Option<usize>` | Must be > 0 if present |
| `ingestion.chunking.overlap_ratio` | `Option<f32>` | Must be in 0.0..1.0 if present |

### Naming convention

`document.pdf` → `document.metadata.json` (same directory, matching projectAlpha).

---

## IngestService

### Service struct

```rust
pub struct IngestService {
    stores: Stores,
    extractors: ExtractorRegistry,
    embedder: AnyEmbedder,
    bm25: Bm25Embedder,
    default_collection: String,
    chunking_max_tokens: usize,
    chunking_overlap_ratio: f32,
    known_collections: Mutex<HashSet<String>>,
}
```

`default_collection` derived from `Stores::default_collection` (not duplicated from
config). `known_collections` uses `tokio::sync::Mutex` for interior mutability — the
public API takes `&self`, and the cache must be mutated on ensure/evict/retry.
`Mutex` (not `RwLock`) is sufficient since mutations are infrequent and contention is
negligible with sequential document processing.

### Public API

```rust
impl IngestService {
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self>;
    pub async fn ingest_file(&self, req: IngestFileRequest) -> Result<IngestOutcome>;
    pub async fn ingest_directory(&self, req: IngestDirectoryRequest) -> Result<IngestBatchOutcome>;
}
```

### Request and outcome types

```rust
pub struct IngestFileRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

pub struct IngestOutcome {
    pub document_id: String,
    pub collection: String,
    pub chunks_created: usize,
    pub skipped: bool,
}

pub struct IngestDirectoryRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

pub struct IngestBatchOutcome {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    pub failures: Vec<DocumentFailure>,
}

pub struct DocumentFailure {
    pub path: PathBuf,
    pub error: String,
}
```

### Pipeline flow (ingest_file)

```
1. Read file bytes
   - Validate extension maps to supported FileType; fail cleanly if unsupported
   - Load sidecar if document.metadata.json exists alongside the file

2. extract_and_checksum() → PreparedDocument
   - Extract text via ExtractorRegistry
   - Compute SHA-256 checksum
   - Resolve document_id: sidecar.document.id > filename stem
   - Look up existing checksum in Postgres (requires document_id, resolved first)
   - If checksum matches:
     - Update sidecar-derived metadata fields in Postgres if changed
     - Return early with skipped=true
   - Returns: PreparedDocument { document_id, tenant, collection, sidecar, text, checksum }

3. chunk_text() → ChunkedDocument
   - Resolve chunking strategy: sidecar.ingestion.chunking.strategy > auto-detect > "tokens"
   - Resolve max_tokens: sidecar > config default (chunking_max_tokens)
   - Resolve overlap_ratio: sidecar > config default (chunking_overlap_ratio)
   - Auto-detect: if text contains markdown headings, use "markdown"; else "tokens"
   - Call rag-chunking::chunk_text_with_strategy_sectioned()
   - Returns: ChunkedDocument { ...prepared fields, chunks: Vec<ChunkWithSection> }

4. embed_chunks() → EmbeddedDocument
   - Dense: embedder.embed_batch(chunk texts)
   - Sparse: bm25.embed_document(chunk text) for each chunk
   - Returns: EmbeddedDocument { ...chunked fields, dense_vectors, sparse_vectors }

5. persist()
   - Resolve collection: collection_override > sidecar.ingestion.collection > default
   - Ensure Qdrant collection exists (check cache, create if missing, cache result)
   - Upsert document row in Postgres (with sidecar metadata)
   - Upsert chunks in Postgres
   - Upsert vectors + payloads in Qdrant
   - Delete stale chunks in Postgres (chunk_index >= new chunk count)
   - Delete stale points in Qdrant (matching logic)
   - Failure rule: if Postgres succeeds but Qdrant fails, ingest fails.
     Retry reconciles by re-upserting vectors for the same stable chunk IDs.

6. update_corpus_stats()
   - Non-fatal: log warning on failure, do not fail the ingest
```

### Directory discovery

Walk directory, discover `(document_path, Option<sidecar_path>)` pairs:
- Skip files ending in `.metadata.json` as source documents
- Skip hidden files (starting with `.`)
- Skip files with unsupported extensions (not in FileType::from_extension)
- For each document file, check if `stem.metadata.json` exists alongside it
- Process pairs sequentially, continue on per-document error

---

## Configuration

### New fields in AppConfig

```toml
# Chunking defaults (overridable per-document via sidecar)
chunking_max_tokens = 600
chunking_overlap_ratio = 0.15
```

Validated at config load:
- `chunking_max_tokens > 0`
- `0.0 <= chunking_overlap_ratio < 1.0`

### Resolution precedence

| Setting | Order |
|---------|-------|
| Collection | API `collection_override` > `sidecar.ingestion.collection` > `Stores::default_collection` |
| Document ID | `sidecar.document.id` > filename stem |
| Chunking strategy | `sidecar.ingestion.chunking.strategy` > auto-detect > `"tokens"` |
| max_tokens | `sidecar.ingestion.chunking.max_tokens` > `config.chunking_max_tokens` |
| overlap_ratio | `sidecar.ingestion.chunking.overlap_ratio` > `config.chunking_overlap_ratio` |

---

## Error Handling

| Scope | Behavior |
|-------|----------|
| Single file: any step fails | `ingest_file()` returns `Err` with context |
| Batch: one file fails | Captured as `DocumentFailure`, batch continues |
| Corpus stats update fails | Log warning, ingest still succeeds |
| Stale cleanup fails | Ingest still succeeds (extra data, no data loss) |
| Qdrant collection-not-found | Evict from cache, retry once |
| Postgres succeeds, Qdrant fails | Ingest fails; retry re-upserts vectors for same stable chunk IDs |

---

## Testing

### Unit tests (no DB/Qdrant)

- Sidecar parsing: valid JSON, missing required fields, invalid overlap_ratio, invalid max_tokens, schema_version != 1
- Document discovery: finds pairs, skips hidden files, skips unsupported extensions, skips `.metadata.json` as source
- Chunking strategy resolution: sidecar override > auto-detect (markdown headings) > `"tokens"`
- Collection resolution: API override > sidecar > default
- Document ID resolution: sidecar > filename stem

### Integration tests (require `just up`)

- Ingest directory of test fixtures (MD + TXT with sidecars) → verify documents + chunks in Postgres, points in Qdrant
- Re-ingest same files → checksum skip, no new chunks
- Checksum-skip path still updates metadata when sidecar-derived metadata changes
- Modify a file → chunks updated, stale chunks cleaned
- Ingest with sidecar collection override → routed to correct Qdrant collection
- Collection-cache retry path: evicts and retries on simulated not-found

PDF integration tests deferred (PdfExtractor is still a placeholder).

---

## Divergences from projectAlpha

| Area | projectAlpha | apex | Rationale |
|------|-------------|------|-----------|
| Concurrency | Semaphore, default 4 | Sequential | MVP simplicity; extension point preserved |
| Sidecar BM25 overrides | Full (stopwords, stemming, field boost) | Not wired | Core BM25 only at MVP |
| Failure classification | `FailureStage` enum from error chain | `DocumentFailure { path, error }` | Simpler for MVP |
| Ingest queue | In-memory or Redis | None | Phase 12 |
| OCR | Tesseract fallback | Deferred | Phase 13 |
| Embedding cache | Tenant-scoped with TTL | None | Hardening |
| Metadata sidecar | Full schema | Full schema (matched) | Important for per-doc config |
