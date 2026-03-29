# Phase 7: HTTP Server (MVP API)

**Date:** 2026-03-29
**Status:** Approved
**Scope:** Axum REST API exposing the core RAG pipeline — ingest, search, and chat — with tenant extraction and request ID middleware. 9 endpoints, modular file layout, graceful shutdown, integration tests with mock LLM and mock embedder.
**Approach:** Modular by layer — `state.rs`, `middleware/`, `routes/`, `router.rs`, `main.rs`. Protected routes get tenant + request ID middleware applied before merging with public routes.

---

## 1. AppState (`state.rs`)

### 1.1 AppState

```rust
pub struct AppState {
    pub ingest: IngestService,
    pub retrieval: RetrievalService,
    pub chat: ChatService,
    pub stores: Stores,
    pub config: AppConfig,
    pub tenant_header: HeaderName,
}
```

Wrapped in `Arc<AppState>` for shared ownership across handlers and middleware. Handlers borrow through `State<Arc<AppState>>`.

### 1.2 ApiError

```rust
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}
```

`IntoResponse` implementation sets the HTTP status from `self.status` and serializes only `ErrorBody { error: self.message }` as the JSON response body. The `status` field is transport-only — it does not appear in the JSON payload.

Fallback: if JSON serialization fails, return `{"error":"internal error"}` with the original status code.

### 1.3 RequestContext and Typed Extractor

```rust
pub struct RequestContext {
    pub request_id: Uuid,
    pub tenant: TenantId,
}
```

Inserted into request extensions by the tenant extraction middleware.

Extracted in handlers via a typed `Ctx` extractor (not raw `Extension<RequestContext>`):

```rust
pub struct Ctx(pub RequestContext);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<RequestContext>()
            .cloned()
            .map(Ctx)
            .ok_or_else(|| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "missing request context".into(),
            })
    }
}
```

This gives one place to define the "missing context is a 500" behavior and keeps handler signatures clean.

---

## 2. Middleware

Two middleware layers applied to **protected routes only**, before merging with public routes. Axum executes layers bottom-to-top (last `.layer()` call runs first).

### 2.1 Request ID (`middleware/request_id.rs`)

**Signature:** `async fn request_id(req: Request, next: Next) -> Response`

Uses `from_fn` (no state needed). The header name `x-request-id` is hardcoded — not configurable. This keeps the middleware stateless.

Behavior:
1. Read the `x-request-id` header from the incoming request.
2. Attempt to parse as UUID. If missing or unparseable, generate a new UUID v4.
3. Insert the `Uuid` into request extensions.
4. Call `next.run(req)`.
5. Set `x-request-id` response header to the UUID string.

Never rejects a request — invalid or missing IDs are silently replaced.

### 2.2 Tenant Extraction (`middleware/tenant.rs`)

**Signature:** `async fn tenant_extraction(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Result<Response, ApiError>`

Uses `from_fn_with_state` (needs `state.tenant_header`).

Behavior:
1. Read the tenant header (name from `state.tenant_header`, default: `x-tenant`).
2. If missing, use `"default"`.
3. Validate via `TenantId::new(value)`. If invalid, return 400 `ApiError`.
4. Retrieve the `Uuid` request ID from extensions (inserted by request ID middleware).
5. Construct `RequestContext { request_id, tenant }` and insert into extensions.
6. Set `x-tenant` response header.
7. Call `next.run(req)`.

Depends on request ID middleware having run first.

### 2.3 Body Size Limit

Applied via `DefaultBodyLimit::max(10 * 1024 * 1024)` on the merged router (covers all routes). The `/ingest/upload` endpoint gets a route-specific override of `DefaultBodyLimit::max(50 * 1024 * 1024)`.

### 2.4 Router Assembly Order

```rust
let protected = Router::new()
    .route("/ingest", post(ingest_paths))
    .route("/ingest/upload", post(ingest_upload)
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024)))
    .route("/search/dense", post(search_dense))
    .route("/search/sparse", post(search_sparse))
    .route("/search/hybrid", post(search_hybrid))
    .route("/chat", post(chat))
    .route("/collections/:collection/stats", get(collection_stats))
    .layer(from_fn_with_state(state.clone(), tenant_extraction))
    .layer(from_fn(request_id))
    .with_state(state.clone());

let public = Router::new()
    .route("/health", get(health))
    .route("/readiness", get(readiness))
    .with_state(state.clone());

Router::new()
    .merge(public)
    .merge(protected)
    .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
```

Execution order per protected request: request ID → tenant extraction → handler.

Public routes (`/health`, `/readiness`) are not wrapped by tenant or request ID middleware.

