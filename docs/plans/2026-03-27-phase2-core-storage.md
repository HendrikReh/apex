# Phase 2: Core Storage Layer — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the `rag-core` crate with config loading, tenant isolation, Postgres + Qdrant stores, and 2 consolidated migrations.

**Architecture:** Concrete `Stores` struct holding `PgPool` + `QdrantClient`. `AppConfig` loads from `config/app.toml` with env overrides via `dotenvy`. `TenantId` newtype validates all tenant strings. Two migrations consolidate projectAlpha's 15 into documents/chunks/corpus_stats + conversations.

**Tech Stack:** sqlx 0.8 (Postgres, compile-time checked), qdrant-client 1.16, toml + dotenvy for config, thiserror for errors, tokio async runtime.

**Prerequisites:** Docker services running (`just up`). `DATABASE_URL` set in `.env`.

---

### Task 1: Add dotenvy dependency to rag-core

**Files:**
- Modify: `crates/rag-core/Cargo.toml`

**Step 1: Add the dependency**

Add `dotenvy.workspace = true` to `[dependencies]` in `crates/rag-core/Cargo.toml`.

**Step 2: Verify it compiles**

Run: `cargo check -p rag-core`
Expected: success

**Step 3: Commit**

```bash
git add crates/rag-core/Cargo.toml
git commit -m "chore(rag-core): add dotenvy dependency"
```

---

### Task 2: TenantId newtype

**Files:**
- Create: `crates/rag-core/src/tenant.rs`
- Modify: `crates/rag-core/src/lib.rs`

**Step 1: Write tests for TenantId**

In `tenant.rs`, add a `#[cfg(test)] mod tests` block with tests for:
- Valid tenant IDs: `"default"`, `"my-tenant"`, `"tenant_123"`, `"A"`
- Empty string → `TenantIdError::Empty`
- Too long (129 chars) → `TenantIdError::TooLong`
- Invalid characters (`"has spaces"`, `"has/slash"`) → `TenantIdError::InvalidCharacter`
- `default_tenant()` returns `TenantId("default")`
- `FromStr` roundtrip: parse then display
- `Serialize` / `Deserialize` roundtrip via serde_json
- `TryFrom<String>` and `TryFrom<&str>`

**Step 2: Implement TenantId**

```rust
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

const MAX_TENANT_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct TenantId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TenantIdError {
    #[error("tenant ID must not be empty")]
    Empty,
    #[error("tenant ID exceeds {MAX_TENANT_LENGTH} characters")]
    TooLong,
    #[error("invalid character '{0}' at position {1}")]
    InvalidCharacter(char, usize),
}
```

Implement:
- `TenantId::new(s: impl Into<String>) -> Result<Self, TenantIdError>` — validate non-empty, max length, chars are `[a-zA-Z0-9_-]`
- `TenantId::validate(s: &str) -> Result<(), TenantIdError>` — static validation
- `TenantId::as_str(&self) -> &str`
- `TenantId::default_tenant() -> Self` — returns `TenantId("default".into())`
- Traits: `Display`, `FromStr`, `TryFrom<String>`, `TryFrom<&str>`, `From<TenantId> for String`, `AsRef<str>`, `Deref` (target `str`)
- Custom `Deserialize` that validates on deserialization

**Step 3: Wire into lib.rs**

Add `pub mod tenant;` and `pub use tenant::TenantId;` to `lib.rs`.

**Step 4: Run tests**

Run: `cargo test -p rag-core`
Expected: all TenantId tests pass

**Step 5: Commit**

```bash
git add crates/rag-core/src/tenant.rs crates/rag-core/src/lib.rs
git commit -m "feat(rag-core): add TenantId newtype with validation"
```

---

### Task 3: AppConfig with toml + env loading

**Files:**
- Create: `crates/rag-core/src/config.rs`
- Modify: `crates/rag-core/src/lib.rs`

**Step 1: Write tests for AppConfig**

