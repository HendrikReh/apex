#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

mod common;

use axum::Router;
use axum::routing::get;
use test_support::spawn_app;

/// GET /health returns 200 with body "ok". No database needed.
#[tokio::test]
async fn health_returns_ok() {
    let app = Router::new().route("/health", get(rag_server::routes::health::health));
    let server = spawn_app(app).await.expect("spawn");

    let resp = reqwest::get(format!("{}/health", server.base_url())).await.expect("request");

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.expect("body"), "ok");
}

/// GET /readiness returns 200 with checks when services are up.
#[tokio::test]
#[ignore] // requires `just up`
async fn readiness_returns_checks() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let resp = reqwest::get(format!("{}/readiness", server.base_url())).await.expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["ready"], true);
    assert_eq!(body["checks"]["postgres"], "ok");
    assert_eq!(body["checks"]["qdrant"], "ok");
}

/// GET /readiness returns 503 when a dependency is unreachable.
///
/// **Destructive**: stops/restarts the Qdrant container.
/// Must not run in parallel with other readiness tests.
/// Run with: `cargo test -p rag-server --test health -- --ignored --test-threads=1`
#[tokio::test]
#[ignore] // requires `just up`; stops and restarts Qdrant container
async fn readiness_degrades_gracefully() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    // Stop Qdrant container.
    let _ = std::process::Command::new("docker").args(["compose", "stop", "qdrant"]).status();

    let resp = reqwest::get(format!("{}/readiness", server.base_url())).await.expect("request");

    // Restart Qdrant so other tests aren't affected.
    let _ = std::process::Command::new("docker").args(["compose", "start", "qdrant"]).status();

    // Wait for Qdrant to become responsive again before returning,
    // so parallel tests in other binaries aren't affected.
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if reqwest::get(format!("{}/readiness", server.base_url()))
            .await
            .map(|r| r.status() == 200)
            .unwrap_or(false)
        {
            break;
        }
    }

    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["ready"], false);
    assert_eq!(body["checks"]["postgres"], "ok");
    assert_ne!(body["checks"]["qdrant"], "ok");
}
