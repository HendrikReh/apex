#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

//! Destructive readiness test — stops and restarts the Qdrant container.
//! Isolated in its own binary so it cannot race with other readiness tests.

mod common;

use std::path::PathBuf;

use test_support::spawn_app;

const DEFAULT_COMPOSE_PROJECT_NAME: &str = "apex";

fn compose_project_name() -> String {
    std::env::var("APEX_COMPOSE_PROJECT_NAME")
        .unwrap_or_else(|_| DEFAULT_COMPOSE_PROJECT_NAME.to_string())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

fn docker_compose_service(action: &str, service: &str) {
    let compose_file = workspace_root().join("docker-compose.yml");
    let _ = std::process::Command::new("docker")
        .current_dir(workspace_root())
        .args([
            "compose",
            "-p",
            &compose_project_name(),
            "-f",
            compose_file.to_str().expect("compose file path"),
            action,
            service,
        ])
        .status();
}

/// GET /readiness returns 503 when a dependency is unreachable.
#[tokio::test]
#[ignore] // requires `just up`; stops and restarts Qdrant container
async fn readiness_degrades_gracefully() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    // Stop Qdrant container from the shared local compose project.
    docker_compose_service("stop", "qdrant");

    let resp = reqwest::get(format!("{}/readiness", server.base_url())).await.expect("request");

    // Restart Qdrant so other tests aren't affected.
    docker_compose_service("start", "qdrant");

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