Tests for:
- `AppConfig::from_env()` with defaults (no toml file, no env vars) — all fields have sane defaults
- Env var override: set `DATABASE_URL` env var, verify it takes precedence
- `EmbedderKind` parsing: `"openai"` → `OpenAi`, `"mock"` → `Mock`
- Unknown embedder string → error

**Step 2: Implement AppConfig**

Lean config with ~20 fields matching `config/app.toml`:

```rust
#[derive(Debug, Clone)]
pub struct AppConfig {
    // Qdrant
    pub qdrant_url: String,
    pub qdrant_api_key: Option<String>,
    pub qdrant_timeout_secs: u64,
    pub qdrant_connect_timeout_secs: u64,
    // Postgres
    pub postgres_url: String,
    pub postgres_max_connections: u32,
    pub postgres_connect_timeout_secs: u64,
    // BM25
    pub bm25_avgdl: f32,
    pub bm25_k1: f32,
    pub bm25_b: f32,
    // Collections
    pub default_collection: String,
    // Embedding
    pub embedding_model: String,
    pub embedder: EmbedderKind,
    pub embed_timeout_secs: u64,
    pub embed_max_retries: u32,
    // Server
    pub bind_addr: String,
    // Auth
    pub auth_mode: AuthMode,
    pub tenant_header: String,
    pub request_id_header: String,
}
```

Enums:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedderKind { OpenAi, Mock }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode { None, ApiKey, Oidc }
```

TOML intermediate struct:
```rust
#[derive(Deserialize)]
struct AppSettings {
    app: Option<AppSection>,
}
#[derive(Deserialize)]
struct AppSection { /* all fields as Option<T> */ }
```

Loading pattern in `AppConfig::from_env() -> Result<Self>`:
1. `dotenvy::dotenv().ok()` — load `.env` silently
2. Read `APP_CONFIG_PATH` env var (default `"config/app.toml"`)
3. If file exists, parse via `toml::from_str::<AppSettings>()`
4. For each field: `env::var("ENV_NAME").ok().and_then(parse) .or(file_value) .unwrap_or(default)`
5. `DATABASE_URL` → `postgres_url`, `QDRANT_URL` → `qdrant_url`, `QDRANT_API_KEY` → `qdrant_api_key`

**Step 3: Wire into lib.rs**

Add `pub mod config;` and `pub use config::AppConfig;` to `lib.rs`.

**Step 4: Run tests**

Run: `cargo test -p rag-core`
Expected: all config tests pass

**Step 5: Commit**

```bash
git add crates/rag-core/src/config.rs crates/rag-core/src/lib.rs
git commit -m "feat(rag-core): add AppConfig with toml + env loading"
```

---

### Task 4: SQL Migrations

**Files:**
- Create: `crates/rag-core/migrations/0001_documents_and_chunks.sql`
- Create: `crates/rag-core/migrations/0002_conversations.sql`

**Step 1: Write migration 0001 — documents, chunks, corpus_stats**

```sql
-- Documents table: tenant-scoped document metadata (ADR-002)
CREATE TABLE IF NOT EXISTS documents (
    tenant     TEXT NOT NULL CHECK (tenant <> ''),
    id         TEXT NOT NULL,
    title      TEXT NOT NULL,
    language   TEXT,
    metadata   JSONB,
    source_path TEXT,
    version    TEXT,
    checksum   TEXT,
    ingest_run_id UUID,
    token_count BIGINT,
    collection TEXT,
    created_at TIMESTAMPTZ DEFAULT now(),
    PRIMARY KEY (tenant, id)
);

CREATE INDEX IF NOT EXISTS idx_documents_ingest_run_id ON documents(ingest_run_id);
CREATE INDEX IF NOT EXISTS idx_documents_tenant ON documents(tenant);

