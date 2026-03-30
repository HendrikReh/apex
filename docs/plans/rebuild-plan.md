# Apex Accelerator — Rebuild Plan

**Version:** 1.2
**Date:** 2026-03-30
**Source of truth:** projectAlpha (`/Users/hendrik/Developer/projectAlpha`)
**Target repo:** apex (`/Users/hendrik/Developer/apex`)

---

## 0. Rebuild Philosophy

projectAlpha is the behavioral reference — read it to understand *what* the system does and *why*. However, this is a **clean-room rebuild**, not a port. When reimplementing, evaluate each behavior critically:

- **Match projectAlpha** when the behavior is correct and well-designed.
- **Diverge from projectAlpha** when a correctness bug, design flaw, or unnecessary complexity is identified. Fix it in apex rather than replicating the problem.
- **Document every intentional divergence** with a brief rationale, so the delta between the two codebases is traceable.

### Divergence Criteria

A divergence is warranted when:

1. **Correctness bug** — projectAlpha produces wrong output (e.g., overlap compounding, normalization applied to wrong code path, budget accounting that ignores overhead).
2. **Semantic mismatch** — the code doesn't match its documented intent (e.g., a "packing" chunker that doesn't actually pack).
3. **Unnecessary complexity** — projectAlpha has workarounds or compatibility shims that the clean-room rebuild doesn't need.

A divergence is **not** warranted for subjective style preferences or speculative improvements without a concrete correctness argument.

### Tracking Divergences

Each intentional divergence is recorded as a beads issue (`bd create --type=task --title="divergence: ..."`) so the list stays queryable alongside the rest of the work.

---

## 1. Current Behavior to Preserve

These are the observable behaviors of the existing system that the rebuild must eventually match (or intentionally improve upon — see Section 0). Ordered by user-facing importance.

### 1.1 Core RAG Pipeline

| Behavior | Description |
|---|---|
| **Document ingestion** | Accept files (PDF, MD, TXT at MVP; 15 formats at parity) via API or directory scan. Extract text, compute checksum (skip unchanged), chunk, generate dense + sparse vectors, store in Postgres (metadata) + Qdrant (vectors). |
| **Hybrid retrieval** | Parallel dense search (Qdrant HNSW) + sparse BM25 search (Qdrant sparse vectors) → RRF fusion (k=60) → ranked results. |
| **Context assembly** | Take ranked chunks, deduplicate (by chunk_id/doc_id/semantic/none), enforce token budget (default 8000), optionally include citations, produce assembled context string. |
| **Chat** | Accept user query + collection + conversation history. Resolve prompt template, assemble context from retrieval, call LLM (OpenAI), return response with source references. |
| **Multi-turn conversation** | Track conversation turns per tenant in Postgres. Include prior turns as context for follow-up queries. |

### 1.2 API Surface

| Behavior | Description |
|---|---|
| **REST API** | Axum server on configurable bind address (default `0.0.0.0:8080`). ~50 endpoints across health, ingest, search, chat, agents, runs, admin, prompts, evidence, SBOM. |
| **Health/readiness** | `/health` (liveness), `/readiness` (Postgres + Qdrant connectivity check). |
| **Tenant isolation** | `X-Tenant` header on every protected request. All queries filter by tenant. Default tenant: `"default"`. |
| **Auth modes** | `none` (dev), `api-key` (Bearer/header), `oidc` (JWT validation). Pluggable middleware. |
| **RBAC** | 5 roles: viewer, editor, agent_operator, admin, platform_operator. Enforced per-endpoint. |
| **Rate limiting** | Global (RPS + burst + concurrency) and per-tenant limits. |
| **Request correlation** | `X-Request-Id` header echoed in responses, threaded through logs. |

### 1.3 CLI

| Behavior | Description |
|---|---|
| **CLI client** | Calls server API over HTTP via `rag-client`. Subcommands: `chat`, `ingest`, `collections`, `refresh`, `agents`, `runs`, `reindex-bm25`, `reembed`, `api-key`, `keys`, `evidence`, `prompts`, `sbom`. |
| **Ingest** | `rag-cli ingest --dir <path> --collection <name>` — sends files to server for ingestion. Supports dry-run. |
| **Chat** | `rag-cli chat --query "..." --collection <name>` — interactive query with optional agent mode. |

### 1.4 Agent Orchestration

| Behavior | Description |
|---|---|
| **Graph-based execution** | YAML-defined agent specs → DAG of tasks (classify, search, summarize, evidence map, policy check, etc.) with conditional edges. |
| **Search tools** | Semantic, sparse, hybrid, FTS — all pluggable into agent graphs. |
| **ReAct loop** | Think → Act → Observe → Evaluate cycle with configurable stop conditions (max iterations, confidence threshold). |
| **Checkpoints** | Human-in-the-loop approval/rejection points. Run lifecycle: pending → running → awaiting_approval → approved/rejected → completed/failed. |
| **Timeline** | Event log per run (decisions, tool calls, intermediate results). Exportable as evidence pack. |

### 1.5 Storage

| Behavior | Description |
|---|---|
| **Postgres** | 15 migrations. Tables: documents, chunks, corpus_stats, conversation_turns, hallucination_audit_logs, prompts, prompt_revisions, agents, agent_runs, suggestion_feedback, etc. |
| **Qdrant** | Dense vectors (OpenAI `text-embedding-3-small`, 1536 dims) + BM25 sparse vectors per chunk. Collections created on demand. |
| **Checksum dedup** | SHA-256 of document content. Re-ingest skips unchanged documents. |
| **Document identity** | PK is `(tenant, document_id)`. Collection is routing metadata, not identity (ADR-002). |

### 1.6 Platform Concerns (post-MVP)

| Behavior | Description |
|---|---|
| **Async ingest** | `POST /ingest/async` returns job ID. `GET /ingest/jobs/:id` polls status. Backends: in-memory, Redis. |
| **Refresh scheduler** | Periodic stale-document detection, notification, deprecation, health reports. |
| **Prompt catalog** | DB-backed prompt definitions with revision/publish/unpublish/rollback lifecycle + filesystem fallback. |
| **Evidence packs** | ZIP export of agent run artifacts with Ed25519 digital signatures, manifest, checksums. |
| **Notifications** | SMTP email via lettre, Handlebars templates, concurrent sending. |
| **SBOM** | CycloneDX generation at compile time, runtime enrichment with infrastructure versions, signing. |
| **License enforcement** | PASETO v4.public tokens, self-audit loop, state machine (Valid → Grace → Expired), middleware guard. |
| **Guardrails** | Prompt injection detection, PII redaction, SQL safety, SSRF prevention, egress filtering. |
| **Observability** | OpenTelemetry traces, Prometheus metrics, Langfuse LLM logging. |
| **Binary hardening** | Obfuscated strings (`obfuscate-macros`), stripped/LTO release builds, Ed25519 binary signing. |

