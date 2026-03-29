#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

mod common;

use test_support::spawn_app;
use uuid::Uuid;

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_then_hybrid_search() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let tenant = format!("test-search-{}", unique_suffix());
    let collection = format!("test-search-coll-{}", unique_suffix());

    let client = reqwest::Client::new();

    // Ingest first.
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest request");
    assert_eq!(resp.status(), 200);

    // Search.
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": &collection
        }))
        .send()
        .await
        .expect("search request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "search should return at least one result");
}
