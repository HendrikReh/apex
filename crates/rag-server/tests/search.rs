mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_then_hybrid_search() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();

    // Ingest first.
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", "test-search")
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": "test-search-collection"
        }))
        .send()
        .await
        .expect("ingest request");
    assert_eq!(resp.status(), 200);

    // Search.
    let resp = client
        .post(format!("{}/search/hybrid", server.base_url()))
        .header("x-tenant", "test-search")
        .json(&serde_json::json!({
            "query": "sample document",
            "collection": "test-search-collection"
        }))
        .send()
        .await
        .expect("search request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "search should return at least one result");
}
