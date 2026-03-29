# Phase 7: HTTP Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the existing IngestService, RetrievalService, and ChatService into a running Axum HTTP server with 9 endpoints, tenant/request-ID middleware, and integration tests.

**Architecture:** Modular by layer — `state.rs` (shared types), `middleware/` (request ID + tenant), `routes/` (one file per endpoint group), `router.rs` (assembly), `main.rs` (bootstrap). Protected routes get middleware before merging with public routes.

**Tech Stack:** Axum 0.7, tower-http (trace + limit), tokio (signal), serde/serde_json, uuid, tempfile, dotenvy, tracing-subscriber.

**Spec:** `docs/superpowers/specs/2026-03-29-phase7-http-server-design.md`

---

## File Map

### New files (rag-server)

| File | Responsibility |
|------|---------------|
| `crates/rag-server/src/lib.rs` | Library root — exports all `pub mod` so integration tests can import |
| `crates/rag-server/src/state.rs` | `AppState`, `ApiError`, `ErrorBody`, `RequestContext`, `Ctx` extractor |
| `crates/rag-server/src/router.rs` | `build_router(Arc<AppState>) -> Router` |
| `crates/rag-server/src/middleware/mod.rs` | Re-export `request_id` and `tenant` |
| `crates/rag-server/src/middleware/request_id.rs` | Stateless request ID middleware |
| `crates/rag-server/src/middleware/tenant.rs` | Stateful tenant extraction middleware |
| `crates/rag-server/src/routes/mod.rs` | Re-export all route modules |
| `crates/rag-server/src/routes/health.rs` | `GET /health`, `GET /readiness` |
| `crates/rag-server/src/routes/ingest.rs` | `POST /ingest`, `POST /ingest/upload` |
| `crates/rag-server/src/routes/search.rs` | `POST /search/dense`, `/search/sparse`, `/search/hybrid` |
| `crates/rag-server/src/routes/chat.rs` | `POST /chat` |
| `crates/rag-server/src/routes/collections.rs` | `GET /collections/:collection/stats` |
| `crates/rag-server/tests/health.rs` | Health/readiness integration tests |
| `crates/rag-server/tests/ingest.rs` | Ingest integration tests |
| `crates/rag-server/tests/search.rs` | Search integration test |
| `crates/rag-server/tests/e2e.rs` | End-to-end journey test |
| `crates/rag-server/tests/common/mod.rs` | Shared test setup helpers |

### Modified files

| File | Change |
|------|--------|
| `Cargo.toml` | Add `multipart` to axum features, `limit` to tower-http features |
| `crates/rag-server/Cargo.toml` | Add `uuid`, `anyhow`, `tempfile`; dev-deps `test-support`, `reqwest` |
| `crates/rag-server/src/main.rs` | Replace stub with bootstrap + serve |
| `crates/rag-core/src/llm.rs` | Add `ChatBackend::Mock` variant |
| `crates/rag-core/src/chat.rs` | Add `ChatService::with_mock_llm()` constructor |
| `crates/rag-core/src/lib.rs` | Re-export new types (`CorpusStats`) |

---

### Task 1: Workspace Dependencies and Cargo.toml

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/rag-server/Cargo.toml`

- [ ] **Step 1: Add multipart and limit features to workspace deps**

In `Cargo.toml` (workspace root), change the axum, tower-http, and reqwest lines:

```toml
axum = { version = "0.7", features = ["macros", "json", "multipart"] }
tower-http = { version = "0.5", features = ["trace", "limit"] }
reqwest = { version = "0.12", default-features = false, features = [
    "json",
    "rustls-tls",
    "multipart",
] }
```

- [ ] **Step 2: Add missing deps to rag-server Cargo.toml**

Replace the full `crates/rag-server/Cargo.toml` content:

```toml
[package]
name = "rag-server"
description = "Axum REST API server for the Apex RAG pipeline"
edition.workspace = true
rust-version.workspace = true
version.workspace = true
license.workspace = true
publish.workspace = true

# Expose a library target so integration tests can import state, router, etc.
[lib]
name = "rag_server"
path = "src/lib.rs"

[[bin]]
name = "rag-server"
path = "src/main.rs"

[dependencies]
rag-core = { path = "../rag-core" }
axum.workspace = true
tokio.workspace = true
tower.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
dotenvy.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
anyhow.workspace = true
tempfile.workspace = true

[dev-dependencies]
test-support = { path = "../test-support" }
reqwest.workspace = true
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: compiles with no errors (main.rs is still a stub).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/rag-server/Cargo.toml
git commit -m "chore: add multipart, limit, and rag-server deps for Phase 7"
```

---

### Task 2: AppState, ApiError, RequestContext (`state.rs`)

**Files:**
- Create: `crates/rag-server/src/state.rs`
- Modify: `crates/rag-server/src/main.rs` (add `mod state;`)

- [ ] **Step 1: Create state.rs**

Create `crates/rag-server/src/state.rs`:

```rust
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_core::TenantId;
use serde::Serialize;
use uuid::Uuid;