-- Chunks table: denormalized chunk text with FK to documents
CREATE TABLE IF NOT EXISTS chunks (
    id          BIGSERIAL PRIMARY KEY,
    tenant      TEXT NOT NULL CHECK (tenant <> ''),
    document_id TEXT NOT NULL,
    chunk_index INT NOT NULL,
    text        TEXT NOT NULL,
    created_at  TIMESTAMPTZ DEFAULT now(),
    FOREIGN KEY (tenant, document_id) REFERENCES documents(tenant, id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_tenant_document_chunk
    ON chunks(tenant, document_id, chunk_index);

-- Corpus stats for BM25 avgdl tracking
CREATE TABLE IF NOT EXISTS corpus_stats (
    tenant       TEXT NOT NULL,
    collection   TEXT NOT NULL,
    total_docs   BIGINT DEFAULT 0,
    total_tokens BIGINT DEFAULT 0,
    avgdl        DOUBLE PRECISION GENERATED ALWAYS AS (
        CASE WHEN total_docs > 0 THEN total_tokens::float / total_docs ELSE 0.0 END
    ) STORED,
    updated_at   TIMESTAMPTZ DEFAULT now(),
    PRIMARY KEY (tenant, collection)
);
```

**Step 2: Write migration 0002 — conversations**

```sql
-- Users (anonymous or OIDC-authenticated)
CREATE TABLE IF NOT EXISTS users (
    id         UUID PRIMARY KEY,
    tenant     TEXT NOT NULL CHECK (tenant <> ''),
    email      TEXT,
    oidc_issuer  TEXT,
    oidc_subject TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_tenant_anonymous
    ON users (tenant) WHERE email IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_oidc_identity
    ON users (tenant, oidc_issuer, oidc_subject);

-- App sessions
CREATE TABLE IF NOT EXISTS app_sessions (
    id         UUID PRIMARY KEY,
    user_id    UUID REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ DEFAULT now(),
    last_seen  TIMESTAMPTZ DEFAULT now(),
    ip_hash    TEXT,
    user_agent TEXT
);

CREATE INDEX IF NOT EXISTS idx_app_sessions_user_id ON app_sessions(user_id);

-- Conversations
CREATE TABLE IF NOT EXISTS conversations (
    id          UUID PRIMARY KEY,
    user_id     UUID REFERENCES users(id) ON DELETE CASCADE,
    tenant      TEXT NOT NULL CHECK (tenant <> ''),
    title       TEXT,
    collection  TEXT,
    created_at  TIMESTAMPTZ DEFAULT now(),
    archived_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_conversations_user_id ON conversations(user_id);
CREATE INDEX IF NOT EXISTS idx_conversations_tenant ON conversations(tenant);

-- Messages within conversations
CREATE TABLE IF NOT EXISTS messages (
    id              UUID PRIMARY KEY,
    conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content         TEXT NOT NULL,
    metadata        JSONB,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id);

-- Runs (retrieval + generation executions)
CREATE TABLE IF NOT EXISTS runs (
    id              UUID PRIMARY KEY,
    conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE,
    user_id         UUID REFERENCES users(id) ON DELETE CASCADE,
    app_session_id  UUID REFERENCES app_sessions(id) ON DELETE SET NULL,
    tenant          TEXT NOT NULL CHECK (tenant <> ''),
    trace_id        TEXT,
    status          TEXT,
    meta            JSONB,
    created_at      TIMESTAMPTZ DEFAULT now(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_runs_conversation_id ON runs(conversation_id);
CREATE INDEX IF NOT EXISTS idx_runs_tenant ON runs(tenant);
CREATE INDEX IF NOT EXISTS idx_runs_trace_id ON runs(trace_id);
CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
```

**Step 3: Verify migrations compile with sqlx**

Run: `cargo check -p rag-core`
Expected: success (sqlx will verify SQL at compile time if `DATABASE_URL` is set and Postgres is running)

**Step 4: Commit**

```bash
git add crates/rag-core/migrations/
git commit -m "feat(rag-core): add 2 consolidated SQL migrations"
```

---

### Task 5: Stores struct with Postgres + Qdrant + migration runner

**Files:**
- Create: `crates/rag-core/src/stores/mod.rs`
- Modify: `crates/rag-core/src/lib.rs`

**Step 1: Implement Stores struct and constructor**

```rust
use anyhow::{Context, Result};
use qdrant_client::Qdrant;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use crate::config::AppConfig;

pub mod chunks;
pub mod corpus_stats;
pub mod documents;
pub mod vectors;

pub struct Stores {
    pool: PgPool,
    qdrant: Arc<Qdrant>,
    pub default_collection: String,
}
```

Constructor: `Stores::new(cfg: &AppConfig) -> Result<Self>`:
1. Create PgPool: `PgPoolOptions::new().max_connections(cfg.postgres_max_connections).acquire_timeout(...).connect(&cfg.postgres_url).await`
2. Build Qdrant: `Qdrant::from_url(&cfg.qdrant_url).timeout(...).connect_timeout(...).build()`; optionally `.api_key()` if set
3. Run migrations: `sqlx::migrate!("./migrations").run(&pool).await`
4. Return `Stores { pool, qdrant, default_collection }`

Accessors:
- `pub fn pg_pool(&self) -> &PgPool`
- `pub fn qdrant_client(&self) -> &Qdrant`

**Step 2: Wire into lib.rs**

Add `pub mod stores;` and `pub use stores::Stores;` to `lib.rs`.

**Step 3: Verify it compiles**

Run: `cargo check -p rag-core`
Expected: success

**Step 4: Commit**

```bash
git add crates/rag-core/src/stores/ crates/rag-core/src/lib.rs
git commit -m "feat(rag-core): add Stores struct with PgPool + Qdrant + migrations"
```

---

### Task 6: Document CRUD operations

**Files:**
- Create: `crates/rag-core/src/stores/documents.rs`

**Step 1: Write integration test**

Test `upsert_document` then `get_document` then `delete_document`, all tenant-scoped. Verify a document inserted for tenant "a" is not visible to tenant "b". Use a helper that creates a fresh `Stores` connected to the test database.

**Step 2: Implement document operations on `Stores`**

Methods on `impl Stores`:
- `upsert_document(tenant, id, title, language, metadata, source_path, version, checksum, ingest_run_id, token_count, collection)` — `INSERT ... ON CONFLICT (tenant, id) DO UPDATE`
- `get_document(tenant, id)` — `SELECT ... WHERE tenant = $1 AND id = $2`
- `delete_document(tenant, id)` — `DELETE FROM documents WHERE tenant = $1 AND id = $2` (cascades to chunks)
- `get_document_checksum(tenant, id, collection)` — `SELECT checksum FROM documents WHERE tenant = $1 AND id = $2`

Return types: use a `Document` struct with all columns.

**Step 3: Run integration tests**

Run: `cargo test -p rag-core -- documents`
Expected: pass (requires `just up`)

**Step 4: Commit**

```bash
git add crates/rag-core/src/stores/documents.rs
git commit -m "feat(rag-core): add document CRUD operations"
```

---

### Task 7: Chunk operations

**Files:**
- Create: `crates/rag-core/src/stores/chunks.rs`

**Step 1: Write integration test**

Test `insert_chunks` (batch), `get_chunks_by_document`, `delete_document_chunks`. Verify chunk_index ordering. Verify tenant isolation.

**Step 2: Implement chunk operations on `Stores`**

Methods:
- `insert_chunks(tenant, document_id, chunks: &[String])` — batch insert with chunk_index 0..N
- `get_chunks_by_document(tenant, document_id)` — `SELECT ... ORDER BY chunk_index`
- `delete_document_chunks(tenant, document_id)` — `DELETE FROM chunks WHERE tenant = $1 AND document_id = $2`

Return type: `ChunkRecord { id: i64, tenant: String, document_id: String, chunk_index: i32, text: String }`

**Step 3: Run integration tests**

Run: `cargo test -p rag-core -- chunks`
Expected: pass

**Step 4: Commit**

```bash
git add crates/rag-core/src/stores/chunks.rs
git commit -m "feat(rag-core): add chunk batch insert/delete operations"
```

---

### Task 8: Corpus stats operations

**Files:**
- Create: `crates/rag-core/src/stores/corpus_stats.rs`

**Step 1: Write integration test**

Test `update_corpus_stats` for a document, then `get_corpus_stats` returns correct avgdl. Test re-ingestion (update token count for same document). Test default avgdl when no stats exist.

**Step 2: Implement corpus stats on `Stores`**

Methods:
- `update_corpus_stats(tenant, collection, document_id, token_count)` — upsert corpus_stats row, incrementing total_docs/total_tokens
- `get_corpus_stats(tenant, collection, default_avgdl: f64)` — return `CorpusStats { total_docs, total_tokens, avgdl }`
- `get_avgdl(tenant, collection, default_avgdl: f64) -> f64` — convenience wrapper

Struct: `CorpusStats { pub total_docs: i64, pub total_tokens: i64, pub avgdl: f64 }`

**Step 3: Run integration tests**

Run: `cargo test -p rag-core -- corpus`
Expected: pass

**Step 4: Commit**

```bash
git add crates/rag-core/src/stores/corpus_stats.rs
git commit -m "feat(rag-core): add corpus stats operations for BM25 avgdl"
```

---

### Task 9: Qdrant vector operations

**Files:**
- Create: `crates/rag-core/src/stores/vectors.rs`

**Step 1: Write integration test**

Test `ensure_collection` creates a collection. Test `upsert_points` then `delete_document_points`. Test `collection_exists`. All tests use a unique collection name to avoid interference.

**Step 2: Implement Qdrant operations on `Stores`**

Methods:
- `ensure_collection(name, vector_size: u64, distance: Distance)` — create if not exists, validate if exists
- `collection_exists(name) -> bool`
- `delete_collection(name)` — delete Qdrant collection
- `list_collections() -> Vec<String>`
- `upsert_points(collection, points: Vec<PointStruct>)` — batch upsert
- `delete_document_points(collection, document_id, tenant)` — delete by filter on `tenant` + `document_id` payload fields
- `search_dense(collection, vector, tenant, limit) -> Vec<ScoredPoint>` — dense search filtered by tenant

**Step 3: Run integration tests**

Run: `cargo test -p rag-core -- vectors`
Expected: pass (requires Qdrant running via `just up`)

**Step 4: Commit**

```bash
git add crates/rag-core/src/stores/vectors.rs
git commit -m "feat(rag-core): add Qdrant collection and point operations"
```

---

### Task 10: Integration test for full document lifecycle

**Files:**
- Create: `crates/rag-core/tests/integration_lifecycle.rs`

**Step 1: Write end-to-end test**

Test the full lifecycle:
1. Create `AppConfig` pointing to test DB
2. Create `Stores::new(&config).await`
3. `ensure_collection("test_collection", 4, Cosine)`
4. `upsert_document(tenant, id, ...)`
5. `insert_chunks(tenant, id, ["chunk1", "chunk2"])`
6. `update_corpus_stats(tenant, "test_collection", id, 100)`
7. `get_document(tenant, id)` — verify fields
8. `get_chunks_by_document(tenant, id)` — verify 2 chunks in order
9. `get_avgdl(tenant, "test_collection", 300.0)` — verify not default
10. `delete_document(tenant, id)` — cascades to chunks
11. `get_chunks_by_document(tenant, id)` — verify empty
12. Cleanup: `delete_collection("test_collection")`

**Step 2: Run the integration test**

Run: `cargo test -p rag-core --test integration_lifecycle`
Expected: pass

**Step 3: Commit**

```bash
git add crates/rag-core/tests/integration_lifecycle.rs
git commit -m "test(rag-core): add full document lifecycle integration test"
```

---

### Task 11: cargo check and clippy pass

**Step 1: Run formatting check**

Run: `cargo fmt --all -- --check`
Expected: no diff

**Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings -D clippy::disallowed_methods`
Expected: no warnings

**Step 3: Run full workspace test**

Run: `cargo test --workspace`
Expected: all tests pass (rag-chunking + rag-core)

**Step 4: Commit any fixups**

If clippy or fmt required changes, commit them:
```bash
git commit -am "style(rag-core): fix clippy and fmt warnings"
```
