# Phase 8: HTTP Client and CLI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `rag-client` (HTTP client library with tenant header injection) and `rag-cli` (CLI binary with ingest, search, chat, and collection-stats subcommands) for the Apex RAG server.

**Architecture:** Client-first, CLI second. `rag-client` wraps `reqwest` with a `TenantApiClient` struct, `ApiClient` trait, and typed request/response types matching the Phase 7 server contracts. `rag-cli` is a thin Clap binary dispatching to handlers generic over `impl ApiClient`, with human/JSON output via a `Write`-based output module.

**Tech Stack:** Rust 2024, reqwest 0.12, clap 4.5, tokio 1.37, trait-variant, thiserror 2.0, anyhow 1.0, uuid 1, serde/serde_json, percent-encoding, test-support (workspace)

**Spec:** `docs/superpowers/specs/2026-03-29-phase8-http-client-cli-design.md`

**Beads issue:** apex-1c6

---

## File Structure

### rag-client (new files)

| File | Responsibility |
|------|---------------|
| `crates/rag-client/src/lib.rs` | Re-exports, module declarations |
| `crates/rag-client/src/tenant_id.rs` | `TenantId` newtype with validation |
| `crates/rag-client/src/error.rs` | `ClientError` enum |
| `crates/rag-client/src/types.rs` | All request/response types |
| `crates/rag-client/src/client.rs` | `TenantApiClient` struct, generic helpers, typed methods |
| `crates/rag-client/src/trait_def.rs` | `ApiClient` trait definition |
| `crates/rag-client/Cargo.toml` | Dependencies (modify) |
| `crates/rag-client/tests/client_tests.rs` | Integration tests with `spawn_app` |

### rag-cli (new files)

| File | Responsibility |
|------|---------------|
| `crates/rag-cli/src/main.rs` | Tokio entry, parse args, dispatch |
| `crates/rag-cli/src/cli.rs` | Clap `Cli` struct and `Command` enum |
| `crates/rag-cli/src/output.rs` | `print_or_json` helper |
| `crates/rag-cli/src/commands/mod.rs` | Module declarations |
| `crates/rag-cli/src/commands/ingest.rs` | Ingest handler |
| `crates/rag-cli/src/commands/search.rs` | Search handler |
| `crates/rag-cli/src/commands/chat.rs` | Chat handler |
| `crates/rag-cli/src/commands/collections.rs` | Collection-stats handler |
| `crates/rag-cli/Cargo.toml` | Dependencies (modify) |

### Workspace

| File | Change |
|------|--------|
| `Cargo.toml` | Add `trait-variant` and `percent-encoding` to workspace deps |

---

### Task 1: Workspace dependencies and rag-client Cargo.toml

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/rag-client/Cargo.toml`
- Modify: `crates/rag-cli/Cargo.toml`

- [ ] **Step 1: Add trait-variant and percent-encoding to workspace deps**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
trait-variant = "0.1"
percent-encoding = "2.3"
```

- [ ] **Step 2: Update rag-client Cargo.toml**

Replace the entire `[dependencies]` section in `crates/rag-client/Cargo.toml` with:

```toml
[dependencies]
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
uuid.workspace = true
trait-variant.workspace = true
percent-encoding.workspace = true

[dev-dependencies]
tokio.workspace = true
axum.workspace = true
test-support = { path = "../test-support" }
```

- [ ] **Step 3: Update rag-cli Cargo.toml**

Replace the entire `[dependencies]` section in `crates/rag-cli/Cargo.toml` with:

```toml
[dependencies]
rag-client = { path = "../rag-client" }
clap.workspace = true
tokio.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true

[dev-dependencies]
reqwest.workspace = true
tempfile.workspace = true
```

- [ ] **Step 4: Verify workspace compiles**

Run: `cargo check -p rag-client -p rag-cli`
Expected: PASS (empty lib/main, deps resolve)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rag-client/Cargo.toml crates/rag-cli/Cargo.toml
git commit -m "chore: add Phase 8 dependencies for rag-client and rag-cli"
```

---

### Task 2: TenantId newtype

**Files:**
- Create: `crates/rag-client/src/tenant_id.rs`
- Modify: `crates/rag-client/src/lib.rs`

- [ ] **Step 1: Write the TenantId tests and implementation**

Create `crates/rag-client/src/tenant_id.rs`:

```rust
//! Tenant identity type with validation (client-local).
//!
//! Mirrors the validation rules of `rag_core::TenantId` but is defined
//! independently so `rag-client` has no workspace dependencies.

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

/// Maximum length for a tenant identifier.
const MAX_TENANT_LENGTH: usize = 128;