/// Shared application state, wrapped in `Arc` for Axum handlers.
pub struct AppState {
    pub ingest: IngestService,
    pub retrieval: RetrievalService,
    pub chat: ChatService,
    pub stores: Stores,
    pub config: AppConfig,
    pub tenant_header: axum::http::HeaderName,
}

/// Per-request context injected by middleware.
#[derive(Clone)]
pub struct RequestContext {
    pub request_id: Uuid,
    pub tenant: TenantId,
}

/// Typed extractor for `RequestContext`. Returns 500 if middleware did not run.
pub struct Ctx(pub RequestContext);

impl<S: Send + Sync> FromRequestParts<S> for Ctx {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            parts
                .extensions
                .get::<RequestContext>()
                .cloned()
                .map(Ctx)
                .ok_or_else(|| ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "missing request context".into(),
                }),
        )
    }
}

/// Standardised JSON error response.
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: self.message };
        let json = serde_json::to_string(&body)
            .unwrap_or_else(|_| r#"{"error":"internal error"}"#.to_string());

        Response::builder()
            .status(self.status)
            .header(CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(json))
            .unwrap_or_else(|_| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(axum::body::Body::from(
                        r#"{"error":"internal error"}"#,
                    ))
                    .expect("hardcoded response must build")
            })
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("{err:#}"),
        }
    }
}
```

- [ ] **Step 2: Create lib.rs**

Create `crates/rag-server/src/lib.rs` — this is the library root that integration tests import from:

```rust
pub mod state;
```

- [ ] **Step 3: Update main.rs to use the library**

Replace `crates/rag-server/src/main.rs` with:

```rust
fn main() {
    // Server entrypoint — wired in Task 7.
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: compiles with no errors. `state` module loads via lib.rs.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-server/src/state.rs crates/rag-server/src/lib.rs crates/rag-server/src/main.rs
git commit -m "feat(server): add AppState, ApiError, RequestContext, Ctx extractor"
```

---

### Task 3: Request ID Middleware

**Files:**
- Create: `crates/rag-server/src/middleware/mod.rs`
- Create: `crates/rag-server/src/middleware/request_id.rs`
- Modify: `crates/rag-server/src/main.rs` (add `mod middleware;`)

- [ ] **Step 1: Create middleware/mod.rs**

Create `crates/rag-server/src/middleware/mod.rs`:

```rust
pub mod request_id;
pub mod tenant;
```

- [ ] **Step 2: Create middleware/request_id.rs**

Create `crates/rag-server/src/middleware/request_id.rs`:

```rust
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// Middleware that ensures every request has a UUID request ID.
///
/// Reads `x-request-id` from the incoming request. If missing or not a valid
/// UUID, generates a new v4 UUID. The ID is inserted into request extensions
/// and echoed on the response.
pub async fn request_id(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<Uuid>().ok())
        .unwrap_or_else(Uuid::new_v4);

    req.extensions_mut().insert(id);

    let mut response = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(&id.to_string()) {
        response.headers_mut().insert("x-request-id", val);
    }

    response
}
```

- [ ] **Step 3: Create tenant.rs stub** (compiles but body is next task)

Create `crates/rag-server/src/middleware/tenant.rs`:

```rust
// Tenant extraction middleware — implemented in Task 4.
```

- [ ] **Step 4: Wire module into lib.rs**

Update `crates/rag-server/src/lib.rs`:

```rust
pub mod middleware;
pub mod state;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: compiles with no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-server/src/middleware/
git add crates/rag-server/src/lib.rs
git commit -m "feat(server): add request ID middleware"
```

---

### Task 4: Tenant Extraction Middleware

**Files:**
- Modify: `crates/rag-server/src/middleware/tenant.rs`

- [ ] **Step 1: Implement tenant extraction middleware**

Replace `crates/rag-server/src/middleware/tenant.rs`:

```rust
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderValue, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use rag_core::TenantId;
use uuid::Uuid;

use crate::state::{ApiError, AppState, RequestContext};

/// Middleware that extracts and validates the tenant from the configured header.
///
/// Falls back to `"default"` if the header is absent. Returns 400 if the value
/// fails `TenantId` validation. Depends on request-ID middleware having run
/// first (reads `Uuid` from extensions).
pub async fn tenant_extraction(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let tenant_str = req
        .headers()
        .get(&state.tenant_header)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default");

    let tenant = TenantId::new(tenant_str).map_err(|e| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid tenant: {e}"),
    })?;

    let request_id = req
        .extensions()
        .get::<Uuid>()
        .copied()
        .unwrap_or_else(Uuid::new_v4);

    let ctx = RequestContext { request_id, tenant: tenant.clone() };
    req.extensions_mut().insert(ctx);

    let mut response = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(tenant.as_str()) {
        response.headers_mut().insert(state.tenant_header.clone(), val);
    }

    Ok(response)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/src/middleware/tenant.rs
