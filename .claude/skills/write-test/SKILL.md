---
name: write-test
description: Use when writing or modifying tests — covers spawn_app pattern, state capture with Arc<Mutex>, and Axum route syntax for this Rust workspace. Also use when asked to "add a test", "write tests for this", or "test this endpoint/function".
---

# Write Test

Guide for writing tests in this project. Read the relevant existing tests first, then follow these patterns.

## Test Naming

Follow the pattern `test_<what>_<expected_behavior>`:
```
test_ingest_returns_400_for_invalid_path
test_search_filters_by_tenant
test_chunking_splits_on_sentence_boundary
```

Use `#[tokio::test]` for all async tests (most tests in this project). Use `#[test]` only for pure synchronous logic.

## HTTP-Level Tests (CLI -> Server)

For testing CLI commands that call the server API:

1. Build a mock `axum::Router` with the endpoints the command calls
2. Use `test_support::spawn_app(router)` to bind an ephemeral port
3. Call the real CLI function against the mock server URL
4. Assert on the CLI output and/or captured request state

```rust
use axum::{routing::get, Router, Json};
use test_support::spawn_app;

#[tokio::test]
async fn test_list_runs_returns_formatted_output() {
    let router = Router::new()
        .route("/runs", get(|| async { Json(serde_json::json!({"runs": []})) }));

    let server = spawn_app(router).await;
    let url = server.url();

    let result = my_command(&url).await;
    assert!(result.is_ok());
}
```

## Oneshot Pattern (Server Handler Tests)

For testing individual handlers without spawning a server — faster and simpler:

```rust
use axum::{body::Body, http::{Request, StatusCode, header}, Router};
use tower::ServiceExt; // for .oneshot()

#[tokio::test]
async fn test_stats_endpoint_returns_200() {
    let app = Router::new()
        .route("/collections/:collection/stats", get(stats))
        .with_state(state)
        .layer(Extension(test_ctx()));

    let res = app
        .oneshot(
            Request::builder()
                .uri("/collections/test-collection/stats")
                .method("GET")
                .header("x-tenant", "test-tenant")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}
```

## State Capture (Request Bodies / Query Strings)

Use `Arc<Mutex<Option<T>>>` to capture what the CLI sends:

```rust
use std::sync::{Arc, Mutex};
use axum::{extract::Json, routing::post, Router};

#[tokio::test]
async fn test_ingest_sends_correct_payload() {
    let captured = Arc::new(Mutex::new(None::<serde_json::Value>));
    let captured_clone = captured.clone();

    let router = Router::new()
        .route("/ingest", post(move |Json(body): Json<serde_json::Value>| {
            let captured = captured_clone.clone();
            async move {
                *captured.lock().unwrap() = Some(body);
                Json(serde_json::json!({"ok": true}))
            }
        }));

    let server = spawn_app(router).await;
    my_command(&server.url()).await.unwrap();

    let body = captured.lock().unwrap().clone().unwrap();
    assert_eq!(body["collections"][0], "docs");
}
```

## Axum Route Syntax

Use `:param` syntax (Axum 0.7): `/runs/:run_id`

Do NOT use `{param}` (that's Axum 0.8+ / OpenAPI syntax — different from Axum routing).

## Integration Tests

Integration tests use `#[ignore]` and env var gates. See `/integration-test` skill for full details. Key pattern:

```rust
#[tokio::test]
#[ignore = "requires RUN_QDRANT_INTEGRATION_TESTS=1 and Postgres+Qdrant"]
#[serial]
async fn test_stores_create_collection() {
    if !require_integration_env() {
        return;
    }
    // ... test with real services
}
```

## Reference Tests

Look at these files for real examples:
- `crates/rag-cli/src/commands/` — CLI command tests with state capture
- `crates/rag-server/src/` — server handler tests with oneshot
- `crates/rag-core/tests/` — integration test patterns

## Cross-references

- `/add-endpoint` — includes test template for new endpoints
- `/integration-test` — running integration tests with correct env vars
- `/debug` — diagnosing test failures
