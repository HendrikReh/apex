#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

//! Destructive readiness test — stops and restarts the Qdrant container.
//! Isolated in its own binary so it cannot race with other readiness tests.

mod common;

use test_support::spawn_app;

/// GET /readiness returns 503 when a dependency is unreachable.
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
    // so tests in other binaries aren't affected.
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