---

## 2. Target Workspace Shape

```
apex/
├── Cargo.toml                    # Workspace root
├── Justfile                      # Task runner (single file, all recipes)
├── config/
│   ├── app.toml                  # Non-secret configuration
│   ├── agents/                   # Agent YAML specs (post-MVP)
│   └── prompts/                  # Prompt templates (file-based for MVP)
├── .env.example                  # Secrets template
├── docker-compose.yml            # Postgres + Qdrant
├── crates/
│   ├── rag-core/                 # Core library: extraction, ingest, embed, retrieval, context, stores
│   │   ├── src/
│   │   └── migrations/           # sqlx migrations
│   ├── rag-chunking/             # Chunking strategies (pure, no IO)
│   │   └── src/
│   ├── rag-server/               # Axum REST API
│   │   └── src/
│   ├── rag-client/               # HTTP client (TenantApiClient)
│   │   └── src/
│   ├── rag-cli/                  # CLI binary (clap)
│   │   └── src/
│   ├── agent-core/               # Graph-flow agent orchestration (post-MVP)
│   │   └── src/
│   ├── rag-evidence/             # Evidence pack signing (post-MVP)
│   │   └── src/
│   ├── rag-license/              # License enforcement (deferred hardening)
│   │   └── src/
│   ├── rag-notifications/        # SMTP notifications (post-MVP)
│   │   └── src/
│   ├── rag-sbom/                 # SBOM generation (deferred hardening)
│   │   └── src/
│   ├── obfuscate-macros/         # String obfuscation (deferred hardening)
│   │   └── src/
│   └── test-support/             # spawn_app() for ephemeral test servers
│       └── src/
├── docs/
│   ├── plans/                    # This plan
│   └── openapi.yaml              # API spec (built incrementally)
├── .github/
│   └── workflows/
│       └── ci.yml
└── rustfmt.toml
```

### Crate Boundary Rules

| Crate | Type | May depend on | May NOT depend on |
|---|---|---|---|
| `rag-chunking` | lib | None (pure logic + tiktoken-rs) | Any other workspace crate |
| `rag-core` | lib | `rag-chunking` | `rag-server`, `rag-cli`, `agent-core` |
| `agent-core` | lib | `rag-core` | `rag-server`, `rag-cli` |
| `rag-server` | bin | `rag-core`, `agent-core`, `rag-evidence`, `rag-notifications`, `rag-sbom` | `rag-cli` |
| `rag-client` | lib | None (reqwest only) | Any other workspace crate |
| `rag-cli` | bin | `rag-client`, `rag-evidence` | `rag-core`, `rag-server` |
| `rag-evidence` | lib | None (crypto only) | `rag-core` |
| `rag-license` | lib | None (crypto + metrics) | `rag-core` |
| `rag-notifications` | lib | None (lettre + handlebars) | `rag-core` |
| `rag-sbom` | lib | None | `rag-core` |
| `test-support` | lib (dev) | `axum`, `tokio` | Any workspace crate |

### Key Differences from projectAlpha

1. **No `tools/` directory** — desktop apps and standalone tools are out of scope.
2. **No `web-client/`** — frontend out of scope.
3. **No `rag-eval/`** — evaluation ported later as a standalone effort.
4. **No `rag-preview/`** — chunking preview tool deferred.
5. **Migrations rebuilt from scratch** — clean schema, informed by the existing 15 migrations but consolidated where possible.
6. **Config simplified** — `config/app.toml` starts lean, grows as features land.

---

## 3. Phased Rebuild Plan

### Phase 0: Scaffold and Infrastructure

**Goal:** Buildable, testable, CI-passing empty workspace.

**Scope:**
- Cargo workspace with all planned crate shells (lib.rs / main.rs that compile)
- `rustfmt.toml` (max_width=100, edition 2021, Unix newlines, reorder_imports)
- `clippy.toml` (disallow `.unwrap()`, `.expect()` on Result/Option)
- `docker-compose.yml` for Postgres 16 + Qdrant
- `.env.example` with `DATABASE_URL`, `QDRANT_URL`, `OPENAI_API_KEY`
- `config/app.toml` skeleton (bind_addr, qdrant_url, database_url, embedder, default_collection)
- `Justfile` with: `fmt`, `clippy`, `test`, `up` (docker), `run-server`, `run-server-mock`
- `test-support` crate with `spawn_app(Router) -> TestServer`
- GitHub Actions CI: fmt → clippy → cargo test
- `.env.example`, `.gitignore`
- CLAUDE.md for the new repo

**Behaviors reimplemented:** None — this is scaffolding.

**Simplified vs matched:**
- Justfile starts minimal (6-8 recipes vs 30+ in projectAlpha)
- Config starts with ~20 settings vs 100+
- CI runs cargo directly (no boundary checks, no SBOM, no docs checks yet)

**Key deliverables:**
- [x] `cargo build --workspace` passes
- [x] `cargo test --workspace` passes (zero tests, zero failures)
- [x] `just fmt && just clippy` pass
- [x] `just up` starts Postgres + Qdrant
- [x] CI green on push

**Dependencies:** None.

**Risks:**
- Qdrant version mismatch between docker image and `qdrant-client` crate version. Pin both.
- sqlx compile-time checks need `DATABASE_URL` even for `cargo check`. Set up `.env` early.

**Exit criteria:** `cargo build --workspace && cargo clippy --workspace && cargo test --workspace` all pass. CI green.

**Status: COMPLETE** ✓

---

### Phase 1: Chunking Library

**Goal:** Pure text chunking library with no IO dependencies.

**Scope:**
- `rag-chunking` crate: `ChunkingStrategy` enum (all 9 variants), `SectionInfo`, `ChunkWithSection`
- MVP strategies implemented: `Tokens`, `Markdown`, `Sentences` (the three most used)
- `chunk_text_with_strategy()` dispatcher
- `select_strategy()` auto-detection
- Boundary normalization (`normalize_chunk_boundaries`, `ensure_word_boundary_start`)
- CJK/multilingual support (`is_cjk_char`)
- Remaining 6 strategies: `PageTokens`, `Semantic`, `Chars`, `Paragraphs`, `Pages`, `RecursiveChars`

**Behaviors reimplemented:**
- All 9 chunking strategies from projectAlpha
- Boundary normalization (non-ASCII, CJK, Arabic, Hebrew, Devanagari, Thai)
- Strategy auto-selection based on content heuristics

