#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_search_chat_journey() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

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
    assert_eq!(resp.headers().get("x-tenant").and_then(|v| v.to_str().ok()), Some(tenant));

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
