#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

mod common;

use test_support::spawn_app;
use uuid::Uuid;

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_search_chat_journey() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-e2e-coll-{suffix}");
    let tenant = format!("test-e2e-{suffix}");

    // --- Step 1: Ingest ---
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");

    assert_eq!(resp.status(), 200);
    // Verify x-request-id is present on response.
    assert!(resp.headers().get("x-request-id").is_some());
    // Verify x-tenant is echoed.
    assert_eq!(resp.headers().get("x-tenant").and_then(|v| v.to_str().ok()), Some(tenant.as_str()));

    // --- Step 2: Search ---
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": &collection
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
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
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

#[tokio::test]
#[ignore] // requires `just up`
async fn tenant_isolation_through_api() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-tenant-iso-coll-{suffix}");
    let tenant_a = format!("test-tenant-a-{suffix}");
    let tenant_b = format!("test-tenant-b-{suffix}");

    // Tenant A ingests a document.
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant_a)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(resp.status(), 200);

    // Tenant A can search its own data.
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", &tenant_a)
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": &collection
        }))
        .send()
        .await
        .expect("search tenant A");
    assert_eq!(resp.status(), 200);
    let tenant_a_body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        !tenant_a_body["results"].as_array().expect("array").is_empty(),
        "tenant A should see its own ingested document"
    );

    // Tenant B should not see Tenant A's data via search.
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", &tenant_b)
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": &collection
        }))
        .send()
        .await
        .expect("search tenant B");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-tenant").and_then(|v| v.to_str().ok()),
        Some(tenant_b.as_str())
    );
    let tenant_b_search_body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        tenant_b_search_body["results"].as_array().expect("array").is_empty(),
        "tenant B must not see tenant A's search results"
    );

    // Tenant B chat should also have no citations from Tenant A's corpus.
    let resp = client
        .post(format!("{}/chat", server.base_url()))
        .header("x-tenant", &tenant_b)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("chat tenant B");
    assert_eq!(resp.status(), 200);
    let tenant_b_chat_body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        tenant_b_chat_body["citations"].as_array().expect("array").is_empty(),
        "tenant B chat must not receive citations from tenant A's documents"
    );
}
