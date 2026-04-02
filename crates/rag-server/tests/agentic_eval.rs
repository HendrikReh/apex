#![allow(clippy::disallowed_methods)]

mod common;

use std::fs;
use std::path::Path;

use reqwest::StatusCode;
use serde::Deserialize;
use test_support::spawn_app;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct BenchmarkCase {
    id: String,
    query: String,
    expected_route: String,
    expected_query_class: String,
    expected_evidence: Vec<String>,
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[tokio::test]
#[ignore] // requires `just up`
async fn benchmark_routes_and_evidence_are_scored() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");
    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("eval-agentic-{suffix}");
    let tenant = format!("eval-agentic-{suffix}");
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("data/evals/agentic_search_v1/corpus");
    let benchmark_path = corpus_dir.parent().expect("benchmark dir").join("benchmark.json");

    let benchmark: Vec<BenchmarkCase> =
        serde_json::from_str(&fs::read_to_string(benchmark_path).expect("benchmark json"))
            .expect("parse benchmark");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [corpus_dir.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let mut route_hits = 0usize;
    let mut evidence_hits = 0usize;

    for case in &benchmark {
        let _ = (&case.id, &case.expected_query_class);

        let baseline = client
            .post(format!("{}/chat", server.base_url()))
            .header("x-tenant", &tenant)
            .json(&serde_json::json!({
                "query": &case.query,
                "collection": &collection
            }))
            .send()
            .await
            .expect("baseline");
        assert_eq!(baseline.status(), StatusCode::OK);

        let routed = client
            .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
            .header("x-tenant", &tenant)
            .json(&serde_json::json!({
                "query": &case.query,
                "collection": &collection
            }))
            .send()
            .await
            .expect("routed");
        assert_eq!(routed.status(), StatusCode::OK);
        let body: serde_json::Value = routed.json().await.expect("routed json");

        if body["route_decision"]["selected_path"].as_str() == Some(case.expected_route.as_str()) {
            route_hits += 1;
        }

        let docs: Vec<String> = body["search_results"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["document_id"].as_str().map(|doc| doc.to_owned()))
                    .collect()
            })
            .unwrap_or_default();

        if case.expected_evidence.iter().all(|doc| docs.iter().any(|seen| seen == doc)) {
            evidence_hits += 1;
        }
    }

    assert!(route_hits >= benchmark.len().saturating_sub(1), "route accuracy too low");
    assert!(evidence_hits >= 2, "evidence recall too low");
}