---

## 3. Route Definitions

### 3.1 Public Routes

#### `GET /health`

Liveness check. No dependencies.

- **Response:** `200 OK` with body `"ok"` (plain text).

#### `GET /readiness`

Dependency health check. Concurrent Postgres (`SELECT 1`) + Qdrant health check, each with a 5-second timeout.

- **Success (200):**
  ```json
  { "ready": true, "checks": { "postgres": "ok", "qdrant": "ok" } }
  ```
- **Failure (503):** Per-component status with error detail:
  ```json
  { "ready": false, "checks": { "postgres": "ok", "qdrant": "timeout after 5s" } }
  ```

### 3.2 Protected Routes

#### `POST /ingest` (path-based)

**Security note:** This endpoint accepts filesystem paths on the server. It is an admin-sensitive operation. For MVP (`auth_mode=none`), this is acceptable for local dev use. Later phases will gate this behind auth and RBAC (Phase 10).

**Request:**
```json
{
  "paths": ["/absolute/path/to/file.pdf", "/absolute/path/to/directory/"],
  "collection": "optional-collection-name"
}
```

- `paths` — required, non-empty array of absolute paths. Files are ingested directly; directories are ingested recursively via `ingest_directory`.
- `collection` — optional, overrides the default collection from config.

**Response (200):**
```json
{
  "documents": 3,
  "chunks": 42,
  "skipped": 1,
  "failures": [
    { "path": "/path/to/bad.pdf", "error": "extraction failed: ..." }
  ]
}
```

`failures` is omitted when empty.

#### `POST /ingest/upload` (file upload)

**Request:** `multipart/form-data` with:
- `file` — required, the file to ingest.
- `collection` — optional form field, overrides default collection.

Route-specific body limit: 50 MB.

**Behavior:** Writes the uploaded file to a temporary file (via `tempfile` crate), calls `ingest_file` with the temp path, cleans up on completion (temp file dropped automatically).

**Response (200):** Same shape as path-based ingest, but always `"documents": 1` (single file).

#### `POST /search/dense`

**Request:**
```json
{
  "query": "search text",
  "collection": "collection-name",
  "top_k": 20
}
```

- `query` — required, non-empty.
- `collection` — required, non-empty.
- `top_k` — optional, defaults to config `dense_top_k`.

Calls `RetrievalService::search_dense(collection, query, tenant, top_k)`.

**Response (200):**
```json
{
  "results": [
    {
      "chunk_id": "abc-123",
      "document_id": "doc-abc",
      "chunk_index": 2,
      "text": "chunk content...",
      "score": 0.87
    }
  ]
}
```

#### `POST /search/sparse`

Same request/response shape as `/search/dense`. Calls `RetrievalService::search_sparse`. Default `top_k` from config `sparse_top_k`.

#### `POST /search/hybrid`

**Request:**
```json
{
  "query": "search text",
  "collection": "collection-name",
  "dense_top_k": 20,
  "sparse_top_k": 20,
  "rrf_k": 60
}
```

All override fields are optional — defaults come from config.

Calls `RetrievalService::search_hybrid(collection, query, tenant, overrides)`.

**Response (200):**
```json
{
  "results": [
    {
      "chunk_id": "abc-123",
      "document_id": "doc-abc",
      "chunk_index": 2,
      "text": "chunk content...",
      "fused_score": 0.032
    }
  ]
}
```

Note: hybrid results return `fused_score` (from RRF fusion), not `score`.

#### `POST /chat`

**Request:**
```json
{
  "query": "What does the document say about X?",
  "collection": "optional-on-resume",
  "conversation_id": null,
  "language": null,
  "history_limit": null
}
```

- `query` — required, non-empty.
- `collection` — required on first message (new conversation). Optional when resuming (`conversation_id` provided) — reuses stored collection. See Phase 6 spec Section 4.3 for collection resolution rules.
- `conversation_id` — optional UUID. If provided, resumes existing conversation.
- `language` — optional, for response language instruction.
- `history_limit` — optional, overrides default (30).

Maps tenant from `RequestContext` into `ChatRequest::tenant`. Calls `ChatService::chat()`.

**Response (200):**
```json
{
  "answer": "Based on the documents...",
  "conversation_id": "uuid",
  "citations": [
    { "chunk_id": "abc-123", "document_id": "doc-abc", "chunk_index": 1, "sources": ["dense", "sparse"] }
  ],
  "usage": { "prompt_tokens": 1200, "completion_tokens": 350 },
  "model": "gpt-4o"
}
```

#### `GET /collections/:collection/stats`