/// A validated tenant identifier.
///
/// Guarantees: non-empty, max 128 chars, ASCII alphanumeric + `-` + `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Create a new `TenantId`, validating the input.
    pub fn new(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        if s.is_empty() {
            return Err("tenant ID must not be empty".into());
        }
        if s.len() > MAX_TENANT_LENGTH {
            return Err(format!("tenant ID exceeds {MAX_TENANT_LENGTH} characters"));
        }
        for (i, c) in s.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
                return Err(format!("invalid character '{c}' at position {i}"));
            }
        }
        Ok(Self(s))
    }

    /// Get the tenant ID as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TenantId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Deref for TenantId {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for TenantId {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids() {
        for id in ["default", "my-tenant", "tenant_123", "A"] {
            assert!(TenantId::new(id).is_ok(), "expected {id:?} to be valid");
        }
    }

    #[test]
    fn empty_rejected() {
        assert!(TenantId::new("").is_err());
    }

    #[test]
    fn too_long_rejected() {
        assert!(TenantId::new("a".repeat(129)).is_err());
        assert!(TenantId::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn invalid_chars_rejected() {
        assert!(TenantId::new("has spaces").is_err());
        assert!(TenantId::new("has/slash").is_err());
        assert!(TenantId::new("has.dot").is_err());
    }

    #[test]
    fn display_and_deref() {
        let t = TenantId::new("acme").unwrap();
        assert_eq!(format!("{t}"), "acme");
        assert_eq!(t.as_str(), "acme");
        assert!(t.starts_with("ac"));
    }
}
```

- [ ] **Step 2: Update lib.rs to declare the module**

Replace contents of `crates/rag-client/src/lib.rs` with:

```rust
//! HTTP client for the Apex RAG server API.
//!
//! Provides `TenantApiClient` for tenant-scoped API operations.

mod tenant_id;

pub use tenant_id::TenantId;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rag-client`
Expected: All TenantId tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rag-client/src/tenant_id.rs crates/rag-client/src/lib.rs
git commit -m "feat(rag-client): add TenantId newtype with validation"
```

---

### Task 3: ClientError enum

**Files:**
- Create: `crates/rag-client/src/error.rs`
- Modify: `crates/rag-client/src/lib.rs`

- [ ] **Step 1: Create the error module**

Create `crates/rag-client/src/error.rs`:

```rust
//! Error types for the RAG client.

/// Errors returned by `TenantApiClient` operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The provided base URL could not be parsed.
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),

    /// The provided tenant identifier failed validation.
    #[error("invalid tenant: {0}")]
    InvalidTenant(String),

    /// A network-level failure (connection refused, timeout, DNS, etc.).
    #[error("transport error: {0}")]
    Transport(reqwest::Error),

    /// The server returned a non-2xx status code.
    #[error("server returned {status}: {body}")]
    HttpStatus {
        status: u16,
        url: String,
        body: String,
    },

    /// The response body could not be decoded (invalid JSON, etc.).
    #[error("failed to decode response: {0}")]
    Decode(reqwest::Error),

    /// A client-side validation check failed before sending the request.
    #[error("{0}")]
    Validation(String),
}

impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        ClientError::Transport(err)
    }
}
```

Note: `From<reqwest::Error>` maps to `Transport` by default. The `Decode` variant is constructed explicitly in the response-handling code to distinguish decode failures from send failures.

- [ ] **Step 2: Update lib.rs**

Add to `crates/rag-client/src/lib.rs`:

```rust
mod error;

pub use error::ClientError;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rag-client`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/rag-client/src/error.rs crates/rag-client/src/lib.rs
git commit -m "feat(rag-client): add ClientError enum"
```

---

### Task 4: Request/Response types

**Files:**
- Create: `crates/rag-client/src/types.rs`
- Modify: `crates/rag-client/src/lib.rs`

- [ ] **Step 1: Create the types module**

Create `crates/rag-client/src/types.rs`:

```rust
//! Request and response types matching the Apex RAG server API.
//!
//! These types are defined independently of `rag-server` so that
//! `rag-client` has no workspace dependencies.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Ingest ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct IngestRequest {
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestResponse {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    #[serde(default)]
    pub failures: Vec<IngestFailure>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestFailure {
    pub path: String,
    pub error: String,
}

// ── Search ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HybridSearchResponse {
    pub results: Vec<HybridSearchResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HybridSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub fused_score: f32,
}

// ── Chat ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: Uuid,
    pub citations: Vec<Citation>,
    pub usage: Usage,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Citation {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ── Collections ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CollectionStatsResponse {
    pub collection: String,
    pub tenant: String,
    pub total_docs: i64,
    pub total_tokens: i64,
    pub avgdl: f64,
}

// ── Health / Readiness ──────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub checks: ReadinessChecks,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReadinessChecks {
    pub postgres: String,
    pub qdrant: String,
}
```

Note: response types derive both `Deserialize` and `Serialize` so the CLI can re-serialize them for `--json` output.

- [ ] **Step 2: Update lib.rs**

Add to `crates/rag-client/src/lib.rs`:

```rust
pub mod types;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rag-client`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/rag-client/src/types.rs crates/rag-client/src/lib.rs
git commit -m "feat(rag-client): add request/response types matching server API"
```

---

### Task 5: TenantApiClient — construction and generic helpers

**Files:**
- Create: `crates/rag-client/src/client.rs`
- Modify: `crates/rag-client/src/lib.rs`

- [ ] **Step 1: Write the failing construction tests**

Create `crates/rag-client/tests/client_tests.rs`:

```rust
#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

use rag_client::{ClientError, TenantApiClient};

#[test]
fn invalid_base_url_rejected() {
    let err = TenantApiClient::new("not a url", "default").unwrap_err();
    assert!(matches!(err, ClientError::InvalidBaseUrl(_)));
}

#[test]
fn invalid_tenant_rejected() {
    let err = TenantApiClient::new("http://localhost:8080", "").unwrap_err();
    assert!(matches!(err, ClientError::InvalidTenant(_)));

    let err = TenantApiClient::new("http://localhost:8080", "a/b").unwrap_err();
    assert!(matches!(err, ClientError::InvalidTenant(_)));
}

#[test]
fn valid_construction() {
    let client = TenantApiClient::new("http://localhost:8080", "my-tenant");
    assert!(client.is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-client --test client_tests`
Expected: FAIL — `TenantApiClient` not found

- [ ] **Step 3: Implement TenantApiClient**

Create `crates/rag-client/src/client.rs`:

```rust
//! HTTP client with automatic tenant header injection.

use std::time::Duration;

use reqwest::header::{HeaderValue, CONTENT_TYPE};
use reqwest::{Method, RequestBuilder, Response, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ClientError;
use crate::tenant_id::TenantId;

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Tenant header name.
const TENANT_HEADER: &str = "x-tenant";

/// HTTP client that injects an `x-tenant` header on every request.
pub struct TenantApiClient {
    client: reqwest::Client,
    base_url: Url,
    tenant: TenantId,
}

impl TenantApiClient {
    /// Create a new client with the default 60-second timeout.
    pub fn new(base_url: &str, tenant: &str) -> Result<Self, ClientError> {
        Self::with_timeout(base_url, tenant, DEFAULT_TIMEOUT)
    }

    /// Create a new client with a custom timeout.
    pub fn with_timeout(
        base_url: &str,
        tenant: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let url = Url::parse(base_url)
            .map_err(|e| ClientError::InvalidBaseUrl(format!("{base_url}: {e}")))?;

        let tenant_id = TenantId::new(tenant)
            .map_err(|e| ClientError::InvalidTenant(e))?;

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(ClientError::Transport)?;

        Ok(Self {
            client,
            base_url: url,
            tenant: tenant_id,
        })
    }

    /// Build a request with the tenant header and resolved URL.
    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, ClientError> {
        let url = self.base_url.join(path)
            .map_err(|e| ClientError::InvalidBaseUrl(format!("cannot join path '{path}': {e}")))?;

        Ok(self.client.request(method, url).header(TENANT_HEADER, self.tenant.as_str()))
    }

    /// Check the response status and deserialize the body as JSON.
    async fn into_success<T: DeserializeOwned>(response: Response) -> Result<T, ClientError> {
        let status = response.status();
        let url = response.url().to_string();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::HttpStatus {
                status: status.as_u16(),
                url,
                body,
            });
        }

        response.json::<T>().await.map_err(ClientError::Decode)
    }

    /// Send a GET request and deserialize the JSON response.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let response = self.request(Method::GET, path)?.send().await?;
        Self::into_success(response).await
    }

    /// Send a GET request and return the raw response.
    pub async fn get_response(&self, path: &str) -> Result<Response, ClientError> {
        let response = self.request(Method::GET, path)?.send().await?;
        let status = response.status();
        if !status.is_success() {
            let url = response.url().to_string();
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::HttpStatus {
                status: status.as_u16(),
                url,
                body,
            });
        }
        Ok(response)
    }

    /// Send a POST request with a JSON body and deserialize the response.
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let response = self
            .request(Method::POST, path)?
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .json(body)
            .send()
            .await?;
        Self::into_success(response).await
    }

    /// Send a POST request with a per-request timeout override.
    pub async fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, ClientError> {
        let response = self
            .request(Method::POST, path)?
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
            .json(body)
            .timeout(timeout)
            .send()
            .await?;
        Self::into_success(response).await
    }
}
```

- [ ] **Step 4: Update lib.rs**

Add to `crates/rag-client/src/lib.rs`:

```rust
mod client;

pub use client::TenantApiClient;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rag-client --test client_tests`
Expected: All 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-client/src/client.rs crates/rag-client/src/lib.rs crates/rag-client/tests/client_tests.rs
git commit -m "feat(rag-client): add TenantApiClient with construction and generic helpers"
```

---

### Task 6: Typed convenience methods

**Files:**
- Modify: `crates/rag-client/src/client.rs`

- [ ] **Step 1: Write the failing integration tests**

Append to `crates/rag-client/tests/client_tests.rs`:

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use rag_client::types::*;
use test_support::spawn_app;

#[derive(Clone, Default)]
struct Captured {
    requests: Arc<Mutex<Vec<(String, HeaderMap, String)>>>,
}

async fn capture_and_respond(
    State(cap): State<Captured>,
    headers: HeaderMap,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    body: String,
) -> impl IntoResponse {
    cap.requests.lock().unwrap().push((uri.to_string(), headers, body));

    let path = uri.path();
    if path == "/ingest" {
        return axum::Json(serde_json::json!({
            "documents": 3, "chunks": 10, "skipped": 1
        })).into_response();
    }
    if path == "/chat" {
        return axum::Json(serde_json::json!({
            "answer": "test answer",
            "conversation_id": "550e8400-e29b-41d4-a716-446655440000",
            "citations": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
            "model": "mock"
        })).into_response();
    }
    if path == "/search/hybrid" {
        return axum::Json(serde_json::json!({
            "results": [{
                "chunk_id": "c1", "document_id": "d1",
                "chunk_index": 0, "text": "hello", "fused_score": 0.9
            }]
        })).into_response();
    }
    if path == "/search/dense" || path == "/search/sparse" {
        return axum::Json(serde_json::json!({
            "results": [{
                "chunk_id": "c1", "document_id": "d1",
                "chunk_index": 0, "text": "hello", "score": 0.8
            }]
        })).into_response();
    }
    if path.starts_with("/collections/") && path.ends_with("/stats") {
        return axum::Json(serde_json::json!({
            "collection": "test", "tenant": "default",
            "total_docs": 42, "total_tokens": 1234, "avgdl": 29.38
        })).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn health_ok() -> impl IntoResponse {
    "ok"
}

async fn readiness_ok() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "ready": true,
        "checks": {"postgres": "ok", "qdrant": "ok"}
    }))
}

fn test_router() -> (Router, Captured) {
    let cap = Captured::default();
    let router = Router::new()
        .route("/health", get(health_ok))
        .route("/readiness", get(readiness_ok))
        .route("/ingest", post(capture_and_respond))
        .route("/chat", post(capture_and_respond))
        .route("/search/dense", post(capture_and_respond))
        .route("/search/sparse", post(capture_and_respond))
        .route("/search/hybrid", post(capture_and_respond))
        .route("/collections/:collection/stats", get(capture_and_respond))
        .with_state(cap.clone());
    (router, cap)
}

#[tokio::test]
async fn tenant_header_injected() {
    let (router, cap) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "my-tenant").unwrap();

    let _: IngestResponse = client.post_json("/ingest", &IngestRequest {
        paths: vec!["/tmp/test.txt".into()],
        collection: None,
    }).await.unwrap();

    let reqs = cap.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let tenant = reqs[0].1.get("x-tenant").unwrap().to_str().unwrap();
    assert_eq!(tenant, "my-tenant");
}

#[tokio::test]
async fn health_ok_test() {
    let (router, _) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    client.health().await.unwrap();
}

#[tokio::test]
async fn readiness_json_test() {
    let (router, _) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let resp = client.readiness().await.unwrap();
    assert!(resp.ready);
    assert_eq!(resp.checks.postgres, "ok");
}

#[tokio::test]
async fn ingest_request_shape() {
    let (router, cap) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let resp = client.ingest(&IngestRequest {
        paths: vec!["/data/file.pdf".into()],
        collection: Some("docs".into()),
    }).await.unwrap();

    assert_eq!(resp.documents, 3);
    assert_eq!(resp.chunks, 10);

    let reqs = cap.requests.lock().unwrap();
    let body: serde_json::Value = serde_json::from_str(&reqs[0].2).unwrap();
    assert_eq!(body["paths"][0], "/data/file.pdf");
    assert_eq!(body["collection"], "docs");
}

#[tokio::test]
async fn chat_roundtrip() {
    let (router, _) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let resp = client.chat(&ChatRequest {
        query: "hello".into(),
        collection: Some("test".into()),
        conversation_id: None,
        language: None,
        history_limit: None,
    }).await.unwrap();

    assert_eq!(resp.answer, "test answer");
    assert_eq!(resp.model, "mock");
    assert_eq!(resp.conversation_id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
}

#[tokio::test]
async fn search_hybrid_roundtrip() {
    let (router, _) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let resp = client.search_hybrid(&HybridSearchRequest {
        query: "hello".into(),
        collection: "test".into(),
        dense_top_k: None,
        sparse_top_k: None,
        rrf_k: None,
    }).await.unwrap();

    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].fused_score, 0.9);
}

#[tokio::test]
async fn collection_stats_path_encoding() {
    let (router, cap) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let _resp = client.collection_stats("my/collection").await.unwrap();

    let reqs = cap.requests.lock().unwrap();
    assert!(reqs[0].0.contains("my%2Fcollection"), "path should be percent-encoded: {}", reqs[0].0);
}

#[tokio::test]
async fn collection_stats_empty_rejected() {
    let client = TenantApiClient::new("http://localhost:9999", "default").unwrap();
    let err = client.collection_stats("").await.unwrap_err();
    assert!(matches!(err, ClientError::Validation(_)));
}

#[tokio::test]
async fn http_error_preserved() {
    let router = Router::new()
        .route("/fail", post(|| async { StatusCode::BAD_REQUEST.into_response() }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client.post_json::<serde_json::Value, serde_json::Value>("/fail", &serde_json::json!({}))
        .await.unwrap_err();
    match err {
        ClientError::HttpStatus { status, .. } => assert_eq!(status, 400),
        other => panic!("expected HttpStatus, got {other:?}"),
    }
}

#[tokio::test]
async fn decode_error() {
    let router = Router::new()
        .route("/bad-json", post(|| async { "not json" }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client.post_json::<serde_json::Value, serde_json::Value>("/bad-json", &serde_json::json!({}))
        .await.unwrap_err();
    assert!(matches!(err, ClientError::Decode(_)));
}

#[tokio::test]
async fn health_unexpected_body() {
    let router = Router::new()
        .route("/health", get(|| async { "not ok" }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client.health().await.unwrap_err();
    assert!(matches!(err, ClientError::Decode(_) | ClientError::HttpStatus { .. }));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-client --test client_tests`
Expected: FAIL — `health()`, `readiness()`, `ingest()`, `chat()`, `search_*()`, `collection_stats()` not found

- [ ] **Step 3: Add typed convenience methods to client.rs**

Append to the `impl TenantApiClient` block in `crates/rag-client/src/client.rs`:

```rust
    // ── Typed convenience methods ───────────────────────────────

    /// Check server liveness. Returns `Ok(())` if the server responds with "ok".
    pub async fn health(&self) -> Result<(), ClientError> {
        let response = self.get_response("/health").await?;
        let body = response.text().await.map_err(ClientError::Decode)?;
        if body.trim() == "ok" {
            Ok(())
        } else {
            Err(ClientError::Decode(
                reqwest::Client::new()
                    .get("http://invalid")
                    .send()
                    .await
                    .unwrap_err(), // placeholder — see note
            ))
        }
    }
```

Wait — that placeholder trick is bad. Let me use a proper approach. The `health()` method should return a `Validation` error for unexpected body:

```rust
    /// Check server liveness. Returns `Ok(())` if the server responds with "ok".
    pub async fn health(&self) -> Result<(), ClientError> {
        let response = self.get_response("/health").await?;
        let body = response.text().await.map_err(ClientError::Decode)?;
        if body.trim() == "ok" {
            Ok(())
        } else {
            Err(ClientError::Validation(format!(
                "unexpected health response: {body}"
            )))
        }
    }

    /// Check server readiness (Postgres + Qdrant).
    pub async fn readiness(&self) -> Result<crate::types::ReadinessResponse, ClientError> {
        self.get_json("/readiness").await
    }

    /// Ingest files/directories. Uses a 300-second per-request timeout.
    pub async fn ingest(
        &self,
        req: &crate::types::IngestRequest,
    ) -> Result<crate::types::IngestResponse, ClientError> {
        self.post_json_with_timeout("/ingest", req, Duration::from_secs(300))
            .await
    }

    /// Dense vector search.
    pub async fn search_dense(
        &self,
        req: &crate::types::SearchRequest,
    ) -> Result<crate::types::SearchResponse, ClientError> {
        self.post_json("/search/dense", req).await
    }

    /// Sparse (BM25) search.
    pub async fn search_sparse(
        &self,
        req: &crate::types::SearchRequest,
    ) -> Result<crate::types::SearchResponse, ClientError> {
        self.post_json("/search/sparse", req).await
    }

    /// Hybrid search (dense + sparse with RRF fusion).
    pub async fn search_hybrid(
        &self,
        req: &crate::types::HybridSearchRequest,
    ) -> Result<crate::types::HybridSearchResponse, ClientError> {
        self.post_json("/search/hybrid", req).await
    }

    /// RAG chat.
    pub async fn chat(
        &self,
        req: &crate::types::ChatRequest,
    ) -> Result<crate::types::ChatResponse, ClientError> {
        self.post_json("/chat", req).await
    }

    /// Fetch collection statistics. Percent-encodes the collection name.
    pub async fn collection_stats(
        &self,
        collection: &str,
    ) -> Result<crate::types::CollectionStatsResponse, ClientError> {
        if collection.is_empty() {
            return Err(ClientError::Validation(
                "collection must not be empty".into(),
            ));
        }
        let encoded =
            percent_encoding::utf8_percent_encode(collection, percent_encoding::NON_ALPHANUMERIC);
        let path = format!("/collections/{encoded}/stats");
        self.get_json(&path).await
    }
```

Add the import at the top of `client.rs`:

```rust
use percent_encoding;
```

- [ ] **Step 4: Update the health_unexpected_body test assertion**

Since `health()` now returns `ClientError::Validation` for unexpected body, update the test:

```rust
#[tokio::test]
async fn health_unexpected_body() {
    let router = Router::new()
        .route("/health", get(|| async { "not ok" }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client.health().await.unwrap_err();
    assert!(matches!(err, ClientError::Validation(_)));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rag-client --test client_tests`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rag-client/src/client.rs crates/rag-client/tests/client_tests.rs
git commit -m "feat(rag-client): add typed convenience methods for all 9 endpoints"
```

---

### Task 7: Ingest timeout override test

**Files:**
- Modify: `crates/rag-client/tests/client_tests.rs`

- [ ] **Step 1: Write the timeout override test**

Append to `crates/rag-client/tests/client_tests.rs`:

```rust
#[tokio::test]
async fn ingest_timeout_override() {
    // Server delays 2 seconds on every POST
    let router = Router::new()
        .route("/ingest", post(|| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            axum::Json(serde_json::json!({
                "documents": 1, "chunks": 1, "skipped": 0
            }))
        }))
        .route("/slow", post(|| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            axum::Json(serde_json::json!({"ok": true}))
        }));
    let server = spawn_app(router).await.expect("spawn");

    // Client with 1-second default timeout
    let client = TenantApiClient::with_timeout(
        &server.base_url(), "default", Duration::from_secs(1)
    ).unwrap();

    // ingest() should succeed because it uses 300s per-request timeout
    let resp = client.ingest(&IngestRequest {
        paths: vec!["/tmp/test.txt".into()],
        collection: None,
    }).await;
    assert!(resp.is_ok(), "ingest should succeed with 300s override: {resp:?}");

    // A plain post_json to the same delayed endpoint should time out at 1s
    let err = client.post_json::<serde_json::Value, serde_json::Value>(
        "/slow", &serde_json::json!({})
    ).await;
    assert!(err.is_err(), "plain post_json should timeout at 1s");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p rag-client --test client_tests ingest_timeout_override`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/rag-client/tests/client_tests.rs
git commit -m "test(rag-client): verify ingest 300s per-request timeout override"
```