git commit -m "feat(server): add tenant extraction middleware"
```

---

### Task 5: Health and Readiness Routes

**Files:**
- Create: `crates/rag-server/src/routes/mod.rs`
- Create: `crates/rag-server/src/routes/health.rs`
- Modify: `crates/rag-server/src/main.rs` (add `mod routes;`)

- [ ] **Step 1: Create routes/mod.rs**

Create `crates/rag-server/src/routes/mod.rs`:

```rust
pub mod health;
pub mod ingest;
pub mod search;
pub mod chat;
pub mod collections;
```

- [ ] **Step 2: Create routes/health.rs**

Create `crates/rag-server/src/routes/health.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::AppState;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[derive(Serialize)]
struct ReadinessResponse {
    ready: bool,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
struct ReadinessChecks {
    postgres: String,
    qdrant: String,
}

pub async fn readiness(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let timeout = Duration::from_secs(5);

    let pg_check = tokio::time::timeout(timeout, async {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(state.stores.pg_pool())
            .await
            .map(|_| "ok".to_string())
            .map_err(|e| format!("{e}"))
    });

    let qdrant_check = tokio::time::timeout(timeout, async {
        state
            .stores
            .qdrant_client()
            .health_check()
            .await
            .map(|_| "ok".to_string())
            .map_err(|e| format!("{e}"))
    });

    let (pg_result, qdrant_result) = tokio::join!(pg_check, qdrant_check);

    let pg_status = match pg_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(_) => "timeout after 5s".to_string(),
    };

    let qdrant_status = match qdrant_result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(_) => "timeout after 5s".to_string(),
    };

    let ready = pg_status == "ok" && qdrant_status == "ok";
    let status = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };

    let body = ReadinessResponse {
        ready,
        checks: ReadinessChecks {
            postgres: pg_status,
            qdrant: qdrant_status,
        },
    };

    (status, axum::Json(body))
}
```

- [ ] **Step 3: Create stub files for remaining route modules**

Create `crates/rag-server/src/routes/ingest.rs`:
```rust
// Ingest routes — implemented in Task 8.
```

Create `crates/rag-server/src/routes/search.rs`:
```rust
// Search routes — implemented in Task 9.
```

Create `crates/rag-server/src/routes/chat.rs`:
```rust
// Chat route — implemented in Task 10.
```

Create `crates/rag-server/src/routes/collections.rs`:
```rust
// Collection stats route — implemented in Task 11.
```

- [ ] **Step 4: Wire module into lib.rs**

Update `crates/rag-server/src/lib.rs`:

```rust
pub mod middleware;
pub mod routes;
pub mod state;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: compiles. The `sqlx::query_scalar` in readiness requires the `sqlx` dependency. Since `rag-core` re-exports `sqlx` types via `Stores::pg_pool()` returning `&PgPool`, we need sqlx as a direct dep. Add `sqlx.workspace = true` to `crates/rag-server/Cargo.toml` under `[dependencies]` if the check fails.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-server/src/routes/ crates/rag-server/src/lib.rs
git commit -m "feat(server): add health and readiness routes"
```

---

### Task 6: Router Assembly

**Files:**
- Create: `crates/rag-server/src/router.rs`
- Modify: `crates/rag-server/src/main.rs` (add `mod router;`)

- [ ] **Step 1: Create router.rs**

Create `crates/rag-server/src/router.rs`:

```rust
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};

use crate::middleware::request_id::request_id;
use crate::middleware::tenant::tenant_extraction;
use crate::routes;
use crate::state::AppState;

/// Build the full application router.
///
/// Public routes (`/health`, `/readiness`) have no middleware.
/// Protected routes get request-ID and tenant-extraction middleware.
pub fn build_router(state: Arc<AppState>) -> Router {
    let protected = Router::new()
        .route("/ingest", post(routes::ingest::ingest_paths))
        .route(
            "/ingest/upload",
            post(routes::ingest::ingest_upload)
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/search/dense", post(routes::search::search_dense))
        .route("/search/sparse", post(routes::search::search_sparse))
        .route("/search/hybrid", post(routes::search::search_hybrid))
        .route("/chat", post(routes::chat::chat))
        .route(
            "/collections/:collection/stats",
            get(routes::collections::collection_stats),
        )
        .layer(from_fn_with_state(state.clone(), tenant_extraction))
        .layer(from_fn(request_id))
        .with_state(state.clone());

    let public = Router::new()
        .route("/health", get(routes::health::health))
        .route("/readiness", get(routes::health::readiness))
        .with_state(state);

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
}
```

- [ ] **Step 2: Wire module into lib.rs**

Update `crates/rag-server/src/lib.rs`:

```rust
pub mod middleware;
pub mod router;
pub mod routes;
pub mod state;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: may fail because route handler stubs don't export the expected function names yet. This is expected — the router references functions that will be implemented in Tasks 8-11. If it fails, that's fine; it will compile once all route handlers exist. Move on.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-server/src/router.rs crates/rag-server/src/main.rs
git commit -m "feat(server): add router assembly with middleware layering"
```

---

### Task 7: Bootstrap main.rs and Shutdown

**Files:**
- Modify: `crates/rag-server/src/main.rs`

- [ ] **Step 1: Implement main.rs**

Replace `crates/rag-server/src/main.rs`:

```rust
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};
use rag_server::router;
use rag_server::state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = AppConfig::from_env().context("loading config")?;
    tracing::info!(bind = %config.bind_addr, "starting server");

    let stores = Stores::new(&config).await.context("connecting stores")?;

    let ingest =
        IngestService::new(stores.clone(), &config).context("building ingest service")?;
    let retrieval =
        RetrievalService::new(stores.clone(), &config).context("building retrieval service")?;
    let chat =
        ChatService::new(stores.clone(), &config).context("building chat service")?;

    let tenant_header = config
        .tenant_header
        .parse()
        .context("parsing tenant header name")?;

    let state = Arc::new(AppState {
        ingest,
        retrieval,
        chat,
        stores,
        config,
        tenant_header,
    });

    let app = router::build_router(state);

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .context("binding listener")?;

    tracing::info!(addr = %listener.local_addr()?, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("running server")?;

    Ok(())
}

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

    tracing::info!("shutdown signal received");
}
```

**Important:** The `state` variable is moved into `build_router`, so `listener` must bind using the address before moving `state`. Fix by extracting `bind_addr` first:

```rust
    let bind_addr = state.config.bind_addr.clone();
    let app = router::build_router(state);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .context("binding listener")?;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: will fail if route handler stubs don't export functions yet. This is expected — the full compilation check happens after Tasks 8-11.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/src/main.rs