**Response (200):**
```json
{
  "collection": "hybrid_docs",
  "tenant": "default",
  "total_docs": 15,
  "total_tokens": 42000,
  "avgdl": 2800.0
}
```

Calls `Stores::get_corpus_stats(tenant, collection, default_avgdl)`. If no row exists, the store returns a zero-count struct with `avgdl` set to the configured default — this is passed through as-is. No 404; an empty collection returns `total_docs: 0, total_tokens: 0`.

---

## 4. Server Bootstrap and Shutdown (`main.rs`)

### 4.1 Startup Sequence

1. `dotenvy::dotenv().ok()` — load `.env` before anything else, so `RUST_LOG` and other env vars are available.
2. Initialize tracing subscriber with `EnvFilter` from `RUST_LOG`.
3. `AppConfig::from_env()` — load config (TOML + env vars + defaults). Errors here are fatal.
4. `Stores::new(&config).await` — connect Postgres, run migrations, connect Qdrant. Errors are fatal.
5. Construct services with explicit `stores.clone()` for each:
   - `IngestService::new(stores.clone(), &config)`
   - `RetrievalService::new(stores.clone(), &config)`
   - `ChatService::new(stores.clone(), &config)`
6. Parse `HeaderName` for tenant header from config (`config.tenant_header`). The request ID header is hardcoded to `x-request-id`.
7. Build `Arc<AppState>` with all services, stores, config, and parsed tenant header name.
8. `build_router(state)` — assemble public + protected routes with middleware.
9. `TcpListener::bind(&config.bind_addr).await` — bind to configured address.
10. Log bind address at `info` level.
11. `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await`.

Any step that returns `Err` causes `main` to exit with the error printed via `anyhow`. No partial startup states.

### 4.2 Graceful Shutdown

```rust
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
```

Cross-platform: `ctrl_c()` works everywhere, SIGTERM is unix-only with a `pending()` fallback on other platforms. Axum's built-in drain handles in-flight requests.

### 4.3 Justfile Entries

```just
run-server:
    RUST_LOG="info,rag_server=debug,rag_core=debug,tower_http=debug" \
    RAG_EMBEDDER=openai \
    cargo run --bin rag-server --target-dir target/run

run-server-mock:
    RUST_LOG="info,rag_server=debug,rag_core=debug,tower_http=debug" \
    RAG_EMBEDDER=mock \
    cargo run --bin rag-server --target-dir target/run
```

---

## 5. Mock LLM Backend (Test-Only)

### 5.1 ChatBackend::Mock Variant

Added to the existing `ChatBackend` enum in `rag-core/src/llm.rs`:

```rust
pub enum ChatBackend {
    OpenAiCompatible { client: ..., model: String },
    Anthropic { client: ..., model: String, base_url: String },
    Mock { response: String },
}
```

The `Mock` variant returns a deterministic `LlmResponse` from `complete()`:
```rust
ChatBackend::Mock { response } => Ok(LlmResponse {
    text: response.clone(),
    usage: TokenUsage { prompt_tokens: 0, completion_tokens: 0 },
    model: "mock".into(),
})
```

No network calls, no API keys, deterministic output.

### 5.2 Test-Only Constructor

The production `LlmProvider` enum remains `OpenAiCompatible | Anthropic` — no `Mock` variant in config.

`ChatBackend::Mock` is a public variant in the enum (always compiled, matching the `AnyEmbedder::Mock(MockEmbedder)` pattern). It is never constructed by config-driven code — only by test setup code.

`ChatService` gets a public constructor for injecting the mock backend:

```rust
impl ChatService {
    /// Constructs a ChatService with a mock LLM backend that returns the given response.
    /// Intended for integration tests — not selected by any config value.
    pub fn with_mock_llm(stores: Stores, config: &AppConfig, response: String) -> Result<Self> {
        let backend = ChatBackend::Mock { response };
        // ... same wiring as `new()` but with the mock backend
    }
}
```

No `#[cfg(test)]` gate — the constructor is public so integration tests in `rag-server` (a separate crate) can call it. This matches how `MockEmbedder` is public and always compiled.

---

## 6. File Layout