---

### Task 8: ApiClient trait and impl

**Files:**
- Create: `crates/rag-client/src/trait_def.rs`
- Modify: `crates/rag-client/src/client.rs`
- Modify: `crates/rag-client/src/lib.rs`

- [ ] **Step 1: Create the trait definition**

Create `crates/rag-client/src/trait_def.rs`:

```rust
//! `ApiClient` trait for abstracting over the HTTP client.
//!
//! CLI command handlers are generic over `impl ApiClient`, enabling
//! unit testing with a `FakeClient`.

use crate::error::ClientError;
use crate::types::*;

#[trait_variant::make(Send)]
pub trait ApiClient {
    async fn health(&self) -> Result<(), ClientError>;
    async fn readiness(&self) -> Result<ReadinessResponse, ClientError>;
    async fn ingest(&self, req: &IngestRequest) -> Result<IngestResponse, ClientError>;
    async fn search_dense(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_sparse(&self, req: &SearchRequest) -> Result<SearchResponse, ClientError>;
    async fn search_hybrid(&self, req: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError>;
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError>;
    async fn collection_stats(&self, collection: &str) -> Result<CollectionStatsResponse, ClientError>;
}
```

- [ ] **Step 2: Implement ApiClient for TenantApiClient**

Add to the bottom of `crates/rag-client/src/client.rs`:

```rust
use crate::trait_def::ApiClient;

impl ApiClient for TenantApiClient {
    async fn health(&self) -> Result<(), ClientError> {
        self.health().await
    }

    async fn readiness(&self) -> Result<crate::types::ReadinessResponse, ClientError> {
        self.readiness().await
    }

    async fn ingest(&self, req: &crate::types::IngestRequest) -> Result<crate::types::IngestResponse, ClientError> {
        self.ingest(req).await
    }

    async fn search_dense(&self, req: &crate::types::SearchRequest) -> Result<crate::types::SearchResponse, ClientError> {
        self.search_dense(req).await
    }

    async fn search_sparse(&self, req: &crate::types::SearchRequest) -> Result<crate::types::SearchResponse, ClientError> {
        self.search_sparse(req).await
    }

    async fn search_hybrid(&self, req: &crate::types::HybridSearchRequest) -> Result<crate::types::HybridSearchResponse, ClientError> {
        self.search_hybrid(req).await
    }

    async fn chat(&self, req: &crate::types::ChatRequest) -> Result<crate::types::ChatResponse, ClientError> {
        self.chat(req).await
    }

    async fn collection_stats(&self, collection: &str) -> Result<crate::types::CollectionStatsResponse, ClientError> {
        self.collection_stats(collection).await
    }
}
```