**Simplified vs matched:**
- **Reference baseline:** projectAlpha chunking output informs expected behavior, but apex diverges where correctness bugs are found (see Section 0)
- **Kept from projectAlpha:** boundary normalization applied unconditionally in the dispatcher for all strategies — necessary because even "natural boundary" strategies (sentences, paragraphs, markdown) delegate to the token chunker for oversized inputs or overlap, producing mid-word starts
- **Divergence-aware:** overlap in sentence/recursive chunkers uses original chunks, not compounding overlapped output (fixes a projectAlpha bug)
- **Divergence-aware:** paragraph chunker packs small paragraphs into shared output chunks (matches documented intent that projectAlpha's implementation missed)
- **Divergence-aware:** page-aware chunkers reserve label token budget before splitting content (fixes token budget overrun in projectAlpha)

**Key deliverables:**
- [x] `ChunkingStrategy` enum with `FromStr`, `as_str`, `all()`
- [x] All 9 strategy functions
- [x] Boundary normalization
- [x] Strategy auto-selection
- [x] Unit tests for each strategy (port test cases from projectAlpha)
- [x] Property: no empty chunks, no chunks exceeding max_tokens by more than overlap

**Dependencies:** Phase 0.

**Risks:**
- `tiktoken-rs` tokenizer initialization is slow on first call (lazy_static or once_cell). Match projectAlpha's caching pattern.
- Semantic chunking depends on `async-openai` — can be stubbed at this phase if needed.

**Exit criteria:** All 9 strategies produce correct output. Boundary normalization handles multilingual text. 100% unit test coverage of public API.

**Status: COMPLETE** ✓ — 114 unit tests passing.

---

### Phase 2: Core Storage Layer

**Goal:** Postgres + Qdrant abstractions with tenant-scoped operations.

**Scope:**
- `rag-core` crate begins
- `config` module: `AppConfig` loaded from `config/app.toml` + env overrides
- `tenant` module: `TenantId` newtype
- `stores` module: Postgres pool (`sqlx::PgPool`) + Qdrant client, wrapped behind trait abstractions
- sqlx migrations (consolidated from projectAlpha's 15):
  - `0001_documents_and_chunks.sql` — documents, chunks, corpus_stats tables
  - `0002_conversations.sql` — conversation_turns table
- Document CRUD: insert, update, get_by_id, delete (all tenant-scoped)
- Chunk CRUD: batch insert, get_by_document, delete_by_document
- Qdrant operations: create_collection (dense + sparse), upsert_points, delete_points, search_dense, search_sparse
- Corpus stats: update avgdl, get avgdl
- Document identity semantics (ADR-002): PK on `(tenant, document_id)`

**Behaviors reimplemented:**
- Tenant-scoped Postgres CRUD for documents and chunks
- Qdrant collection management and point operations
- Corpus statistics for BM25
- Document identity: `(tenant, id)` is identity, collection is routing metadata

**Simplified vs matched:**
- **Simplified:** 2 migrations instead of 15. Consolidate the schema — no need to replay the historical evolution.
- **Matched (or improved):** document identity semantics (ADR-002)
- **Matched (or improved):** tenant isolation on every query
- **Deferred:** audit columns (created_at/updated_at/updated_by) — add when needed
- **Deferred:** prompt, agent, suggestion_feedback, hallucination tables — later phases

**Key deliverables:**
- [x] `AppConfig` with toml + env loading
- [x] `TenantId` newtype with `Display`, `FromStr`, header extraction
- [x] `Stores` struct (PgPool + QdrantClient)
- [x] Document + chunk Postgres operations with integration tests
- [x] Qdrant create/upsert/delete/search operations with integration tests
- [x] 2 migrations that create a clean schema

**Dependencies:** Phase 0. Docker services (Postgres + Qdrant) running.

**Risks:**
- sqlx compile-time verification requires a running Postgres with migrations applied. CI needs a Postgres service or `sqlx-data.json` offline mode.
- Qdrant gRPC client version must match the Docker image version exactly.

**Exit criteria:** Integration tests pass against real Postgres + Qdrant. Documents can be stored and retrieved by tenant + id. Qdrant collections can be created and searched.

**Status: COMPLETE** ✓ — 2 migrations (documents/chunks/corpus_stats + conversations).

---

### Phase 3: Extraction and Embeddings

**Goal:** Extract text from PDF/MD/TXT. Generate dense + sparse vectors.

**Scope:**
- `extract` module: `ExtractorRegistry` with extractors for PDF (pdfium), Markdown (passthrough), plain text (passthrough)
- `embed` module: `EmbedService` trait with two backends:
  - `OpenAiEmbedder` (async-openai, `text-embedding-3-small`)
  - `MockEmbedder` (deterministic fake vectors for testing)
- `bm25` module: sparse vector generation (term frequencies, IDF from corpus stats)
- `bm25_tokenizer` module: tokenization for BM25 (lowercasing, stemming, stopwords)
- Checksum computation (SHA-256 of document content)

**Behaviors reimplemented:**
- PDF text extraction via pdfium
- Markdown/TXT passthrough extraction
- OpenAI embedding generation with batching, timeout, retry
- Mock embedder for testing (no API key needed)
- BM25 sparse vector generation
- Document checksum for deduplication

**Simplified vs matched:**
- **Simplified:** 3 extractors instead of 15. ExtractorRegistry designed for easy addition of more.
- **Matched (or improved):** embedding dimensions (1536 for text-embedding-3-small)
- **Matched (or improved):** BM25 tokenization and sparse vector format (must produce compatible Qdrant sparse vectors)
- **Matched (or improved):** checksum-based skip logic
- **Deferred:** ~~OCR (Tesseract),~~ DOCX/PPTX/XLSX, HTML, CSV, JSON, YAML, XML, code, email, ZIP
- **Shipped beyond plan:** Tesseract OCR fallback for scanned PDF pages (PR #11)

**Key deliverables:**
- [x] `ExtractorRegistry` with PDF, MD, TXT extractors
- [x] `EmbedService` trait + OpenAI + Mock implementations
- [x] Batch embedding with configurable max_batch_tokens and max_batch_size
- [x] Retry with exponential backoff on transient OpenAI errors
- [x] BM25 sparse vector generation
- [x] SHA-256 checksum computation
- [x] Unit tests for each extractor
- [x] Integration test: embed real text with mock embedder, verify vector dimensions
- [x] Tesseract OCR fallback for scanned PDF pages (beyond original scope)

**Dependencies:** Phase 2 (stores for corpus stats used by BM25).

**Risks:**
- pdfium native library must be vendored or downloaded. Need a clear setup path in CLAUDE.md/README.
- OpenAI rate limits during testing — mock embedder is essential for CI.

**Exit criteria:** Can extract text from a PDF, chunk it (Phase 1), embed it (dense + sparse), and the vectors have correct dimensions. Mock embedder works without API key.

**Status: COMPLETE** ✓ — Plus OCR fallback shipped ahead of Phase 13.

---

### Phase 4: Ingest Pipeline

**Goal:** End-to-end synchronous document ingestion.

**Scope:**
- `ingest` module: `IngestService` orchestrating the full pipeline:
  1. Accept file path(s) + collection + tenant
  2. Extract text via `ExtractorRegistry`
  3. Compute checksum, skip if unchanged
  4. Chunk text via `rag-chunking`
  5. Generate dense embeddings via `EmbedService`
  6. Generate BM25 sparse vectors
  7. Upsert document metadata to Postgres
  8. Upsert chunk vectors to Qdrant
  9. Clean up stale chunks (deleted from source)
  10. Update corpus stats (avgdl)
- Directory scanning: walk a directory, filter by allowed extensions, ingest each file
- Metadata sidecar support (optional `metadata.json` next to document)

**Behaviors reimplemented:**
- Full ingest pipeline from file to stored vectors
- Checksum-based deduplication (skip unchanged)
- Stale chunk cleanup on re-ingest
- Corpus stats update after batch ingest
- Directory walking with extension filtering

**Simplified vs matched:**
- **Simplified:** synchronous only (no async queue, no job tracking)
- **Simplified:** no OCR, no archive extraction, no email parsing
- **Matched (or improved):** the 10-step pipeline sequence (write-before-delete for safety)
- **Matched (or improved):** document identity semantics on re-ingest

**Key deliverables:**
- [x] `IngestService` with full pipeline
- [x] Directory scan with extension filtering
- [x] Checksum dedup (skip unchanged documents)
- [x] Stale chunk cleanup (no-op — stable UUIDs handle overwrites; full cleanup deferred to Phase 12)
- [x] Corpus stats update
- [x] Integration test: ingest a directory of test files, verify documents + chunks in Postgres, points in Qdrant
- [x] Test: re-ingest same files → no changes (checksum skip)
- [ ] Integration test: modify a file → chunks updated in place (stable UUID overwrite); explicit old-chunk cleanup path is not applicable in the current design
- [x] Sidecar metadata support (beyond original scope)
- [x] Dry-run support (beyond original scope)

**Dependencies:** Phase 1 (chunking), Phase 2 (stores), Phase 3 (extraction + embedding).

**Risks:**
- Ingest of large PDFs may be slow with synchronous embedding. Acceptable for MVP; async queue comes later.
- Stale chunk cleanup must handle partial failures gracefully (don't delete new chunks if Qdrant upsert failed).

**Exit criteria:** Can ingest a directory of PDF/MD/TXT files. Documents and chunks persisted. Re-ingest is idempotent. Stale chunks cleaned up.

**Status: COMPLETE** ✓ — Stale chunk cleanup is a no-op (stable UUIDs overwrite in place); modify-file integration test not yet written.

---

### Phase 5: Retrieval and Context Assembly

**Goal:** Hybrid search with context assembly.

**Scope:**
- `retrieval` module: `RetrievalService`
  - Dense search (Qdrant HNSW)
  - Sparse BM25 search (Qdrant sparse vectors)
  - RRF fusion (configurable k, default 60)
  - Tenant + collection filtering on all searches
- `context` module: `ContextBuilder`
  - Take ranked chunks, enforce token budget (default 8000 tokens)
  - Deduplication modes: chunk_id, doc_id, none (semantic deferred)
  - Citation inclusion (chunk source references)
  - Output: assembled context string ready for LLM prompt

**Behaviors reimplemented:**
- Parallel dense + sparse search
- RRF fusion
- Token-budget-aware context assembly
- Chunk deduplication
- Citation generation

**Simplified vs matched:**
- **Simplified:** no reranking (cross-encoder), no MMR diversity sampling — deferred to post-MVP
- **Simplified:** 3 dedupe modes instead of 4 (semantic deferred)
- **Matched (or improved):** RRF formula and default parameters
- **Matched (or improved):** token budget enforcement
- **Matched (or improved):** search result format (chunk text + score + document metadata)

**Key deliverables:**
- [x] `RetrievalService` with dense, sparse, and hybrid search
- [x] RRF fusion with configurable k
- [x] `ContextBuilder` with token budget, dedup, citations
- [x] Integration test: ingest documents → hybrid search → verify results ranked correctly
- [x] Integration test: context assembly respects token budget
- [x] Test: tenant isolation — tenant A cannot see tenant B's documents

**Dependencies:** Phase 4 (documents must be ingested to search them).

**Risks:**
- BM25 quality depends on correct avgdl and term frequency statistics. Verify against projectAlpha's output for the same corpus.
- RRF fusion parameter sensitivity — use the same defaults (k=60, dense_top_k=20, sparse_top_k=20).

**Exit criteria:** Hybrid search returns relevant results. Context assembly stays within token budget. Tenant isolation verified.

**Status: COMPLETE** ✓

---

### Phase 6: Chat Service

**Goal:** LLM-powered chat with context-augmented responses.

**Scope:**
- `chat` module: `ChatService`
  - Accept query + collection + optional conversation history
  - Retrieve context via `RetrievalService` + `ContextBuilder`
  - Resolve prompt template (file-based for MVP: `config/prompts/`)
  - Call OpenAI chat completion (async-openai)
  - Return response with source references
- `messages` module: conversation turn storage in Postgres
- Migration `0003_conversation_tracking.sql` (if not already covered in Phase 2)
- Prompt template loading from filesystem (simple Handlebars or string interpolation)

**Behaviors reimplemented:**
- RAG chat: query → retrieve → assemble context → LLM → response
- Multi-turn conversation tracking
- File-based prompt resolution
- Source references in responses

**Simplified vs matched:**
- **Simplified:** file-based prompts instead of DB-backed catalog
- **Simplified:** no guardrails (injection detection, PII redaction) — deferred
- **Simplified:** no suggestion tracking or feedback endpoints
- **Matched (or improved):** chat request/response format (compatible with projectAlpha's API contract)
- **Matched (or improved):** conversation history inclusion in LLM context

**Key deliverables:**
- [x] `ChatService` with end-to-end RAG chat
- [x] Conversation turn persistence
- [x] File-based prompt template loading
- [x] Integration test: ingest → chat → verify response includes source references
- [x] Test: multi-turn conversation maintains history
- [x] Test: works with mock embedder (no OpenAI key needed for search; LLM call still needs key or mock)
- [x] Mock LLM backend for CI (beyond original scope)
- [x] Provider-agnostic LLM backend — OpenAI-compatible + Anthropic (beyond original scope)

**Dependencies:** Phase 5 (retrieval + context assembly).

**Risks:**
- LLM integration testing requires either a real OpenAI key or a mock LLM. Consider a `MockLlm` backend for CI.
- Prompt template format must be designed for forward compatibility with the later DB-backed catalog.

**Exit criteria:** Can ingest documents, ask a question, and get a context-grounded response with source references. Multi-turn conversation works.

**Status: COMPLETE** ✓

---

### Phase 7: HTTP Server (MVP API)

**Goal:** Axum REST API exposing the core pipeline.

**Scope:**
- `rag-server` crate:
  - Router with MVP endpoints:
    - `GET /health` — liveness
    - `GET /readiness` — Postgres + Qdrant check
    - `POST /ingest` — synchronous ingest
    - `POST /search` — hybrid search
    - `POST /chat` — RAG chat
    - `GET /collections/:collection/stats` — collection statistics
  - Middleware stack (ordered):
    - Tenant extraction from `X-Tenant` header (default: `"default"`)
    - Request ID generation/propagation (`X-Request-Id`)
    - Body size limit (configurable, default 10MB)
    - Basic graceful shutdown (SIGTERM handler, drain in-flight)
  - `AppState` holding `Stores`, `IngestService`, `RetrievalService`, `ChatService`, config
  - JSON error responses (standardized format)
  - CORS configuration for localhost development

**Behaviors reimplemented:**
- Core API endpoints (6 of ~50)
- Tenant extraction middleware
- Request ID correlation
- Graceful shutdown (basic)
- Standardized error responses

**Simplified vs matched:**
- **Simplified:** 6 endpoints instead of ~50. Enough for the MVP user journey.
- **Simplified:** `auth_mode=none` only. Middleware slot exists but only tenant extraction is active.
- **Simplified:** no rate limiting, no RBAC enforcement, no license guard, no cancellation tokens
- **Matched (or improved):** endpoint paths and request/response JSON shapes (API compatibility)
- **Matched (or improved):** `X-Tenant` header semantics
- **Matched (or improved):** health/readiness response format

**Key deliverables:**
- [x] Axum router with 6 endpoints (8+ shipped: includes /ingest/upload, /search/dense, /search/sparse, /search/hybrid)
- [x] Tenant extraction middleware
- [x] Request ID middleware
- [x] Body size limit (10 MB default, 50 MB for upload)
- [x] Graceful shutdown (basic)
- [x] AppState initialization (config → stores → services)
- [x] `just run-server` and `just run-server-mock`
- [x] Integration tests using `test-support::spawn_app`
- [x] Smoke test: ingest via API → search via API → chat via API (end-to-end, ignored, requires `just up`)
- [ ] Smoke test: X-Tenant isolation through API

**Dependencies:** Phase 6 (all services wired up).

**Risks:**
- AppState initialization order matters (config → DB pool → Qdrant client → services). Get this right once.
- Axum 0.7 routing syntax (`:param` not `{param}`).

**Exit criteria:** Server starts, accepts requests, serves the MVP user journey end-to-end via HTTP. Integration tests pass against a real running server.

**Status: COMPLETE** ✓ — Shipped more endpoints than planned. End-to-end API journey smoke test exists; X-Tenant API isolation smoke test not yet written.

---

### Phase 8: HTTP Client and CLI

**Goal:** CLI tool that exercises the full MVP pipeline.

**Scope:**
- `rag-client` crate:
  - `TenantApiClient` wrapping `reqwest::Client`
  - Automatic `X-Tenant` header injection
  - Methods: `ingest()`, `search()`, `chat()`, `health()`, `readiness()`, `collection_stats()`
- `rag-cli` crate:
  - `clap` CLI with subcommands:
    - `chat --query "..." --collection <name>` — RAG chat
    - `ingest --dir <path> --collection <name> [--dry-run]` — directory ingestion
    - `collections --collection <name>` — show collection stats
  - Reads server URL from `--server` flag or `RAG_SERVER_URL` env var
  - Reads tenant from `--tenant` flag or `RAG_TENANT` env var

**Behaviors reimplemented:**
- HTTP client with tenant header injection
- CLI ingest (directory walk → upload to server)
- CLI chat (query → display response with sources)
- CLI collection stats

**Simplified vs matched:**
- **Simplified:** 3 subcommands instead of 13. MVP-critical only.
- **Matched (or improved):** CLI output format for chat (response + sources)
- **Matched (or improved):** `TenantApiClient` header injection pattern
- **Deferred:** `refresh`, `agents`, `runs`, `reindex-bm25`, `reembed`, `api-key`, `keys`, `evidence`, `prompts`, `sbom` subcommands

**Key deliverables:**
- [x] `TenantApiClient` with MVP methods
- [x] `rag-cli` binary with 3 subcommands (4 shipped: chat, ingest, search, collection-stats)
- [ ] End-to-end test: start server → CLI ingest → CLI chat → verify response
- [x] `--dry-run` for ingest (list files that would be ingested)
- [x] Interactive chat mode (beyond original scope)
- [x] `--json` output flag (beyond original scope)
- [x] `ApiClient` trait for dependency injection in tests (beyond original scope)
- [x] 30+ unit tests with fake client implementations (beyond original scope)

**Dependencies:** Phase 7 (server must be running for CLI to work).

**Risks:**
- CLI ingest sends files over HTTP — large PDFs need streaming upload or chunked transfer. Match projectAlpha's approach.

**Exit criteria:** `rag-cli ingest --dir data/ --collection test` ingests files. `rag-cli chat --query "..." --collection test` returns a grounded response. The MVP user journey works end-to-end via CLI.

**Status: COMPLETE** ✓ — Shipped search subcommand and interactive chat beyond plan. E2E CLI test not yet written.

---

### **--- MVP COMPLETE ---**

At the end of Phase 8, the following user journey works:

```bash
just up                                              # Start Postgres + Qdrant
just run-server-mock                                 # Start server (mock embeddings)
rag-cli ingest --dir ./data/demo --collection demo  # Ingest PDF/MD/TXT files
rag-cli chat --query "What is X?" --collection demo  # Get context-grounded response
curl localhost:8080/search -d '{"query":"X","collection":"demo"}' -H 'X-Tenant: default'
```

---

### Phase 9: Agent Orchestration

**Goal:** Graph-based agent execution with search tools.

**Scope:**
- `agent-core` crate:
  - Evaluate `graph-flow` crate — keep if maintained, otherwise build minimal DAG runner
  - `AgentGraphConfig`, `build_agent_graph_from_spec`
  - Task registry: `QueryClassifierTask`, `SemanticSearchTool`, `SparseSearchTool`, `HybridSearchTool`, `FtsSearchTool`, `FallbackSearchTask`, `SummarizeTask`, `EvidenceMapTask`
  - `AgentRunConfig`, `AgentRunStatus`, `AgentRunResult`, `Timeline`
  - Checkpoint lifecycle: pending → running → awaiting_approval → approved/rejected → completed/failed
  - ReAct loop: Think → Act → Observe → Evaluate with stop conditions
- Agent YAML spec loading from `config/agents/`
- New migration: `0004_agents.sql` (agents, agent_versions, agent_runs tables)
- New server endpoints:
  - `GET/POST /agents` — list/upload
  - `GET/PUT/DELETE /agents/:id` — CRUD
  - `POST /agents/:id/execute` — run agent
  - `GET /agents/:id/graph` — visualize
  - `GET/POST /runs` — list runs
  - `POST /runs/:id/approve` and `/reject` — checkpoint decisions
  - `DELETE /runs/:id` — delete run
- New CLI subcommands: `agents list`, `agents graph`, `runs list`, `runs export`

**Behaviors reimplemented:**
- YAML-driven agent graph construction
- Task registry with all search tools
- Query classification (Structured/Unstructured/Mixed)
- ReAct reasoning loop
- Checkpoint approval/rejection
- Timeline event logging
- Agent CRUD API
- Run lifecycle management

**Simplified vs matched:**
- **Simplified:** no external tool execution (deferred)
- **Simplified:** no policy check, pricing rules, SQL allowlist tasks (domain-specific, added incrementally)
- **Matched (or improved):** agent spec YAML format (compatibility with existing specs)
- **Matched (or improved):** run lifecycle state machine
- **Matched (or improved):** checkpoint approval/rejection API

**Key deliverables:**
- [ ] Agent graph builder from YAML spec
- [ ] All search tool implementations
- [ ] ReAct loop with configurable stop conditions
- [ ] Checkpoint lifecycle
- [ ] Timeline event collection
- [ ] Agent CRUD endpoints
- [ ] Run management endpoints
- [ ] Integration test: upload agent spec → execute → approve checkpoint → completed
- [ ] Test: ReAct loop terminates at max iterations

**Dependencies:** Phase 8 (MVP complete). `rag-core` retrieval service for search tools.

**Risks:**
- `graph-flow` crate evaluation: if it's unmaintained or API-incompatible, building a minimal DAG runner is ~1-2 weeks of work.
- Agent spec YAML schema must be backward-compatible with existing specs in projectAlpha.

**Exit criteria:** Can upload an agent YAML spec, execute it, approve checkpoints, and get a completed run with timeline.

**Status: NOT STARTED** — `agent-core` crate is an empty stub.

---

### Phase 10: Additional Auth and RBAC

**Goal:** API key and OIDC auth modes with role-based access control.

**Scope:**
- `api-key` auth mode: Bearer token or `X-Api-Key` header validation
- `oidc` auth mode: JWT validation via JWKS endpoint, audience check, issuer validation
- RBAC middleware: enforce role requirements per endpoint
- Role extraction from JWT claims (oidc) or static config (api-key)
- Rate limiting middleware: global (RPS, burst, concurrency) + per-tenant
- New CLI subcommand: `api-key generate`
- New server endpoints: `/license/status` (placeholder)
- Config additions: `auth_mode`, `oidc_*`, `rate_limit_*`, `tenant_rate_limit_*`

**Behaviors reimplemented:**
- 3 auth modes (none, api-key, oidc)
- 5 RBAC roles with per-endpoint enforcement
- Rate limiting (global + per-tenant)
- API key generation utility

**Simplified vs matched:**
- **Matched (or improved):** auth header semantics, role hierarchy, rate limit behavior
- **Deferred:** JWKS cache TTL tuning, clock skew leeway (hardening)

**Key deliverables:**
- [ ] Auth middleware with pluggable mode
- [ ] API key validation
- [ ] OIDC JWT validation
- [ ] RBAC enforcement middleware
- [ ] Rate limiting middleware
- [ ] Integration tests for each auth mode
- [ ] Test: unauthorized request → 401
- [ ] Test: insufficient role → 403
- [ ] Test: rate limit exceeded → 429

**Dependencies:** Phase 7 (server middleware stack).

**Risks:**
- OIDC testing requires a local Keycloak or similar. Docker service addition.
- Rate limiter state management (in-memory vs Redis).

**Exit criteria:** All three auth modes work. RBAC correctly restricts endpoints. Rate limiting enforced.

**Status: PARTIAL** — Tenant extraction middleware exists. No API key, OIDC, RBAC, or rate limiting.

---

### Phase 11: Prompt Catalog

**Goal:** DB-backed prompt management with revision lifecycle.

**Scope:**
- New migration: `0005_prompts.sql` (prompt definitions, revisions, publications)
- `prompt_catalog` module: CRUD for prompt definitions
- `prompt_catalog_write` module: revision/publish/unpublish/rollback lifecycle
- `prompt_filesystem` module: filesystem fallback (existing `config/prompts/`)
- `prompt_registry` module: resolution chain (DB published → DB latest → filesystem)
- Chat service updated to resolve prompts through registry
- New server endpoints:
  - `GET/POST /prompts` — list/create
  - `GET /prompts/:name` — get definition
  - `POST /prompts/:name/revisions` — add revision
  - `POST /prompts/:name/publish` / `unpublish` / `rollback`
  - `GET /prompts/resolve` — resolve by name
- New CLI subcommands: `prompts list`, `prompts get`, `prompts create`, etc.

**Behaviors reimplemented:**
- Full prompt lifecycle (create → revise → publish → rollback)
- Resolution chain with filesystem fallback
- Chat service integration

**Simplified vs matched:**
- **Matched (or improved):** prompt resolution semantics
- **Matched (or improved):** API endpoints and response shapes

**Key deliverables:**
- [ ] Prompt CRUD + lifecycle
- [ ] Resolution chain
- [ ] Chat service integration
- [ ] Prompt API endpoints
- [ ] CLI subcommands
- [ ] Tests: full lifecycle (create → revise → publish → unpublish → rollback)
- [ ] Test: chat resolves prompts correctly

**Dependencies:** Phase 6 (chat service), Phase 7 (server).

**Risks:** Migration must not conflict with existing schema.

**Exit criteria:** Prompt lifecycle works end-to-end. Chat service resolves prompts through the registry.

**Status: NOT STARTED** — File-based prompt loading exists (Phase 6); no DB catalog.

---

### Phase 12: Async Ingest and Background Jobs

**Goal:** Non-blocking ingestion with job tracking. Background refresh scheduler.

**Scope:**
- `jobs` module: `JobQueue` trait with `InMemoryJobQueue` and `RedisJobQueue` backends
- New endpoints: `POST /ingest/async`, `GET /ingest/jobs/:job_id`
- Refresh scheduler: periodic stale document detection
- Admin endpoints:
  - `POST /admin/refresh/trigger`
  - `GET /admin/refresh/stale`
  - `GET /admin/refresh/status`
  - `POST /admin/refresh/deprecate` / `un-deprecate`
  - `POST /admin/reindex-bm25`
  - `POST /admin/reembed`
- New CLI subcommands: `refresh`, `reindex-bm25`, `reembed`
- Migration additions for refresh cadence and deprecation metadata

**Behaviors reimplemented:**
- Async ingest with job polling
- Background refresh scheduler
- Document deprecation lifecycle
- BM25 reindexing
- Re-embedding with model migration support

**Simplified vs matched:**
- **Simplified:** scheduler runs basic stale detection only (no notifications yet)
- **Matched (or improved):** async ingest API contract (job_id polling)
- **Matched (or improved):** admin endpoint paths

**Key deliverables:**
- [ ] Job queue with in-memory + Redis backends
- [ ] Async ingest endpoint
- [ ] Refresh scheduler
- [ ] Admin endpoints
- [ ] CLI subcommands
- [ ] Integration test: async ingest → poll until complete
- [ ] Test: reindex-bm25 updates sparse vectors

**Dependencies:** Phase 4 (ingest service), Phase 7 (server).

**Risks:**
- Redis backend needs a Redis instance in docker-compose.
- Scheduler timing in tests — use a manual trigger, don't rely on cron timing.

**Exit criteria:** Async ingest works with job polling. BM25 reindexing produces correct sparse vectors. Refresh scheduler detects stale documents.

**Status: NOT STARTED** — Ingest is synchronous only.

---

### Phase 13: Additional Extractors

**Goal:** Expand format support toward full parity (15 formats, 57 extensions).

**Scope:**
- Add extractors to the `ExtractorRegistry`:
  - DOCX (docx-rs or similar)
  - PPTX
  - XLSX
  - HTML (html2text or scraper)
  - CSV
  - JSON/JSONL
  - YAML
  - XML
  - Source code (language detection + passthrough)
  - EML email (mail-parser, with the `body_text()` pitfall handled)
  - Images via OCR (Tesseract, with subprocess timeout pattern)
  - ZIP archives (recursive extraction with depth/size limits)
- Config: `ingest_allowed_file_types` with per-format flags

**Behaviors reimplemented:**
- All 15 extractors from projectAlpha
- ZIP recursive extraction with safety limits (max depth, max entries, max size, compression ratio)
- OCR with subprocess timeout (try_wait + kill pattern)
- Email body extraction with script/style stripping

**Simplified vs matched:**
- **Matched (or improved):** extraction output for each format (same text from same input)
- **Matched (or improved):** ZIP safety limits
- **Matched (or improved):** OCR timeout pattern

**Key deliverables:**
- [ ] 12 additional extractors
- [ ] ZIP recursive extraction with all safety limits
- [ ] OCR with Tesseract subprocess management
- [ ] Email extraction with mail-parser pitfall handled
- [ ] Unit tests for each extractor
- [ ] Integration test: ingest a mixed-format directory

**Dependencies:** Phase 4 (ingest pipeline, ExtractorRegistry).

**Risks:**
- Native library dependencies: pdfium (already handled), Tesseract (optional, gated by feature flag)
- DOCX/PPTX crate quality varies — evaluate options

**Exit criteria:** All 15 formats extractable. ZIP bombs rejected. OCR timeouts handled cleanly.

**Status: NOT STARTED** — Only PDF/MD/TXT extractors. Tesseract OCR fallback for scanned PDFs shipped in Phase 3.

---

### Phase 14: Evidence Packs and Notifications

**Goal:** Evidence export with signatures. Email notifications.

**Scope:**
- `rag-evidence` crate:
  - `EvidenceSigner` trait, `Ed25519Signer`, `NoOpSigner`
  - Evidence pack creation (ZIP with manifest, timeline, artifacts)
  - Signature verification
- `rag-notifications` crate:
  - SMTP sending via lettre
  - Handlebars email templates
  - Concurrent sending (buffer_unordered)
- New endpoints:
  - `GET /runs/:id/export` — evidence pack download
  - `GET /evidence-packs` — list packs
  - `GET /evidence-packs/:filename` — download
- Integration with refresh scheduler for stale document notifications

**Behaviors reimplemented:**
- Evidence pack creation and signing
- Signature verification
- Email notification sending
- Template rendering

**Simplified vs matched:**
- **Matched (or improved):** evidence pack ZIP format and manifest schema
- **Matched (or improved):** Ed25519 signature format (compatible with projectAlpha verification)
- **Deferred:** cloud KMS signing, RFC 3161 timestamping

**Key deliverables:**
- [ ] Evidence signer trait + Ed25519 implementation
- [ ] Evidence pack ZIP creation
- [ ] Signature verification
- [ ] SMTP email sending
- [ ] Handlebars template rendering
- [ ] Export and download endpoints
- [ ] Tests: create → sign → verify round-trip
- [ ] Test: notification delivery

**Dependencies:** Phase 9 (agent runs to export), Phase 12 (scheduler for notifications).

**Risks:**
- Email testing needs a mock SMTP server (mailhog or similar in docker-compose).

**Exit criteria:** Evidence packs created, signed, and verifiable. Email notifications sent for stale documents.

**Status: NOT STARTED** — `rag-evidence` and `rag-notifications` are empty stubs.

---

### Phase 15: Guardrails and Security Hardening

**Goal:** Input validation, injection detection, PII handling.

**Scope:**
- `guardrails` module: prompt injection detection (block/flag modes)
- `pii` module: PII detection and redaction
- `sql_safety` module: SQL injection prevention in user inputs
- `ssrf` module: SSRF prevention for external URLs
- `egress` module: egress filtering for outbound requests
- `credential_crypto` module: encryption for stored credentials
- Cooperative cancellation: per-request tokens, `check_cancelled()` checkpoints
- Server middleware updates: cancellation token injection

**Behaviors reimplemented:**
- Prompt injection detection
- PII redaction
- SQL safety checks
- SSRF prevention
- Cooperative cancellation through pipeline stages

**Simplified vs matched:**
- **Matched (or improved):** guardrail detection accuracy (port test cases)
- **Matched (or improved):** cancellation token hierarchy

**Key deliverables:**
- [ ] Guardrail checks integrated into chat and search paths
- [ ] PII redaction in responses
- [ ] Cooperative cancellation
- [ ] Integration tests for each guardrail
- [ ] Test: injection attempt → blocked/flagged
- [ ] Test: cancellation mid-ingest → clean rollback

**Dependencies:** Phase 7 (server middleware).

**Risks:**
- Guardrail false positive rate — port projectAlpha's test cases and thresholds exactly.

**Exit criteria:** Injection attempts detected. PII redacted. Cancellation propagates cleanly.

**Status: NOT STARTED**

---

## 4. MVP vs Parity Roadmap

```
Phase 0  ████████████████████  Scaffold              ─┐
Phase 1  ████████████████████  Chunking               │
Phase 2  ████████████████████  Storage                │
Phase 3  ████████████████████  Extraction/Embedding   │  MVP ✓ COMPLETE
Phase 4  ████████████████████  Ingest Pipeline        │  (2026-03-30)
Phase 5  ████████████████████  Retrieval/Context      │
Phase 6  ████████████████████  Chat                   │
Phase 7  ████████████████████  Server                 │
Phase 8  ████████████████████  Client/CLI             ─┘
         ─────────── MVP LINE ───────────
Phase 9  ░░░░░░░░░░░░░░░░░░░░  Agents                 ─┐
Phase 10 ░░░░░░░░░░░░░░░░░░░░  Auth/RBAC               │
Phase 11 ░░░░░░░░░░░░░░░░░░░░  Prompt Catalog          │  Parity
Phase 12 ░░░░░░░░░░░░░░░░░░░░  Async Ingest/Jobs       │
Phase 13 ░░░░░░░░░░░░░░░░░░░░  All Extractors          │
Phase 14 ░░░░░░░░░░░░░░░░░░░░  Evidence/Notifications  │
Phase 15 ░░░░░░░░░░░░░░░░░░░░  Guardrails/Security     ─┘
         ─────────── PARITY LINE ──────────
Hardening (see Section 6)      Licensing, SBOM, Obfuscation, OTel
```

### Parallelization Opportunities

Some phases can overlap:

- **Phase 1 (chunking) runs in parallel with Phase 2 (storage)** — no dependencies between them
- **Phase 10 (auth) can start alongside Phase 9 (agents)** — independent middleware vs. domain logic
- **Phase 11 (prompts) can start alongside Phase 12 (async ingest)** — independent features
- **Phase 13 (extractors) can start anytime after Phase 4** — additive, no architectural impact

---

## 5. Risks and Open Questions

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| **sqlx offline mode in CI** | CI needs Postgres for compile-time SQL checks or `sqlx-data.json` | Set up Postgres service in GitHub Actions or commit `sqlx-data.json` |
| **Qdrant version coupling** | gRPC client version must match server | Pin both in docker-compose.yml and Cargo.toml |
| **pdfium native library** | Platform-specific binary, not on crates.io | Vendor in repo or download in build script. Document setup. |
| **graph-flow crate maintenance** | If unmaintained, agent-core is blocked | Evaluate early (Phase 9 start). Fallback: build minimal DAG runner (~1-2 weeks). |
| **BM25 quality regression** | Different tokenization → different retrieval quality | Port BM25 tokenization exactly. Run comparative tests against projectAlpha output. |
| **Untracked divergences** | Apex silently differs from projectAlpha without documentation | Record every intentional divergence as a beads issue. Review divergences when porting dependent code. |
| **Migration consolidation** | Merging 15 migrations into fewer may miss edge cases | Review each migration carefully. Test against production-like data volume. |
| **OpenAI API cost during development** | Repeated embedding/chat calls during testing | Use mock embedder and mock LLM for all automated tests. Real API only for manual validation. |

### Open Questions

1. **Schema consolidation strategy:** Merge all 15 migrations into ~5, or replay them exactly? Recommendation: consolidate, but verify table structure matches.
2. **Test corpus:** Should we create a new test corpus or copy test data from projectAlpha? Recommendation: create a minimal new corpus for MVP, port projectAlpha's for parity eval.
3. **Config format:** Keep `config/app.toml` structure identical, or redesign sections? Recommendation: same structure, fewer keys initially.
4. **OpenAPI spec:** Generate from code (e.g., utoipa) or maintain manually? The existing spec is 192KB. Recommendation: generate from code with utoipa annotations to avoid drift.
5. **`graph-flow` evaluation:** When exactly in Phase 9 should we make the keep/replace decision? Recommendation: first task of Phase 9, before writing any agent-core code.

---

## 6. Deferred Hardening Stage

These capabilities are explicitly out of scope for both MVP and parity phases. They are addressed in a separate hardening stage after parity is achieved.

### 6.1 License Enforcement (`rag-license`)

- PASETO v4.public token verification
- Edition tiers (DEMO/PROFESSIONAL/ENTERPRISE)
- Self-audit loop (background re-verification every 300s)
- State machine (Valid → GracePeriod → Expired)
- Degradation policies (DEGRADE/STOP/WARN_ONLY)
- License guard middleware
- Binary integrity self-check (Ed25519-signed sidecar)
- `license-gen` CLI tool

**Prerequisite:** Parity phases complete. Auth/RBAC middleware in place.

### 6.2 SBOM Generation (`rag-sbom`)

- CycloneDX SBOM generation at compile time
- Runtime enrichment with infrastructure versions (Postgres, Qdrant, OS)
- Ed25519 signing of SBOM via `SbomSigner` trait
- SPDX format conversion
- `/sbom` and `/sbom/verify` endpoints

**Prerequisite:** Evidence signing infrastructure (Phase 14).

### 6.3 Binary Obfuscation (`obfuscate-macros`)

- Compile-time XOR string obfuscation proc-macro (`#[obfuscate_strings]`)
- Hardened release builds (stripped, LTO, obfuscated strings)
- Release signing (`just build-release-sign`)

**Prerequisite:** All code finalized. Applied as last step before distribution.

### 6.4 OpenTelemetry Integration

- OTLP exporter to Jaeger
- Distributed trace propagation across HTTP boundaries
- Prometheus metrics endpoint (`/metrics`)
- Langfuse LLM observability integration
- `just run-server-otel` recipe
- Grafana/Prometheus/AlertManager configuration

**Prerequisite:** Server stable. Can be added incrementally without code changes (middleware + config).

### 6.5 Evaluation Framework (`rag-eval`)

- Port `rag-eval` crate from projectAlpha
- Hallucination detection evaluation
- Performance baseline benchmarks
- Metrics validation suite
- Test corpora management
- Nightly evaluation workflow (GitHub Actions)

**Prerequisite:** Full retrieval parity (Phase 15 complete). Same test corpora available.

### 6.6 Additional CI Hardening

- Crate dependency boundary validation (`check-crate-boundaries.py`)
- CLI documentation freshness checks
- Documentation link verification
- SBOM generation in CI
- Security audit workflow (`cargo-audit`)
- Nightly evaluation workflow

**Prerequisite:** Parity phases complete. All crates and docs in place.