git commit -m "feat(server): implement main bootstrap and graceful shutdown"
```

---

### Task 8: Ingest Routes

**Files:**
- Modify: `crates/rag-server/src/routes/ingest.rs`

- [ ] **Step 1: Implement ingest routes**

Replace `crates/rag-server/src/routes/ingest.rs`:

```rust
use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;
use rag_core::{IngestService, TenantId};
use rag_core::ingest::{IngestBatchOutcome, IngestDirectoryRequest, IngestFileRequest};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct IngestPathsRequest {
    pub paths: Vec<String>,
    pub collection: Option<String>,
}

#[derive(Serialize)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailureEntry>,
}

#[derive(Serialize)]
pub struct FailureEntry {
    pub path: String,
    pub error: String,
}

pub async fn ingest_paths(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<IngestPathsRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    if payload.paths.is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "paths must not be empty".into(),
        });
    }

    let mut total = IngestBatchOutcome {
        documents: 0,
        chunks: 0,
        skipped: 0,
        failures: Vec::new(),
    };

    for path_str in &payload.paths {
        let path = std::path::PathBuf::from(path_str);
        if path.is_dir() {
            let req = IngestDirectoryRequest {
                path,
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
            };
            match state.ingest.ingest_directory(req).await {
                Ok(outcome) => {
                    total.documents += outcome.documents;
                    total.chunks += outcome.chunks;
                    total.skipped += outcome.skipped;
                    total.failures.extend(outcome.failures);
                }
                Err(e) => {
                    total.failures.push(rag_core::ingest::DocumentFailure {
                        path: std::path::PathBuf::from(path_str),
                        error: format!("{e:#}"),
                    });
                }
            }
        } else {
            let req = IngestFileRequest {
                path,
                tenant: ctx.tenant.clone(),
                collection_override: payload.collection.clone(),
            };
            match state.ingest.ingest_file(req).await {
                Ok(outcome) => {
                    total.documents += 1;
                    total.chunks += outcome.chunks_created;
                    if outcome.skipped {
                        total.skipped += 1;
                    }
                }
                Err(e) => {
                    total.failures.push(rag_core::ingest::DocumentFailure {
                        path: std::path::PathBuf::from(path_str),
                        error: format!("{e:#}"),
                    });
                }
            }
        }
    }

    let failures = total
        .failures
        .into_iter()
        .map(|f| FailureEntry {
            path: f.path.display().to_string(),
            error: f.error,
        })
        .collect();

    Ok(Json(IngestResponse {
        documents: total.documents,
        chunks: total.chunks,
        skipped: total.skipped,
        failures,
    }))
}

pub async fn ingest_upload(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<IngestResponse>, ApiError> {
    let mut file_data: Option<(String, Vec<u8>)> = None;
    let mut collection: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError {
            status: StatusCode::BAD_REQUEST,
            message: format!("multipart error: {e}"),
        })?
    {
        match field.name() {
            Some("file") => {
                let filename = field
                    .file_name()
                    .unwrap_or("upload")
                    .to_string();
                let bytes = field.bytes().await.map_err(|e| ApiError {
                    status: StatusCode::BAD_REQUEST,
                    message: format!("failed to read file: {e}"),
                })?;
                file_data = Some((filename, bytes.to_vec()));
            }
            Some("collection") => {
                let text = field.text().await.map_err(|e| ApiError {
                    status: StatusCode::BAD_REQUEST,
                    message: format!("failed to read collection field: {e}"),
                })?;
                if !text.is_empty() {
                    collection = Some(text);
                }
            }
            _ => {} // ignore unknown fields
        }
    }

    let (filename, bytes) = file_data.ok_or_else(|| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: "missing 'file' field in multipart body".into(),
    })?;

    // Determine file extension from the original filename.
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");

    let tmp = NamedTempFile::with_suffix(&format!(".{extension}")).map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to create temp file: {e}"),
    })?;

    tokio::fs::write(tmp.path(), &bytes).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to write temp file: {e}"),
    })?;

    let req = IngestFileRequest {
        path: tmp.path().to_path_buf(),
        tenant: ctx.tenant,
        collection_override: collection,
    };

    let outcome = state.ingest.ingest_file(req).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("{e:#}"),
    })?;

    Ok(Json(IngestResponse {
        documents: 1,
        chunks: outcome.chunks_created,
        skipped: if outcome.skipped { 1 } else { 0 },
        failures: Vec::new(),
    }))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server`
Expected: may still fail until all route stubs are filled.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/src/routes/ingest.rs
git commit -m "feat(server): add ingest path and upload routes"
```

