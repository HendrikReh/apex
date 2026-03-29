# Phase 8: HTTP Client and CLI — Design Spec

**Beads issue:** apex-1c6

## Goal

Provide a Rust HTTP client library (`rag-client`) and CLI binary (`rag-cli`) for the Apex RAG server API built in Phase 7. The client covers all 9 endpoints. The CLI exposes 4 subcommands for the primary user workflows: ingest, search, chat, and collection-stats.

## Architecture

Client-first, CLI second. `rag-client` is a standalone crate (no workspace dependencies) wrapping `reqwest` with automatic tenant header injection and typed methods per endpoint. `rag-cli` is a thin Clap binary that dispatches subcommands to handlers generic over an `ApiClient` trait defined in `rag-client`.

**Crate dependencies:**
```
rag-cli -> rag-client -> reqwest
```

`rag-client` depends only on: `reqwest`, `serde`, `serde_json`, `thiserror`, `uuid`, `trait-variant`.
`rag-cli` depends on: `rag-client`, `clap`, `tokio`, `anyhow`, `serde`, `serde_json`, `uuid`.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| CLI output style | Human-friendly default, `--json` flag for machine-readable | Scriptable without sacrificing readability |
| Ingest path input | Positional args, auto-detect file vs directory | Server already handles detection; simpler UX |
| Chat mode | Single-shot default, `--interactive` for REPL | MVP covers both patterns; server tracks conversation_id |
| Client API layers | Generic helpers + typed convenience methods | Extensible base, clean CLI calls |
| Request/response types | Own types in rag-client | Crate boundary rule: rag-client has no workspace deps |
| Tenant validation | Local TenantId newtype in rag-client | Same rules as rag-core but independently defined |
| ApiClient trait | trait-variant for Send async trait | Enables handler unit testing with FakeClient |
| `--interactive --json` | Rejected at CLI boundary | Conflicting output models; defer NDJSON to post-MVP |
| Ingest exit codes | 0 on partial failure, 1 on transport/HTTP/validation errors | Partial success is success for scripting |

---

## Section 1: TenantApiClient

### Core struct

```rust
pub struct TenantApiClient {
    client: reqwest::Client,
    base_url: reqwest::Url,
    tenant: TenantId,
}
```

- `base_url`: stored as `reqwest::Url` (validated at construction, safe joining via `Url::join`)
- `tenant`: local newtype — non-empty, alphanumeric + `-_`, max 128 chars

### TenantId newtype

Defined in `rag-client`, mirrors `rag-core::TenantId` validation rules:
- Non-empty
- Characters: alphanumeric, `-`, `_`
- Max length: 128

### Construction

- `new(base_url: &str, tenant: &str) -> Result<Self, ClientError>` — 60s default timeout
- `with_timeout(base_url: &str, tenant: &str, timeout: Duration) -> Result<Self, ClientError>` — custom timeout

Both validate `base_url` (parseable as URL) and `tenant` (TenantId rules).

### Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("invalid tenant: {0}")]
    InvalidTenant(String),

    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),  // send failed, connection refused, timeout

    #[error("server returned {status}: {body}")]
    HttpStatus { status: u16, url: String, body: String },  // non-2xx response

    #[error("failed to decode response: {0}")]
    Decode(reqwest::Error),  // body read or JSON deserialization failed
}
```

### Generic helpers

```rust
pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError>
pub async fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, ClientError>
pub async fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B, timeout: Duration) -> Result<T, ClientError>
pub async fn get_response(&self, path: &str) -> Result<reqwest::Response, ClientError>
```

### Internal request building

- Private `request(&self, method: Method, path: &str) -> Result<RequestBuilder, ClientError>`: builds request with `x-tenant` header, resolves path via `self.base_url.join(path)`
- Private `into_success<T>(response: Response) -> Result<T, ClientError>`: checks status, reads body, deserializes or returns `HttpStatus`/`Decode` error
- `post_json_with_timeout` sets per-request timeout on the `RequestBuilder`, reuses the shared `reqwest::Client`

### Path handling

All helpers normalize paths via `Url::join`. Both `/chat` and `chat` resolve correctly against a base URL without a path component. No string concatenation.

---

## Section 2: Request/Response Types

All types defined in `rag-client`. Request types derive `Serialize`, response types derive `Deserialize`. These mirror the server contracts from Phase 7 exactly.

### Ingest

```rust
#[derive(Serialize)]
pub struct IngestRequest {
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
}

