#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use rag_client::types::*;
use rag_client::{ClientError, TenantApiClient};
use test_support::spawn_app;

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

// ── Integration tests using a live Axum test server ─────────────────────────

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
        }))
        .into_response();
    }
    if path == "/chat" {
        return axum::Json(serde_json::json!({
            "answer": "test answer",
            "conversation_id": "550e8400-e29b-41d4-a716-446655440000",
            "citations": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
            "model": "mock"
        }))
        .into_response();
    }
    if path == "/search/hybrid" {
        return axum::Json(serde_json::json!({
            "results": [{
                "chunk_id": "c1", "document_id": "d1",
                "chunk_index": 0, "text": "hello", "fused_score": 0.9
            }]
        }))
        .into_response();
    }
    if path == "/search/dense" || path == "/search/sparse" {
        return axum::Json(serde_json::json!({
            "results": [{
                "chunk_id": "c1", "document_id": "d1",
                "chunk_index": 0, "text": "hello", "score": 0.8
            }]
        }))
        .into_response();
    }
    if path.starts_with("/collections/") && path.ends_with("/stats") {
        return axum::Json(serde_json::json!({
            "collection": "test", "tenant": "default",
            "total_docs": 42, "total_tokens": 1234, "avgdl": 29.38
        }))
        .into_response();
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

    let _: IngestResponse = client
        .post_json(
            "/ingest",
            &IngestRequest { paths: vec!["/tmp/test.txt".into()], collection: None },
        )
        .await
        .unwrap();

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

    let resp = client
        .ingest(&IngestRequest {
            paths: vec!["/data/file.pdf".into()],
            collection: Some("docs".into()),
        })
        .await
        .unwrap();

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

    let resp = client
        .chat(&ChatRequest {
            query: "hello".into(),
            collection: Some("test".into()),
            conversation_id: None,
            language: None,
            history_limit: None,
        })
        .await
        .unwrap();

    assert_eq!(resp.answer, "test answer");
    assert_eq!(resp.model, "mock");
    assert_eq!(resp.conversation_id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
}

#[tokio::test]
async fn search_hybrid_roundtrip() {
    let (router, _) = test_router();
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let resp = client
        .search_hybrid(&HybridSearchRequest {
            query: "hello".into(),
            collection: "test".into(),
            dense_top_k: None,
            sparse_top_k: None,
            rrf_k: None,
        })
        .await
        .unwrap();

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
    let router =
        Router::new().route("/fail", post(|| async { StatusCode::BAD_REQUEST.into_response() }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client
        .post_json::<serde_json::Value, serde_json::Value>("/fail", &serde_json::json!({}))
        .await
        .unwrap_err();
    match err {
        ClientError::HttpStatus { status, .. } => assert_eq!(status, 400),
        other => panic!("expected HttpStatus, got {other:?}"),
    }
}

#[tokio::test]
async fn decode_error() {
    let router = Router::new().route("/bad-json", post(|| async { "not json" }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client
        .post_json::<serde_json::Value, serde_json::Value>("/bad-json", &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Decode(_)));
}

#[tokio::test]
async fn health_unexpected_body() {
    let router = Router::new().route("/health", get(|| async { "not ok" }));
    let server = spawn_app(router).await.expect("spawn");
    let client = TenantApiClient::new(&server.base_url(), "default").unwrap();

    let err = client.health().await.unwrap_err();
    assert!(matches!(err, ClientError::Validation(_)));
}

#[tokio::test]
async fn ingest_timeout_override() {
    // Server delays 2 seconds on every POST
    let router = Router::new()
        .route(
            "/ingest",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                axum::Json(serde_json::json!({
                    "documents": 1, "chunks": 1, "skipped": 0
                }))
            }),
        )
        .route(
            "/slow",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                axum::Json(serde_json::json!({"ok": true}))
            }),
        );
    let server = spawn_app(router).await.expect("spawn");

    // Client with 1-second default timeout
    let client =
        TenantApiClient::with_timeout(&server.base_url(), "default", Duration::from_secs(1))
            .unwrap();

    // ingest() should succeed because it uses 300s per-request timeout
    let resp = client
        .ingest(&IngestRequest { paths: vec!["/tmp/test.txt".into()], collection: None })
        .await;
    assert!(resp.is_ok(), "ingest should succeed with 300s override: {resp:?}");

    // A plain post_json to the same delayed endpoint should time out at 1s
    let err = client
        .post_json::<serde_json::Value, serde_json::Value>("/slow", &serde_json::json!({}))
        .await;
    assert!(err.is_err(), "plain post_json should timeout at 1s");
}