---

### Task 9: Search Routes

**Files:**
- Modify: `crates/rag-server/src/routes/search.rs`

- [ ] **Step 1: Implement search routes**

Replace `crates/rag-server/src/routes/search.rs`:

```rust
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rag_core::retrieval::HybridOverrides;
use serde::{Deserialize, Serialize};

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    pub top_k: Option<u64>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

fn validate_search(req: &SearchRequest) -> Result<(), ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if req.collection.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }
    Ok(())
}

pub async fn search_dense(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    validate_search(&payload)?;
    let top_k = payload.top_k.unwrap_or(state.config.dense_top_k);
    let chunks = state
        .retrieval
        .search_dense(&payload.collection, &payload.query, ctx.tenant.as_str(), top_k)
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| SearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            score: c.score,
        })
        .collect();

    Ok(Json(SearchResponse { results }))
}

pub async fn search_sparse(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    validate_search(&payload)?;
    let top_k = payload.top_k.unwrap_or(state.config.sparse_top_k);
    let chunks = state
        .retrieval
        .search_sparse(&payload.collection, &payload.query, ctx.tenant.as_str(), top_k)
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| SearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            score: c.score,
        })
        .collect();

    Ok(Json(SearchResponse { results }))
}

#[derive(Deserialize)]
pub struct HybridSearchRequest {
    pub query: String,
    pub collection: String,
    pub dense_top_k: Option<u64>,
    pub sparse_top_k: Option<u64>,
    pub rrf_k: Option<u32>,
}

#[derive(Serialize)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Serialize)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}

pub async fn search_hybrid(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<HybridSearchRequest>,
) -> Result<Json<HybridSearchResponse>, ApiError> {
    if payload.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }
    if payload.collection.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "collection must not be empty".into(),
        });
    }

    let overrides = if payload.dense_top_k.is_some()
        || payload.sparse_top_k.is_some()
        || payload.rrf_k.is_some()
    {
        Some(HybridOverrides {
            dense_top_k: payload.dense_top_k,
            sparse_top_k: payload.sparse_top_k,
            rrf_k: payload.rrf_k,
        })
    } else {
        None
    };

    let chunks = state
        .retrieval
        .search_hybrid(
            &payload.collection,
            &payload.query,
            ctx.tenant.as_str(),
            overrides,
        )
        .await
        .map_err(ApiError::from)?;

    let results = chunks
        .into_iter()
        .map(|c| HybridSearchResult {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            fused_score: c.fused_score,
        })
        .collect();

    Ok(Json(HybridSearchResponse { results }))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server`

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/src/routes/search.rs
git commit -m "feat(server): add dense, sparse, and hybrid search routes"
```

---

### Task 10: Chat Route

**Files:**
- Modify: `crates/rag-server/src/routes/chat.rs`

- [ ] **Step 1: Implement chat route**

Replace `crates/rag-server/src/routes/chat.rs`:

```rust
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rag_core::chat::ChatRequest;
use rag_core::context::Citation;
use rag_core::llm::TokenUsage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{ApiError, AppState, Ctx};

#[derive(Deserialize)]
pub struct ChatHttpRequest {
    pub query: String,
    pub collection: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub language: Option<String>,
    pub history_limit: Option<i64>,
}

#[derive(Serialize)]
pub struct ChatHttpResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<CitationJson>,
    pub usage: UsageJson,
    pub model: String,
}

#[derive(Serialize)]
pub struct CitationJson {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Serialize)]
pub struct UsageJson {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

pub async fn chat(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatHttpRequest>,
) -> Result<Json<ChatHttpResponse>, ApiError> {
    if payload.query.trim().is_empty() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "query must not be empty".into(),
        });
    }

    let request = ChatRequest {
        query: payload.query,
        collection: payload.collection,
        tenant: ctx.tenant,
        conversation_id: payload.conversation_id,
        language: payload.language,
        history_limit: payload.history_limit,
    };

    let response = state.chat.chat(request).await.map_err(ApiError::from)?;

    let citations = response
        .citations
        .into_iter()
        .map(|c| CitationJson {
            chunk_id: c.chunk_id,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            sources: c.sources,
        })
        .collect();

    Ok(Json(ChatHttpResponse {
        answer: response.answer,
        conversation_id: response.conversation_id,
        citations,
        usage: UsageJson {
            prompt_tokens: response.usage.prompt_tokens,
            completion_tokens: response.usage.completion_tokens,
        },
        model: response.model,
    }))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server`

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/src/routes/chat.rs
git commit -m "feat(server): add chat route"
```

