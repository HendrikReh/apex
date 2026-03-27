# Phase 2: Core Storage Layer — Design

**Date:** 2026-03-27
**Phase:** 2 of rebuild plan
**Reference:** projectAlpha `/crates/rag-core/`

---

## Architecture

```
rag-core/src/
├── lib.rs              # Re-exports: AppConfig, Stores, TenantId
├── config.rs           # AppConfig: toml + env loading
├── tenant.rs           # TenantId newtype with validation
├── error.rs            # CoreError enum (thiserror)
├── stores/
│   ├── mod.rs          # Stores struct (PgPool + QdrantClient)
│   ├── documents.rs    # Document CRUD (tenant-scoped)
│   ├── chunks.rs       # Chunk batch insert/delete (tenant-scoped)
│   ├── vectors.rs      # Qdrant collection + point operations
│   └── corpus_stats.rs # BM25 avgdl tracking
└── migrations/
    ├── 0001_documents_and_chunks.sql
    └── 0002_conversations.sql
```

## Key Decisions

### Config

Start lean — ~20 fields covering bind_addr, database, qdrant, embedder, default_collection, chunking defaults. No hallucination/guardrails/refresh/i18n until those phases land. Load via `config` crate (toml) + `dotenvy` for `.env`, env vars override toml.

### TenantId

Newtype with validation (1-128 chars, alphanumeric + `-` + `_`). Implements `FromStr`, `Display`, `Serialize`/`Deserialize`, `sqlx::Type`. Default: `"default"`.

### Stores

Concrete struct, not traits. Holds `PgPool` + `QdrantClient` + `default_collection`. Methods directly on the struct. Extract traits later if mock backends are needed.

### Migrations

2 consolidated migrations:
- `0001`: `documents` (PK: `tenant, id`), `chunks` (FK to documents), `corpus_stats` (computed `avgdl`)
- `0002`: `conversations`, `messages`, `runs` (minimal — just enough for chat history)

### Error Handling

`thiserror` for `CoreError` at module boundaries, `anyhow` internally. No `.unwrap()`/`.expect()`.

### Testing

Integration tests against real Postgres + Qdrant via `just up`. Use `sqlx::migrate!()` to run migrations in test setup.

## Out of Scope

- FTS/tsvector (Phase 3: retrieval)
- Embedder trait/impl (Phase 3)
- Ingest pipeline (Phase 3)
- Prompt registry, agents, hallucination audit, refresh cadence (later phases)
- Trait abstractions for stores (extract when needed)
