//! Integration tests for authentication, authorization, and rate limiting.
//!
//! Requires `just up` (Docker services) and migrations applied.

mod common;

use reqwest::StatusCode;

// ---------------------------------------------------------------------------
// auth_mode=none (default) — existing behavior preserved
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // requires `just up`
#[allow(clippy::disallowed_methods)]
async fn none_mode_allows_unauthenticated_request() {
    let (router, _state) = common::full_app().await;
    let server = test_support::spawn_app(router).await.expect("spawn");

    let resp = reqwest::Client::new()
        .post(format!("{}/search/dense", server.base_url()))
        .header("x-tenant", "test-auth-none")
        .json(&serde_json::json!({
            "query": "test",
            "collection": "nonexistent",
            "top_k": 1,
        }))
        .send()
        .await
        .expect("send");

    // Should not be 401/403 — auth_mode=none grants anonymous admin
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Public routes remain accessible
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore] // requires `just up`
#[allow(clippy::disallowed_methods)]
async fn health_endpoint_needs_no_auth() {
    let (router, _state) = common::full_app().await;
    let server = test_support::spawn_app(router).await.expect("spawn");

    let resp = reqwest::Client::new()
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore] // requires `just up`
#[allow(clippy::disallowed_methods)]
async fn readiness_endpoint_needs_no_auth() {
    let (router, _state) = common::full_app().await;
    let server = test_support::spawn_app(router).await.expect("spawn");

    let resp = reqwest::Client::new()
        .get(format!("{}/readiness", server.base_url()))
        .send()
        .await
        .expect("send");

    // OK or SERVICE_UNAVAILABLE depending on backend health — but never 401/403
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}