---

### Task 11: Collection Stats Route

**Files:**
- Modify: `crates/rag-server/src/routes/collections.rs`
- Modify: `crates/rag-core/src/lib.rs` (re-export `CorpusStats`)

- [ ] **Step 1: Re-export CorpusStats from rag-core**

In `crates/rag-core/src/lib.rs`, add to the existing `pub use` block:

```rust
pub use stores::corpus_stats::CorpusStats;
```

- [ ] **Step 2: Implement collection stats route**

Replace `crates/rag-server/src/routes/collections.rs`:

```rust
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use crate::state::{ApiError, AppState, Ctx};

#[derive(Serialize)]
pub struct CollectionStatsResponse {
    pub collection: String,
    pub tenant: String,
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}

pub async fn collection_stats(
    Ctx(ctx): Ctx,
    State(state): State<Arc<AppState>>,
    Path(collection): Path<String>,
) -> Result<Json<CollectionStatsResponse>, ApiError> {
    let stats = state
        .stores
        .get_corpus_stats(ctx.tenant.as_str(), &collection, state.config.bm25_avgdl)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(CollectionStatsResponse {
        collection,
        tenant: ctx.tenant.as_str().to_string(),
        total_docs: stats.total_docs,
        total_tokens: stats.total_tokens,
        avgdl: stats.avgdl,
    }))
}
```

- [ ] **Step 3: Verify the full server compiles**

Run: `cargo check -p rag-server`
Expected: all modules compile. This is the first time the full crate should compile end-to-end.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p rag-server -- -D warnings -D clippy::disallowed_methods`
Expected: clean. Fix any warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-core/src/lib.rs crates/rag-server/src/routes/collections.rs
git commit -m "feat(server): add collection stats route and CorpusStats re-export"
```

---

### Task 12: Justfile Run Commands

**Files:**
- Modify: `Justfile`

- [ ] **Step 1: Verify existing run commands work**

The Justfile already has `run-server` and `run-server-mock` commands. Check that the current `main.rs` (now with `#[tokio::main]`) will work with them.

Run: `just run-server-mock`
Expected: server starts, prints listening address. If Postgres/Qdrant are not running it will fail at Stores::new — that's expected without `just up`. Ctrl-C to stop.

If `just up` is running:
Expected: `INFO listening addr=0.0.0.0:8080`

- [ ] **Step 2: Commit** (only if Justfile needed changes)

If no changes needed, skip this step.

---

### Task 13: Mock LLM Backend

**Files:**
- Modify: `crates/rag-core/src/llm.rs`
- Modify: `crates/rag-core/src/chat.rs`

- [ ] **Step 1: Add ChatBackend::Mock variant to llm.rs**

In `crates/rag-core/src/llm.rs`, add a `Mock` variant to the `ChatBackend` enum:

```rust
pub enum ChatBackend {
    OpenAiCompatible {
        client: Box<async_openai::Client<Box<dyn OpenAiConfigTrait>>>,
        model: String,
    },
    Anthropic {
        client: reqwest::Client,
        model: String,
        base_url: String,
    },
    /// Deterministic mock backend for integration tests.
    /// Returns the configured response string on every `complete()` call.
    Mock {
        response: String,
    },
}
```

- [ ] **Step 2: Handle Mock in complete()**

In the `complete()` method's provider_name match and the `result` match, add the Mock arm. Update the `provider_name` match:

```rust
let provider_name = match self {
    Self::OpenAiCompatible { .. } => "OpenAI-compatible completion",
    Self::Anthropic { .. } => "Anthropic completion",
    Self::Mock { .. } => "Mock completion",
};
```

Add a `Mock` arm before the retry loop (early return):

```rust
// Mock backend returns immediately, no retry logic needed.
if let Self::Mock { response } = self {
    return Ok(LlmResponse {
        text: response.clone(),
        usage: TokenUsage { prompt_tokens: 0, completion_tokens: 0 },
        model: "mock".into(),
    });
}
```

Place this right after the `provider_name` assignment and before `for attempt in 0..=max_retries`.

- [ ] **Step 3: Add ChatService::with_mock_llm() to chat.rs**

In `crates/rag-core/src/chat.rs`, add a second constructor to the `impl ChatService` block:

```rust
    /// Construct a ChatService with a mock LLM backend.
    ///
    /// Intended for integration tests in downstream crates. The mock backend
    /// returns `response` verbatim on every chat call, with no network IO.
    pub fn with_mock_llm(
        stores: Stores,
        config: &AppConfig,
        response: String,
    ) -> Result<Self> {
        let backend = ChatBackend::Mock { response };
        let retrieval =
            RetrievalService::new(stores.clone(), config).context("building retrieval service")?;
        let context_builder = ContextBuilder::new();
        let prompt_renderer = PromptRenderer::from_file(&config.llm_prompt_template_path)
            .context("loading prompt template")?;
        let defaults = ChatDefaults::from_config(config);

        Ok(Self { backend, retrieval, context_builder, prompt_renderer, stores, defaults })
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p rag-core`
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-core/src/llm.rs crates/rag-core/src/chat.rs
git commit -m "feat(core): add ChatBackend::Mock and ChatService::with_mock_llm()"
```

---

### Task 14: Test Helpers (common/mod.rs)

**Files:**
- Create: `crates/rag-server/tests/common/mod.rs`

- [ ] **Step 1: Create shared test setup**

Create `crates/rag-server/tests/common/mod.rs`:

```rust
use std::sync::Arc;