- [ ] **Step 3: Update lib.rs**

Add to `crates/rag-client/src/lib.rs`:

```rust
mod trait_def;

pub use trait_def::ApiClient;
```

- [ ] **Step 4: Verify all tests still pass**

Run: `cargo test -p rag-client`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-client/src/trait_def.rs crates/rag-client/src/client.rs crates/rag-client/src/lib.rs
git commit -m "feat(rag-client): add ApiClient trait with TenantApiClient impl"
```

---

### Task 9: CLI Clap parsing and output module

**Files:**
- Create: `crates/rag-cli/src/cli.rs`
- Create: `crates/rag-cli/src/output.rs`
- Modify: `crates/rag-cli/src/main.rs`

- [ ] **Step 1: Create cli.rs with Clap structs**

Create `crates/rag-cli/src/cli.rs`:

```rust
//! CLI argument parsing.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use uuid::Uuid;

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

#[derive(Subcommand)]
pub enum Command {
    /// RAG chat (single-shot or interactive)
    Chat {
        #[arg(long)]
        query: String,
        /// Required for first message; omit when resuming via --conversation-id
        #[arg(long)]
        collection: Option<String>,
        /// Enter interactive multi-turn mode
        #[arg(long)]
        interactive: bool,
        /// Resume an existing conversation
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
        /// Search mode
        #[arg(long, value_enum, default_value_t = SearchMode::Hybrid)]
        mode: SearchMode,
        #[arg(long)]
        top_k: Option<u64>,
    },
    /// Show collection statistics
    #[command(name = "collection-stats")]
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

/// Validate CLI constraints that Clap cannot express declaratively.
pub fn validate(cli: &Cli) -> anyhow::Result<()> {
    if let Command::Chat { interactive, .. } = &cli.command {
        if *interactive && cli.json {
            anyhow::bail!("--interactive and --json cannot be used together");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn ingest_rejects_no_paths() {
        let result = Cli::try_parse_from(["rag-cli", "ingest"]);
        assert!(result.is_err());
    }

    #[test]
    fn interactive_json_conflict() {
        let cli = Cli::try_parse_from([
            "rag-cli", "--json", "chat", "--query", "hi", "--collection", "c", "--interactive",
        ]);
        // Parsing succeeds, but validation fails
        let cli = cli.unwrap();
        assert!(validate(&cli).is_err());
    }

    #[test]
    fn server_env_fallback() {
        // Set env var before parsing
        unsafe { std::env::set_var("RAG_SERVER_URL", "http://my-server:9090") };
        let cli = Cli::try_parse_from([
            "rag-cli", "collection-stats", "--collection", "test",
        ]).unwrap();
        assert_eq!(cli.server, "http://my-server:9090");
        unsafe { std::env::remove_var("RAG_SERVER_URL") };
    }
}
```

- [ ] **Step 2: Create output.rs**

Create `crates/rag-cli/src/output.rs`:

```rust
//! Shared output formatting (human-friendly vs JSON).

use std::io::Write;

use serde::Serialize;

/// Print a value as JSON or human-friendly text.
///
/// If `json_mode` is true, serialize `value` as JSON to `writer`.
/// Otherwise, call `human_fn` to produce human-readable output.
pub fn print_or_json<W: Write, T: Serialize>(
    writer: &mut W,
    json_mode: bool,
    value: &T,
    human_fn: impl FnOnce(&T, &mut W) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if json_mode {
        serde_json::to_writer_pretty(writer, value)?;
        writeln!(writer)?;
        Ok(())
    } else {
        human_fn(value, writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_or_json_human() {
        let mut buf = Vec::new();
        let value = serde_json::json!({"key": "val"});
        let called = std::cell::Cell::new(false);

        print_or_json(&mut buf, false, &value, |_v, w| {
            called.set(true);
            writeln!(w, "human output")?;
            Ok(())
        }).unwrap();

        assert!(called.get());
        assert_eq!(String::from_utf8(buf).unwrap(), "human output\n");
    }

    #[test]
    fn print_or_json_json() {
        let mut buf = Vec::new();
        let value = serde_json::json!({"key": "val"});

        print_or_json(&mut buf, true, &value, |_v, _w| {
            panic!("human_fn should not be called in JSON mode");
        }).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"key\""));
        assert!(output.contains("\"val\""));
    }

    #[test]
    fn print_or_json_write_error() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"))
            }
        }

        let value = serde_json::json!({"key": "val"});
        let result = print_or_json(&mut BrokenWriter, true, &value, |_v, _w| Ok(()));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Update main.rs to a placeholder dispatcher**

Replace `crates/rag-cli/src/main.rs` with:

```rust
mod cli;
mod output;
mod commands;

use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();
    if let Err(e) = cli::validate(&cli) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    if let Err(e) = run(cli).await {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    let client = rag_client::TenantApiClient::new(&cli.server, &cli.tenant)?;
    let mut stdout = std::io::stdout().lock();

    match cli.command {
        cli::Command::Ingest { paths, collection } => {
            commands::ingest::run(&client, &mut stdout, cli.json, paths, collection).await
        }
        cli::Command::Search { query, collection, mode, top_k } => {
            commands::search::run(&client, &mut stdout, cli.json, query, collection, mode, top_k).await
        }
        cli::Command::Chat { query, collection, interactive, conversation_id } => {
            commands::chat::run(&client, &mut stdout, cli.json, query, collection, interactive, conversation_id).await
        }
        cli::Command::CollectionStats { collection } => {
            commands::collections::run(&client, &mut stdout, cli.json, collection).await
        }
    }
}
```

- [ ] **Step 4: Create commands/mod.rs placeholder**

Create `crates/rag-cli/src/commands/mod.rs`:

```rust
pub mod chat;
pub mod collections;
pub mod ingest;
pub mod search;
```

Create stub files for each command (they'll be filled in subsequent tasks):

`crates/rag-cli/src/commands/ingest.rs`:
```rust
use std::io::Write;
use std::path::PathBuf;
use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _paths: Vec<PathBuf>,
    _collection: Option<String>,
) -> anyhow::Result<()> {
    anyhow::bail!("ingest not yet implemented")
}
```

`crates/rag-cli/src/commands/search.rs`:
```rust
use std::io::Write;
use crate::cli::SearchMode;
use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _query: String,
    _collection: String,
    _mode: SearchMode,
    _top_k: Option<u64>,
) -> anyhow::Result<()> {
    anyhow::bail!("search not yet implemented")
}
```

`crates/rag-cli/src/commands/chat.rs`:
```rust
use std::io::Write;
use uuid::Uuid;
use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _query: String,
    _collection: Option<String>,
    _interactive: bool,
    _conversation_id: Option<Uuid>,
) -> anyhow::Result<()> {
    anyhow::bail!("chat not yet implemented")
}
```

`crates/rag-cli/src/commands/collections.rs`:
```rust
use std::io::Write;
use rag_client::ApiClient;

pub async fn run(
    _client: &impl ApiClient,
    _writer: &mut impl Write,
    _json: bool,
    _collection: String,
) -> anyhow::Result<()> {
    anyhow::bail!("collection-stats not yet implemented")
}
```

- [ ] **Step 5: Verify everything compiles**

Run: `cargo check -p rag-cli`
Expected: PASS

- [ ] **Step 6: Run parser and output tests**

Run: `cargo test -p rag-cli`
Expected: All tests in cli.rs and output.rs pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rag-cli/src/
git commit -m "feat(rag-cli): add Clap parsing, output module, and command stubs"
```

---

### Task 10: Ingest command handler

**Files:**
- Modify: `crates/rag-cli/src/commands/ingest.rs`

- [ ] **Step 1: Implement the ingest handler**

Replace `crates/rag-cli/src/commands/ingest.rs` with:

```rust
use std::io::Write;
use std::path::PathBuf;

use rag_client::ApiClient;
use rag_client::types::{IngestRequest, IngestResponse};

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    paths: Vec<PathBuf>,
    collection: Option<String>,
) -> anyhow::Result<()> {
    // Validate paths before sending
    for p in &paths {
        if !p.is_absolute() {
            anyhow::bail!("all paths must be absolute, got: {}", p.display());
        }
        if !p.exists() {
            anyhow::bail!("path does not exist: {}", p.display());
        }
    }

    let req = IngestRequest {
        paths: paths.iter().map(|p| p.display().to_string()).collect(),
        collection,
    };

    let resp = client.ingest(&req).await?;

    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(w, "Ingested {} documents, {} chunks, {} skipped",
            resp.documents, resp.chunks, resp.skipped)?;
        for f in &resp.failures {
            writeln!(w, "WARN: {} — {}", f.path, f.error)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::path::PathBuf;

    struct FakeIngestClient {
        response: IngestResponse,
    }

    impl rag_client::ApiClient for FakeIngestClient {
        async fn health(&self) -> Result<(), ClientError> { Ok(()) }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> { unimplemented!() }
        async fn ingest(&self, _req: &IngestRequest) -> Result<IngestResponse, ClientError> {
            Ok(self.response.clone())
        }
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_hybrid(&self, _: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError> { unimplemented!() }
        async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ClientError> { unimplemented!() }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> { unimplemented!() }
    }

    /// Create a temporary file with an absolute path for testing.
    fn temp_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().unwrap()
    }

    #[tokio::test]
    async fn ingest_human_output() {
        let tmp = temp_file();
        let client = FakeIngestClient {
            response: IngestResponse { documents: 3, chunks: 10, skipped: 1, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, false, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Ingested 3 documents, 10 chunks, 1 skipped"));
    }

    #[tokio::test]
    async fn ingest_partial_failure() {
        let tmp = temp_file();
        let client = FakeIngestClient {
            response: IngestResponse {
                documents: 2, chunks: 5, skipped: 0,
                failures: vec![IngestFailure { path: "/tmp/bad.txt".into(), error: "parse error".into() }],
            },
        };
        let mut buf = Vec::new();
        // run() should succeed (partial failure is not a command error)
        run(&client, &mut buf, false, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("WARN: /tmp/bad.txt — parse error"));
    }

    #[tokio::test]
    async fn ingest_json_output() {
        let tmp = temp_file();
        let client = FakeIngestClient {
            response: IngestResponse { documents: 1, chunks: 2, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        run(&client, &mut buf, true, vec![tmp.path().to_path_buf()], None).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["documents"], 1);
    }

    #[tokio::test]
    async fn ingest_rejects_relative_path() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 0, chunks: 0, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        let result = run(&client, &mut buf, false, vec![PathBuf::from("relative/path")], None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute"));
    }

    #[tokio::test]
    async fn ingest_rejects_nonexistent_path() {
        let client = FakeIngestClient {
            response: IngestResponse { documents: 0, chunks: 0, skipped: 0, failures: vec![] },
        };
        let mut buf = Vec::new();
        let result = run(&client, &mut buf, false, vec![PathBuf::from("/nonexistent/path/xyz")], None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-cli ingest`
Expected: All ingest tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-cli/src/commands/ingest.rs
git commit -m "feat(rag-cli): implement ingest command handler"
```

---

### Task 11: Search command handler

**Files:**
- Modify: `crates/rag-cli/src/commands/search.rs`

- [ ] **Step 1: Implement the search handler**

Replace `crates/rag-cli/src/commands/search.rs` with:

```rust
use std::io::Write;

use rag_client::ApiClient;
use rag_client::types::{
    HybridSearchRequest, HybridSearchResponse, SearchRequest, SearchResponse,
};

use crate::cli::SearchMode;
use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    query: String,
    collection: String,
    mode: SearchMode,
    top_k: Option<u64>,
) -> anyhow::Result<()> {
    match mode {
        SearchMode::Dense => {
            let req = SearchRequest { query, collection, top_k };
            let resp = client.search_dense(&req).await?;
            print_or_json(writer, json, &resp, format_search)
        }
        SearchMode::Sparse => {
            let req = SearchRequest { query, collection, top_k };
            let resp = client.search_sparse(&req).await?;
            print_or_json(writer, json, &resp, format_search)
        }
        SearchMode::Hybrid => {
            let req = HybridSearchRequest {
                query,
                collection,
                dense_top_k: top_k,
                sparse_top_k: top_k,
                rrf_k: None,
            };
            let resp = client.search_hybrid(&req).await?;
            print_or_json(writer, json, &resp, format_hybrid)
        }
    }
}

fn format_search<W: Write>(resp: &SearchResponse, w: &mut W) -> anyhow::Result<()> {
    if resp.results.is_empty() {
        writeln!(w, "No results found.")?;
        return Ok(());
    }
    for (i, r) in resp.results.iter().enumerate() {
        let preview: String = r.text.chars().take(80).collect();
        writeln!(w, "[{}] (score: {:.2}) {} — {}, chunk {}",
            i + 1, r.score, r.chunk_id, r.document_id, r.chunk_index)?;
        writeln!(w, "    {preview}")?;
    }
    Ok(())
}

fn format_hybrid<W: Write>(resp: &HybridSearchResponse, w: &mut W) -> anyhow::Result<()> {
    if resp.results.is_empty() {
        writeln!(w, "No results found.")?;
        return Ok(());
    }
    for (i, r) in resp.results.iter().enumerate() {
        let preview: String = r.text.chars().take(80).collect();
        writeln!(w, "[{}] (score: {:.2}) {} — {}, chunk {}",
            i + 1, r.fused_score, r.chunk_id, r.document_id, r.chunk_index)?;
        writeln!(w, "    {preview}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::sync::Mutex;

    struct FakeSearchClient {
        called: Mutex<Vec<String>>,
    }

    impl FakeSearchClient {
        fn new() -> Self {
            Self { called: Mutex::new(Vec::new()) }
        }
    }

    impl rag_client::ApiClient for FakeSearchClient {
        async fn health(&self) -> Result<(), ClientError> { Ok(()) }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> { unimplemented!() }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> { unimplemented!() }
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            self.called.lock().unwrap().push("dense".into());
            Ok(SearchResponse { results: vec![SearchResult {
                chunk_id: "c1".into(), document_id: "d1".into(),
                chunk_index: 0, text: "hello world".into(), score: 0.8,
            }]})
        }
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> {
            self.called.lock().unwrap().push("sparse".into());
            Ok(SearchResponse { results: vec![] })
        }
        async fn search_hybrid(&self, _: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError> {
            self.called.lock().unwrap().push("hybrid".into());
            Ok(HybridSearchResponse { results: vec![HybridSearchResult {
                chunk_id: "c1".into(), document_id: "d1".into(),
                chunk_index: 0, text: "hello world".into(), fused_score: 0.9,
            }]})
        }
        async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ClientError> { unimplemented!() }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> { unimplemented!() }
    }

    #[tokio::test]
    async fn search_hybrid_human_output() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "test".into(), "coll".into(), SearchMode::Hybrid, None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("[1] (score: 0.90)"));
        assert!(output.contains("hello world"));
    }

    #[tokio::test]
    async fn search_dense_dispatches_correctly() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "test".into(), "coll".into(), SearchMode::Dense, None).await.unwrap();
        let called = client.called.lock().unwrap();
        assert_eq!(called.as_slice(), &["dense"]);
    }

    #[tokio::test]
    async fn search_json_output() {
        let client = FakeSearchClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, true, "test".into(), "coll".into(), SearchMode::Hybrid, None).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(parsed["results"].is_array());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-cli search`
Expected: All search tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-cli/src/commands/search.rs
git commit -m "feat(rag-cli): implement search command handler"
```

---

### Task 12: Chat command handler

**Files:**
- Modify: `crates/rag-cli/src/commands/chat.rs`

- [ ] **Step 1: Implement the chat handler**

Replace `crates/rag-cli/src/commands/chat.rs` with:

```rust
use std::io::{BufRead, Write};

use uuid::Uuid;

use rag_client::ApiClient;
use rag_client::types::{ChatRequest, ChatResponse};

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    query: String,
    collection: Option<String>,
    interactive: bool,
    conversation_id: Option<Uuid>,
) -> anyhow::Result<()> {
    // Validate: collection required for new conversations
    if collection.is_none() && conversation_id.is_none() {
        anyhow::bail!("--collection is required for the first message in a conversation");
    }

    let req = ChatRequest {
        query,
        collection: collection.clone(),
        conversation_id,
        language: None,
        history_limit: None,
    };

    let resp = client.chat(&req).await?;

    if !interactive {
        return print_or_json(writer, json, &resp, format_chat);
    }

    // Interactive mode: print first response, then loop
    format_chat(&resp, writer)?;
    let conv_id = resp.conversation_id;

    let stdin = std::io::stdin();
    let reader = stdin.lock();
    interactive_loop(client, writer, reader, conv_id).await
}

async fn interactive_loop(
    client: &impl ApiClient,
    writer: &mut impl Write,
    mut reader: impl BufRead,
    conversation_id: Uuid,
) -> anyhow::Result<()> {
    loop {
        write!(writer, "\n> ")?;
        writer.flush()?;

        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 || line.trim().is_empty() {
            break;
        }

        let req = ChatRequest {
            query: line.trim().to_string(),
            collection: None,
            conversation_id: Some(conversation_id),
            language: None,
            history_limit: None,
        };

        let resp = client.chat(&req).await?;
        format_chat(&resp, writer)?;
    }
    Ok(())
}

fn format_chat<W: Write>(resp: &ChatResponse, w: &mut W) -> anyhow::Result<()> {
    writeln!(w, "{}", resp.answer)?;

    if !resp.citations.is_empty() {
        writeln!(w)?;
        for (i, c) in resp.citations.iter().enumerate() {
            writeln!(w, "  [{}] {} ({}, chunk {})",
                i + 1, c.chunk_id, c.document_id, c.chunk_index)?;
        }
    }

    writeln!(w)?;
    writeln!(w, "[conversation: {}] [model: {}]", resp.conversation_id, resp.model)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;
    use std::sync::Mutex;

    struct FakeChatClient {
        calls: Mutex<Vec<ChatRequest>>,
    }

    impl FakeChatClient {
        fn new() -> Self {
            Self { calls: Mutex::new(Vec::new()) }
        }
    }

    impl rag_client::ApiClient for FakeChatClient {
        async fn health(&self) -> Result<(), ClientError> { Ok(()) }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> { unimplemented!() }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> { unimplemented!() }
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_hybrid(&self, _: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError> { unimplemented!() }
        async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, ClientError> {
            self.calls.lock().unwrap().push(ChatRequest {
                query: req.query.clone(),
                collection: req.collection.clone(),
                conversation_id: req.conversation_id,
                language: None,
                history_limit: None,
            });
            Ok(ChatResponse {
                answer: "test answer".into(),
                conversation_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
                citations: vec![Citation {
                    chunk_id: "c1".into(), document_id: "d1".into(),
                    chunk_index: 0, sources: vec![],
                }],
                usage: Usage { prompt_tokens: 10, completion_tokens: 5 },
                model: "mock".into(),
            })
        }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> { unimplemented!() }
    }

    #[tokio::test]
    async fn chat_single_shot() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "hello".into(), Some("coll".into()), false, None).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("test answer"));
        assert!(output.contains("[1] c1 (d1, chunk 0)"));
        assert!(output.contains("[conversation: 550e8400"));
        assert!(output.contains("[model: mock]"));
    }

    #[tokio::test]
    async fn chat_requires_collection_without_conversation_id() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        let result = run(&client, &mut buf, false, "hello".into(), None, false, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--collection is required"));
    }

    #[tokio::test]
    async fn chat_allows_no_collection_with_conversation_id() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        let conv_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let result = run(&client, &mut buf, false, "hello".into(), None, false, Some(conv_id)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn chat_conversation_id_threaded() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();

        // First call
        run(&client, &mut buf, false, "first".into(), Some("coll".into()), false, None).await.unwrap();

        // Simulate second call with conversation_id from first response
        let conv_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        run(&client, &mut buf, false, "second".into(), None, false, Some(conv_id)).await.unwrap();

        let calls = client.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].collection.is_some());
        assert!(calls[1].collection.is_none());
        assert_eq!(calls[1].conversation_id, Some(conv_id));
    }

    #[tokio::test]
    async fn chat_json_output() {
        let client = FakeChatClient::new();
        let mut buf = Vec::new();
        run(&client, &mut buf, true, "hello".into(), Some("coll".into()), false, None).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["answer"], "test answer");
        assert!(parsed["conversation_id"].is_string());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-cli chat`
Expected: All chat tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-cli/src/commands/chat.rs
git commit -m "feat(rag-cli): implement chat command handler with interactive mode"
```

---

### Task 13: Collection-stats command handler

**Files:**
- Modify: `crates/rag-cli/src/commands/collections.rs`

- [ ] **Step 1: Implement the collection-stats handler**

Replace `crates/rag-cli/src/commands/collections.rs` with:

```rust
use std::io::Write;

use rag_client::ApiClient;
use rag_client::types::CollectionStatsResponse;

use crate::output::print_or_json;

pub async fn run(
    client: &impl ApiClient,
    writer: &mut impl Write,
    json: bool,
    collection: String,
) -> anyhow::Result<()> {
    let resp = client.collection_stats(&collection).await?;

    print_or_json(writer, json, &resp, |resp, w| {
        writeln!(w, "Collection: {}", resp.collection)?;
        writeln!(w, "Tenant:     {}", resp.tenant)?;
        writeln!(w, "Documents:  {}", resp.total_docs)?;
        writeln!(w, "Tokens:     {}", resp.total_tokens)?;
        writeln!(w, "Avg Doc Length: {:.2}", resp.avgdl)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rag_client::ClientError;
    use rag_client::types::*;

    struct FakeCollectionsClient;

    impl rag_client::ApiClient for FakeCollectionsClient {
        async fn health(&self) -> Result<(), ClientError> { Ok(()) }
        async fn readiness(&self) -> Result<ReadinessResponse, ClientError> { unimplemented!() }
        async fn ingest(&self, _: &IngestRequest) -> Result<IngestResponse, ClientError> { unimplemented!() }
        async fn search_dense(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_sparse(&self, _: &SearchRequest) -> Result<SearchResponse, ClientError> { unimplemented!() }
        async fn search_hybrid(&self, _: &HybridSearchRequest) -> Result<HybridSearchResponse, ClientError> { unimplemented!() }
        async fn chat(&self, _: &ChatRequest) -> Result<ChatResponse, ClientError> { unimplemented!() }
        async fn collection_stats(&self, _: &str) -> Result<CollectionStatsResponse, ClientError> {
            Ok(CollectionStatsResponse {
                collection: "my-collection".into(),
                tenant: "default".into(),
                total_docs: 42,
                total_tokens: 12345,
                avgdl: 293.93,
            })
        }
    }

    #[tokio::test]
    async fn collection_stats_human_output() {
        let client = FakeCollectionsClient;
        let mut buf = Vec::new();
        run(&client, &mut buf, false, "my-collection".into()).await.unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Collection: my-collection"));
        assert!(output.contains("Documents:  42"));
        assert!(output.contains("Tokens:     12345"));
        assert!(output.contains("Avg Doc Length: 293.93"));
    }

    #[tokio::test]
    async fn collection_stats_json_output() {
        let client = FakeCollectionsClient;
        let mut buf = Vec::new();
        run(&client, &mut buf, true, "my-collection".into()).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed["total_docs"], 42);
        assert_eq!(parsed["avgdl"], 293.93);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-cli collections`
Expected: All collection-stats tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-cli/src/commands/collections.rs
git commit -m "feat(rag-cli): implement collection-stats command handler"
```

---

### Task 14: Error rendering tests

**Files:**
- Create: `crates/rag-cli/tests/error_rendering.rs`

- [ ] **Step 1: Write error rendering tests**

Create `crates/rag-cli/tests/error_rendering.rs`:

```rust
#![allow(clippy::disallowed_methods)]

use rag_client::ClientError;

#[test]
fn http_status_error_display() {
    let err = ClientError::HttpStatus {
        status: 400,
        url: "http://localhost/chat".into(),
        body: "query must not be empty".into(),
    };
    let msg = format!("{err}");
    assert!(msg.contains("400"), "should contain status code: {msg}");
    assert!(msg.contains("query must not be empty"), "should contain body: {msg}");
}

#[test]
fn transport_error_display() {
    // Create a transport error by trying to connect to a closed port
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt.block_on(async {
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .unwrap_err()
    });
    let client_err = ClientError::Transport(err);
    let msg = format!("{client_err}");
    assert!(msg.contains("transport error"), "should contain prefix: {msg}");
}

#[test]
fn validation_error_display() {
    let err = ClientError::Validation("collection must not be empty".into());
    let msg = format!("{err}");
    assert_eq!(msg, "collection must not be empty");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p rag-cli --test error_rendering`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rag-cli/tests/error_rendering.rs
git commit -m "test(rag-cli): add error rendering display tests"
```

---

### Task 15: Final cargo check and clippy

**Files:**
- No file changes (verification only)

- [ ] **Step 1: Run cargo check across both crates**

Run: `cargo check -p rag-client -p rag-cli`
Expected: PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p rag-client -p rag-cli -- -D warnings`
Expected: PASS (no warnings). Fix any issues found.

- [ ] **Step 3: Run all tests**

Run: `cargo test -p rag-client && cargo test -p rag-cli`
Expected: All tests pass.

- [ ] **Step 4: Run format check**

Run: `cargo fmt -p rag-client -p rag-cli -- --check`
Expected: PASS. Fix any formatting issues.

- [ ] **Step 5: Commit any fixes**

If clippy or fmt required changes:
```bash
git add -A
git commit -m "chore: fix clippy and fmt issues in rag-client and rag-cli"
```
