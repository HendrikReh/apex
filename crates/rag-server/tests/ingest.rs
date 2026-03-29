mod common;

use test_support::spawn_app;

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_paths_returns_counts() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", "test-ingest")
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": "test-ingest-collection"
        }))
        .send()
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(body["documents"].as_u64().unwrap_or(0) >= 1);
    assert!(body["chunks"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test]
#[ignore] // requires `just up`
async fn ingest_upload_returns_counts() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");
    let file_bytes = std::fs::read(&fixture).expect("read fixture");

    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(file_bytes)
                .file_name("sample.txt")
                .mime_str("text/plain")
                .expect("mime"),
        )
        .text("collection", "test-upload-collection");

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/ingest/upload", server.base_url()))
        .header("x-tenant", "test-upload")
        .multipart(form)
        .send()
        .await
        .expect("request");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["documents"].as_u64().unwrap_or(0), 1);
    assert!(body["chunks"].as_u64().unwrap_or(0) >= 1);
}