#[derive(Deserialize)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(default)]
    pub failures: Vec<IngestFailure>,
}

#[derive(Deserialize)]
pub struct IngestFailure {
    pub path: String,
    pub error: String,
}
```

### Search

```rust
#[derive(Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
}

#[derive(Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Deserialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

#[derive(Serialize)]
pub struct HybridSearchRequest {
    pub query: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense_top_k: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparse_top_k: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrf_k: Option<u32>,
}

#[derive(Deserialize)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Deserialize)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}
```

### Chat

```rust
#[derive(Serialize)]
pub struct ChatRequest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<Citation>,
    pub usage: Usage,
    pub model: String,
}

#[derive(Deserialize)]
pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}
```

### Collections

```rust
#[derive(Deserialize)]
pub struct CollectionStatsResponse {
    pub collection: String,
    pub tenant: String,
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}
```

### Health / Readiness

```rust
#[derive(Deserialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub checks: ReadinessChecks,
}

#[derive(Deserialize)]
pub struct ReadinessChecks {
    pub postgres: String,
    pub qdrant: String,
}
```

---

## Section 3: Typed Convenience Methods

```rust
impl TenantApiClient {
    pub async fn health(&self) -> Result<(), ClientError>
    pub async fn readiness(&self) -> Result<ReadinessResponse, ClientError>
    pub async fn ingest(&self, req: &IngestRequest) -> Result<IngestResponse, ClientError>
    pub async fn search_dense(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>
    pub async fn search_sparse(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>
    pub async fn search_hybrid(&self, req: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError>
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError>
    pub async fn collection_stats(&self, collection: &str) -> Result<CollectionStatsResponse, ClientError>
}
```

**Notes:**
- `health()` returns `Result<(), ClientError>` — uses `get_response()`, verifies body is exactly `"ok"`, errors otherwise
- `ingest()` uses `post_json_with_timeout()` with 300s per-request timeout (large directories)
- `collection_stats()` percent-encodes the collection segment before building `/collections/{collection}/stats`; rejects empty collection locally before any network call
- All search/chat methods are thin wrappers around `post_json()`

---

## Section 4: ApiClient Trait

```rust
#[trait_variant::make(Send)]
pub trait ApiClient {
    async fn ingest(&self, req: &IngestRequest) -> Result<IngestResponse, ClientError>;
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError>;
    async fn search_dense(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_sparse(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_hybrid(&self, req: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError>;
    async fn collection_stats(&self, collection: &str) -> Result<CollectionStatsResponse, ClientError>;
    async fn health(&self) -> Result<(), ClientError>;
    async fn readiness(&self) -> Result<ReadinessResponse, ClientError>;
}
```

`TenantApiClient` implements `ApiClient`. CLI command handlers are generic over `impl ApiClient`, enabling unit testing with a `FakeClient`.

---

## Section 5: CLI Structure

### File layout

```
crates/rag-cli/src/
├── main.rs              # tokio entry, parse args, dispatch
├── cli.rs               # Clap derive structs
├── commands/
│   ├── mod.rs
│   ├── chat.rs          # chat subcommand handler
│   ├── ingest.rs        # ingest subcommand handler
│   ├── search.rs        # search subcommand handler
│   └── collections.rs   # collection-stats subcommand handler
└── output.rs            # shared output formatting (human vs --json)
```

### Clap struct

```rust
#[derive(Parser)]
#[command(name = "rag-cli", about = "CLI for the Apex RAG server")]
pub struct Cli {
    /// Server URL
    #[arg(long, env = "RAG_SERVER_URL", default_value = "http://127.0.0.1:8080")]
    pub server: String,

    /// Tenant identifier
    #[arg(long, env = "RAG_TENANT", default_value = "default")]
    pub tenant: String,

    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}
```

### Subcommands

```rust
pub enum Command {
    /// RAG chat (single-shot or interactive)
    Chat {
        #[arg(long)]
        query: String,
        /// Required for first message; omit when resuming via --conversation-id
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        conversation_id: Option<Uuid>,
    },
    /// Ingest files and directories
    Ingest {
        /// Paths to ingest (files or directories, auto-detected)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        collection: Option<String>,
    },
    /// Search for chunks (dense, sparse, or hybrid)
    Search {
        #[arg(long)]
        query: String,
        #[arg(long)]
        collection: String,
        /// Search mode: dense, sparse, or hybrid (default: hybrid)
        #[arg(long, value_enum, default_value_t = SearchMode::Hybrid)]
        mode: SearchMode,
        #[arg(long)]
        top_k: Option<u64>,
    },
    /// Show collection statistics
    CollectionStats {
        #[arg(long)]
        collection: String,
    },
}

#[derive(Clone, ValueEnum)]
pub enum SearchMode {
    Dense,
    Sparse,
    Hybrid,
}
```

### Validation

- `--interactive --json` is rejected at CLI validation before any network call
- Ingest requires at least one path (Clap `required = true`)
- Chat requires `--collection` when `--conversation-id` is not provided (validated in handler, not Clap — both are optional at the parse level)

---

## Section 6: Command Handlers

All handlers follow: validate CLI inputs -> build client request -> call typed method -> format output.

### Error model

`rag-cli` uses `anyhow::Result<()>` throughout. `ClientError` converts to `anyhow::Error` via `Into`. Write failures, validation errors, and client errors all propagate as `anyhow::Error`. The `main()` function catches the top-level error and renders it to stderr with the appropriate format (see Error rendering below).

### output.rs

```rust
pub fn print_or_json<W: Write, T: Serialize>(
    writer: &mut W,
    json_mode: bool,
    value: &T,
    human_fn: impl FnOnce(&T, &mut W) -> anyhow::Result<()>,
) -> anyhow::Result<()>
```

If `json_mode`, serialize `value` to `writer`. Otherwise call `human_fn`. Production passes `io::stdout().lock()`, tests pass `Vec<u8>`.

### Ingest handler

- Validate all paths exist and are absolute (fail fast before network)
- Build `IngestRequest` with paths as strings + optional collection
- Call `client.ingest(&req)` (300s timeout)
- Human output: `Ingested 12 documents, 48 chunks, 0 skipped`
- If failures non-empty: `WARN: /path/to/file — error message` per failure
- JSON output: serialize full `IngestResponse`
- Exit code: 0 on success (even with per-file failures), 1 only for transport/HTTP/validation errors
- Per-file failures are NOT command failure — partial success is success for scripting

### Chat handler

**Single-shot (default):**
- Validate: `--collection` is required unless `--conversation-id` is provided
- Call `client.chat(&req)` with query + optional collection + optional conversation_id
- Human output:
  ```
  The document discusses...

    [1] chunk-abc (doc-123, chunk 2)
    [2] chunk-def (doc-456, chunk 0)

  [conversation: 550e8400-...] [model: gpt-4o]
  ```
- Citations printed as numbered references before trailing metadata
- JSON output: serialize full `ChatResponse`

**Interactive (`--interactive`):**
1. First turn uses `--query` + `--collection`
2. Server returns `conversation_id`
3. Loop: prompt `> ` -> read stdin line -> send with `conversation_id` (no collection) -> print answer
4. Exit on EOF or empty line
5. `--interactive --json` is rejected at CLI validation

### Search handler

- Dispatch to `client.search_dense()`, `client.search_sparse()`, or `client.search_hybrid()` based on `--mode`
- For dense/sparse: build `SearchRequest` with query, collection, optional top_k
- For hybrid: build `HybridSearchRequest` (top_k maps to both dense_top_k and sparse_top_k)
- Human output:
  ```
  [1] (score: 0.92) chunk-abc — doc-123, chunk 2
      First 80 chars of text...
  [2] (score: 0.87) chunk-def — doc-456, chunk 0
      First 80 chars of text...
  ```
- JSON output: serialize full `SearchResponse` or `HybridSearchResponse`

### Collection-stats handler

- Call `client.collection_stats(&collection)`
- Human output:
  ```
  Collection: my-collection
  Tenant:     default
  Documents:  42
  Tokens:     12,345
  Avg Doc Length: 293.93
  ```
- JSON output: serialize full `CollectionStatsResponse`

### Error rendering (all commands)

- `ClientError::InvalidBaseUrl` / `InvalidTenant` -> `Error: {message}`, exit 1
- `ClientError::Transport` -> `Error: connection failed — {details}`, exit 1
- `ClientError::HttpStatus` -> `Error: server returned {status} — {body}`, exit 1
- `ClientError::Decode` -> `Error: unexpected server response — {details}`, exit 1

---

## Section 7: Testing Strategy

Three explicit layers, no mixing.

### Layer 1: Parser tests (in cli.rs)

| Test | Verifies |
|------|----------|
| `ingest_rejects_no_paths` | Clap rejects `rag-cli ingest` with no arguments |
| `interactive_json_conflict` | Validation rejects `--interactive --json` |
| `server_env_fallback` | `RAG_SERVER_URL` env var populates `--server` |

### Layer 2: Handler unit tests (in commands/*.rs)

Use `FakeClient` implementing `ApiClient` with canned responses and captured calls.

| Test | Verifies |
|------|----------|
| `ingest_human_output` | Success -> `Ingested N documents...` in Write sink |
| `ingest_partial_failure` | WARN lines in sink, no error returned |
| `ingest_json_output` | Valid JSON matching `IngestResponse` shape |
| `chat_single_shot` | Answer + citations + metadata in sink |
| `chat_conversation_id_threaded` | Second call carries `conversation_id` from first response |
| `chat_requires_collection_without_conversation_id` | Error when neither `--collection` nor `--conversation-id` provided |
| `chat_allows_no_collection_with_conversation_id` | Succeeds with `--conversation-id` and no `--collection` |
| `search_hybrid_human_output` | Numbered results with scores and text preview |
| `search_dense_dispatches_correctly` | `--mode dense` calls `search_dense()` not `search_hybrid()` |
| `search_json_output` | Valid JSON matching `SearchResponse` / `HybridSearchResponse` |
| `collection_stats_human_output` | Formatted table output |
| `collection_stats_json_output` | Valid JSON matching `CollectionStatsResponse` |
| `client_error_http_status_output` | `Error: server returned 400 — ...` format |
| `client_error_transport_output` | `Error: connection failed — ...` format |

### Layer 3: Output tests (in output.rs)

| Test | Verifies |
|------|----------|
| `print_or_json_human` | Calls human_fn, nothing serialized |
| `print_or_json_json` | Serializes to sink, human_fn not called |
| `print_or_json_write_error` | Broken writer returns Err |

### rag-client tests (integration-style with spawn_app)

Self-contained test server with `Arc<Mutex<Vec<CapturedRequest>>>`.

| Test | Verifies |
|------|----------|
| `tenant_header_injected` | Every request carries `x-tenant` header |
| `invalid_base_url_rejected` | `ClientError::InvalidBaseUrl` on bad URL |
| `invalid_tenant_rejected` | `ClientError::InvalidTenant` on empty/invalid tenant |
| `health_ok` | `health()` succeeds when body is `"ok"` |
| `health_unexpected_body` | `health()` errors when body is not `"ok"` |
| `readiness_json` | Deserializes `ReadinessResponse` correctly |
| `ingest_request_shape` | Captured body matches `IngestRequest` serialization |
| `ingest_timeout_override` | Client with 1s default, server delays 2s: ingest succeeds (300s override), plain post_json times out (1s default) |
| `chat_roundtrip` | Correct body sent, `ChatResponse` with `Uuid` fields deserialized |
| `search_hybrid_roundtrip` | Request/response round-trip |
| `collection_stats_path_encoding` | Special chars in collection name are percent-encoded |
| `collection_stats_empty_rejected` | Empty collection rejected locally, no request sent |
| `http_error_preserved` | 400 -> `ClientError::HttpStatus` with status + body |
| `decode_error` | Invalid JSON -> `ClientError::Decode` |

---

## Section 8: cq Integration

During Phase 8 implementation, the cq knowledge store is used at three points:

1. **Before implementation:** `query` relevant domains (e.g. `["rust", "reqwest", "cli"]`) to check for known pitfalls
2. **After solving non-obvious problems:** `propose` the insight for future sessions
3. **During review:** `confirm` units that proved correct, `flag` stale or incorrect ones

Knowledge units already seeded from Phases 1-7:
- Destructive test isolation in separate binaries
- UUID-scoped tenants for test isolation
- String-prefix error classification for anyhow
- Clippy macro false positives (tokio::join!, serde_json::json!)
- reqwest Url::join leading-slash behavior

---

## Exit Criteria

- `rag-cli ingest data/ --collection test` ingests files via server API
- `rag-cli search --query "..." --collection test` returns ranked results
- `rag-cli search --query "..." --collection test --mode dense` uses dense search
- `rag-cli chat --query "..." --collection test` returns a grounded response
- `rag-cli chat --query "..." --collection test --interactive` enters REPL mode
- `rag-cli chat --query "..." --conversation-id <uuid>` resumes without collection
- `rag-cli collection-stats --collection test` shows stats
- `--json` flag produces machine-readable output on all subcommands
- All rag-client tests pass against local spawn_app server
- All rag-cli handler tests pass with FakeClient
- The MVP user journey works end-to-end via CLI
