#![allow(clippy::disallowed_methods)]

mod common;

use std::collections::BTreeSet;
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

fn unique_chat_document_ids(body: &serde_json::Value) -> BTreeSet<String> {
    body["citations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["document_id"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn unique_agent_document_ids(body: &serde_json::Value) -> BTreeSet<String> {
    body["search_results"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["document_id"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn count_expected_hits(expected: &[String], seen: &BTreeSet<String>) -> usize {
    expected.iter().filter(|doc| seen.contains(doc.as_str())).count()
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
    assert!(
        benchmark.len() >= 25,
        "benchmark should include at least 25 queries, found {}",
        benchmark.len()
    );

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

    let mut simple_case_count = 0usize;
    let mut agentic_case_count = 0usize;
    let mut enriched_agentic_cases = 0usize;

    for case in &benchmark {
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
        let baseline_body: serde_json::Value = baseline.json().await.expect("baseline json");

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
        let routed_body: serde_json::Value = routed.json().await.expect("routed json");

        assert_eq!(
            routed_body["route_decision"]["selected_path"].as_str(),
            Some(case.expected_route.as_str()),
            "route mismatch for benchmark case {}",
            case.id
        );
        assert_eq!(
            routed_body["route_decision"]["query_class"].as_str(),
            Some(case.expected_query_class.as_str()),
            "query_class mismatch for benchmark case {}",
            case.id
        );

        let rerun = client
            .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
            .header("x-tenant", &tenant)
            .json(&serde_json::json!({
                "query": &case.query,
                "collection": &collection
            }))
            .send()
            .await
            .expect("routed rerun");
        assert_eq!(rerun.status(), StatusCode::OK);
        let rerun_body: serde_json::Value = rerun.json().await.expect("routed rerun json");

        assert_eq!(
            rerun_body["route_decision"]["selected_path"],
            routed_body["route_decision"]["selected_path"],
            "route changed between repeated runs for benchmark case {}",
            case.id
        );
        assert_eq!(
            rerun_body["route_decision"]["query_class"],
            routed_body["route_decision"]["query_class"],
            "query_class changed between repeated runs for benchmark case {}",
            case.id
        );
        assert_eq!(
            rerun_body["route_decision"]["retrieval_profile"],
            routed_body["route_decision"]["retrieval_profile"],
            "retrieval_profile changed between repeated runs for benchmark case {}",
            case.id
        );

        let baseline_docs = unique_chat_document_ids(&baseline_body);
        let routed_docs = unique_agent_document_ids(&routed_body);
        let routed_hits = count_expected_hits(&case.expected_evidence, &routed_docs);
        let baseline_hits = count_expected_hits(&case.expected_evidence, &baseline_docs);
        let routed_score_types: BTreeSet<String> = routed_body["search_results"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["score_type"].as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let has_enriched_metadata = routed_body["search_results"]
            .as_array()
            .map(|items| {
                items.iter().any(|item| {
                    item["text"].as_str().is_some_and(|text| !text.is_empty())
                        && item["score_type"]
                            .as_str()
                            .is_some_and(|score_type| !score_type.is_empty())
                })
            })
            .unwrap_or(false);

        assert_eq!(
            routed_hits,
            case.expected_evidence.len(),
            "routed evidence missed expected documents for benchmark case {}",
            case.id
        );

        if case.expected_route == "single_pass_rag" {
            simple_case_count += 1;
            // `full_app()` wires a mock LLM, so exact answer parity is the
            // strongest signal that the routed single-pass branch is reusing
            // the same baseline generation path as `/chat`.
            assert_eq!(
                baseline_body["answer"], routed_body["answer"],
                "simple-query answer drift for benchmark case {}",
                case.id
            );
            assert_eq!(
                baseline_docs, routed_docs,
                "simple-query evidence drift for benchmark case {}",
                case.id
            );
            assert_eq!(
                baseline_hits, routed_hits,
                "simple-query evidence recall regressed for benchmark case {}",
                case.id
            );
        } else {
            agentic_case_count += 1;
            assert!(
                routed_hits >= baseline_hits,
                "agentic routed path recovered less evidence than baseline /chat for benchmark case {}",
                case.id
            );
            assert!(
                !routed_score_types.is_empty(),
                "agentic routed path should preserve score provenance for benchmark case {}",
                case.id
            );
            if has_enriched_metadata {
                enriched_agentic_cases += 1;
            }
        }
    }

    assert!(simple_case_count > 0, "benchmark must include simple baseline cases");
    assert!(agentic_case_count > 0, "benchmark must include agentic cases");
    assert!(
        enriched_agentic_cases == agentic_case_count,
        "agentic routed path should expose enriched evidence metadata for every agentic benchmark case"
    );
}