use axum::Router;
use rag_core::{AppConfig, ChatService, IngestService, RetrievalService, Stores};

use rag_server::state::AppState;
use rag_server::router::build_router;

/// Build a full AppState with mock embedder and mock LLM, backed by real
/// Postgres and Qdrant (requires `just up`).
pub async fn full_app() -> (Router, Arc<AppState>) {
    // SAFETY: test-only env manipulation; each integration test runs in its
    // own process so there are no data races with other threads reading env.
    unsafe { std::env::set_var("RAG_EMBEDDER", "mock") };
    let config = AppConfig::from_env().expect("test config");
    let stores = Stores::new(&config).await.expect("test stores");
    let ingest = IngestService::new(stores.clone(), &config).expect("test ingest");
    let retrieval = RetrievalService::new(stores.clone(), &config).expect("test retrieval");
    let chat =
        ChatService::with_mock_llm(stores.clone(), &config, "Mock LLM response.".into())
            .expect("test chat");
    let tenant_header = config.tenant_header.parse().expect("tenant header");
    let state = Arc::new(AppState {
        ingest,
        retrieval,
        chat,
        stores,
        config,
        tenant_header,
    });
    let router = build_router(state.clone());
    (router, state)
}
```

**Note:** This works because `rag-server` has a `lib.rs` exporting all modules as `pub mod`. Integration tests import from `rag_server::state`, `rag_server::router`, etc.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rag-server --tests`
Expected: compiles (but tests don't run yet).

- [ ] **Step 4: Commit**

```bash
git add crates/rag-server/tests/common/
git commit -m "feat(server): add shared test helpers for integration tests"
```

---

### Task 15: Health Integration Tests

**Files:**
- Create: `crates/rag-server/tests/health.rs`

- [ ] **Step 1: Write health tests**

Create `crates/rag-server/tests/health.rs`:

```rust
mod common;

use axum::routing::get;
use axum::Router;
use test_support::spawn_app;

/// GET /health returns 200 with body "ok". No database needed.
#[tokio::test]
async fn health_returns_ok() {
    let app = Router::new().route("/health", get(rag_server::routes::health::health));
    let server = spawn_app(app).await.expect("spawn");

    let resp = reqwest::get(format!("{}/health", server.base_url()))
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.expect("body"), "ok");
}

/// GET /readiness returns 200 with checks when services are up.
#[tokio::test]
#[ignore] // requires `just up`
async fn readiness_returns_checks() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let resp = reqwest::get(format!("{}/readiness", server.base_url()))
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["ready"], true);
    assert_eq!(body["checks"]["postgres"], "ok");
    assert_eq!(body["checks"]["qdrant"], "ok");
}

/// GET /readiness returns 503 when a dependency is unreachable.
///
/// Strategy: build AppState with the real Qdrant, then make the readiness
/// handler hit a bad Qdrant endpoint by overriding the Qdrant client's
/// target after construction. Since `Stores` validates the connection at
/// construction time, we cannot simply pass a bad URL. Instead, this test
/// stops the Qdrant container, calls readiness, and restarts it.
///
/// If stopping containers is impractical in your CI, skip this test.
#[tokio::test]
#[ignore] // requires `just up`; stops and restarts Qdrant container
async fn readiness_degrades_gracefully() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    // Stop Qdrant container.
    let _ = std::process::Command::new("docker")
        .args(["compose", "stop", "qdrant"])
        .status();

    let resp = reqwest::get(format!("{}/readiness", server.base_url()))
        .await
        .expect("request");

    // Restart Qdrant so other tests aren't affected.
    let _ = std::process::Command::new("docker")
        .args(["compose", "start", "qdrant"])
        .status();

    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["ready"], false);
    assert_eq!(body["checks"]["postgres"], "ok");
    assert_ne!(body["checks"]["qdrant"], "ok");
}
```

- [ ] **Step 2: Run health_returns_ok (no infra needed)**

Run: `cargo test -p rag-server health_returns_ok -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Run readiness_returns_checks (requires just up)**

Run: `cargo test -p rag-server readiness_returns_checks -- --ignored --nocapture`
Expected: PASS if `just up` is running.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-server/tests/health.rs
git commit -m "test(server): add health and readiness integration tests"
```

---

### Task 16: Ingest Integration Tests

**Files:**
- Create: `crates/rag-server/tests/ingest.rs`

- [ ] **Step 1: Create a test fixture file**

Create `crates/rag-server/tests/fixtures/sample.txt`:

```
This is a sample document for integration testing.
It contains enough text to produce at least one chunk.
The quick brown fox jumps over the lazy dog.
Apex is a clean-room rebuild of projectAlpha.
```

- [ ] **Step 2: Write ingest tests**

Create `crates/rag-server/tests/ingest.rs`:

```rust
mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_paths_returns_counts() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", "test-ingest")
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": "test-ingest-collection"
        }))
        .send()
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(body["documents"].as_u64().unwrap_or(0) >= 1);
    assert!(body["chunks"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_upload_returns_counts() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.txt");
    let file_bytes = std::fs::read(&fixture).expect("read fixture");

    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(file_bytes)
                .file_name("sample.txt")
                .mime_str("text/plain")
                .expect("mime"),
        )
        .text("collection", "test-upload-collection");

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/ingest/upload", server.base_url()))
        .header("x-tenant", "test-upload")
        .multipart(form)
        .send()
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["documents"].as_u64().unwrap_or(0), 1);
    assert!(body["chunks"].as_u64().unwrap_or(0) >= 1);
}
```

- [ ] **Step 3: Run ingest tests**

Run: `cargo test -p rag-server ingest -- --ignored --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-server/tests/ingest.rs crates/rag-server/tests/fixtures/
git commit -m "test(server): add ingest path and upload integration tests"
```

---

### Task 17: Search Integration Test

**Files:**
- Create: `crates/rag-server/tests/search.rs`

- [ ] **Step 1: Write search test**

Create `crates/rag-server/tests/search.rs`:

```rust
mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_then_hybrid_search() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();

    // Ingest first.
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", "test-search")
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": "test-search-collection"
        }))
        .send()
        .await
        .expect("ingest request");
    assert_eq!(resp.status(), 200);

    // Search.
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", "test-search")
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": "test-search-collection"
        }))
        .send()
        .await
        .expect("search request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "search should return at least one result");
}
```

- [ ] **Step 2: Run search test**

Run: `cargo test -p rag-server ingest_then_hybrid_search -- --ignored --nocapture`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/tests/search.rs
git commit -m "test(server): add hybrid search integration test"
```

---

### Task 18: End-to-End Journey Test

**Files:**
- Create: `crates/rag-server/tests/e2e.rs`

- [ ] **Step 1: Write e2e test**

Create `crates/rag-server/tests/e2e.rs`:

```rust
mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_search_chat_journey() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let collection = "test-e2e-collection";
    let tenant = "test-e2e";

    // --- Step 1: Ingest ---
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": collection
        }))
        .send()
        .await
        .expect("ingest");

    assert_eq!(resp.status(), 200);
    // Verify x-request-id is present on response.
    assert!(resp.headers().get("x-request-id").is_some());
    // Verify x-tenant is echoed.
    assert_eq!(
        resp.headers().get("x-tenant").and_then(|v| v.to_str().ok()),
        Some(tenant)
    );

    // --- Step 2: Search ---
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", tenant)
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": collection
        }))
        .send()
        .await
        .expect("search");

    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("x-request-id").is_some());
    let search_body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        !search_body["results"].as_array().expect("array").is_empty(),
        "search should return results after ingest"
    );

    // --- Step 3: Chat ---
    let resp = client
        .post(format!("{}/chat", server.base_url()))
        .header("x-tenant", tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": collection
        }))
        .send()
        .await
        .expect("chat");

    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("x-request-id").is_some());
    let chat_body: serde_json::Value = resp.json().await.expect("json");
    assert!(!chat_body["answer"].as_str().unwrap_or("").is_empty());
    assert!(chat_body["conversation_id"].as_str().is_some());
    assert_eq!(chat_body["model"], "mock");
}
```

- [ ] **Step 2: Run e2e test**

Run: `cargo test -p rag-server ingest_search_chat_journey -- --ignored --nocapture`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-server/tests/e2e.rs
git commit -m "test(server): add end-to-end ingest → search → chat journey test"
```

---

### Task 19: Final Verification

**Files:** None (verification only).

- [ ] **Step 1: Run cargo check**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 2: Run cargo fmt**

Run: `cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 3: Run cargo clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings -D clippy::disallowed_methods`
Expected: clean.

- [ ] **Step 4: Run non-ignored tests**

Run: `cargo test -p rag-server`
Expected: `health_returns_ok` passes. Ignored tests are skipped.

- [ ] **Step 5: Run ignored tests (requires just up)**

Run: `cargo test -p rag-server -- --ignored --nocapture`
Expected: all 7 tests pass.

- [ ] **Step 6: Manual smoke test**

Run: `just up && just run-server-mock`
Expected: server starts, prints `listening addr=0.0.0.0:8080`.

Test with curl:
```bash
curl http://localhost:8080/health
# Expected: ok

curl http://localhost:8080/readiness
# Expected: {"ready":true,"checks":{"postgres":"ok","qdrant":"ok"}}
```

Ctrl-C the server.

- [ ] **Step 7: Commit any final fixes**

If any step required fixes, commit them:

```bash
git add -A
git commit -m "chore: final Phase 7 fixes from verification"
```