```
crates/rag-server/
├── Cargo.toml
└── src/
    ├── main.rs              # dotenvy, tracing, config, stores, services, router, bind, serve, shutdown_signal
    ├── state.rs             # AppState, ApiError, ErrorBody, RequestContext, Ctx extractor
    ├── router.rs            # build_router(Arc<AppState>) -> Router
    ├── middleware/
    │   ├── mod.rs           # re-exports
    │   ├── request_id.rs    # from_fn: parse/generate UUID, set response header
    │   └── tenant.rs        # from_fn_with_state: extract tenant, validate, build RequestContext
    └── routes/
        ├── mod.rs           # re-exports
        ├── health.rs        # GET /health, GET /readiness
        ├── ingest.rs        # POST /ingest, POST /ingest/upload
        ├── search.rs        # POST /search/dense, /search/sparse, /search/hybrid
        ├── chat.rs          # POST /chat
        └── collections.rs   # GET /collections/:collection/stats
```

### 6.1 Cargo.toml Dependencies (rag-server)

**Runtime:**
- `axum` (with `multipart` feature)
- `tower-http` (trace, limit)
- `tokio` (rt-multi-thread, signal, macros, fs)
- `rag-core` (workspace)
- `serde`, `serde_json`
- `uuid` (v4, serde)
- `anyhow`
- `tracing`, `tracing-subscriber` (env-filter)
- `dotenvy`
- `tempfile`

**Dev:**
- `test-support` (workspace)
- `reqwest` (json feature)

---

## 7. Integration Testing Strategy

All integration tests use mock embedder (`RAG_EMBEDDER=mock`) and mock LLM backend (via `ChatService::with_mock_llm`). No real API keys needed.

### 7.1 Test Cases (7 tests across 4 files)

| File | Test | Requires `just up` | Description |
|------|------|---------------------|-------------|
| `tests/health.rs` | `health_returns_ok` | No | `GET /health` → 200, body `"ok"`. Minimal router, no DB. |
| `tests/health.rs` | `readiness_returns_checks` | Yes | `GET /readiness` → 200 with `{ "ready": true, "checks": {...} }`. |
| `tests/health.rs` | `readiness_degrades_gracefully` | Yes | Postgres up + Qdrant unreachable → 503 with per-component status. |
| `tests/ingest.rs` | `ingest_paths_returns_counts` | Yes | `POST /ingest` with fixture file path → document/chunk counts. |
| `tests/ingest.rs` | `ingest_upload_returns_counts` | Yes | `POST /ingest/upload` multipart → same response shape. |
| `tests/search.rs` | `ingest_then_hybrid_search` | Yes | Ingest fixture → `POST /search/hybrid` → results contain ingested content. |
| `tests/e2e.rs` | `ingest_search_chat_journey` | Yes | Ingest → search → chat. Verify full journey, `x-request-id` and `x-tenant` headers on all responses. |

### 7.2 Test Setup Pattern

- **Lightweight (health only):** Build a `Router` with just the `/health` route and a dummy or no state. No external services.
- **Full (all other tests):** `AppConfig` with `RAG_EMBEDDER=mock`, real Postgres + Qdrant (via `just up`). `ChatService::with_mock_llm(...)` for chat. `spawn_app(router)` for ephemeral server.

### 7.3 What Is NOT Tested at This Phase

- Tenant isolation across two tenants (deferred to Phase 10)
- Invalid tenant header validation (unit-testable in middleware)
- Body size rejection (Axum built-in)
- Error paths for individual handler edge cases (added incrementally)

---

## 8. Divergences from projectAlpha

| Area | projectAlpha | Apex Phase 7 | Rationale |
|------|-------------|--------------|-----------|
| Endpoints | ~50 | 9 | MVP scope |
| Auth | API key, OIDC, RBAC | `auth_mode=none` only | Deferred to Phase 10 |
| Rate limiting | Global + per-tenant | None | Deferred |
| Search API | Single `/search` with `mode` field | `/search/dense`, `/search/sparse`, `/search/hybrid` | Explicit endpoints per mode |
| Ingest API | Path-based only | Path-based + file upload | Added upload for future web UI |
| Middleware | 7 layers (auth, RBAC, rate limit, license, cancellation, tracing, context headers) | 2 layers (request ID, tenant) | Slots available for later phases |
| Shutdown | Cancellation tokens, worker drain, lock release, OTEL flush | SIGINT/SIGTERM + Axum drain | No background workers yet |
| AppState | 25+ fields | 7 fields | Only what MVP needs |
| LLM mock | None | `ChatBackend::Mock` (test-only) | Enables integration tests without real LLM |

---

## 9. Out of Scope (Deferred)

- Authentication and RBAC (Phase 10)
- Rate limiting (Phase 10)
- License enforcement
- Cancellation tokens / cooperative shutdown
- Async ingest / job queue (Phase 12)
- Metrics and Prometheus endpoint
- OpenTelemetry tracing
- CORS configuration (add when frontend exists)
- Streaming responses
- Reranking in search results
- TCP keepalive tuning
