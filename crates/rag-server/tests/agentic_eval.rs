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
    #[serde(default)]
    expected_evidence: Vec<String>,
    #[serde(default)]
    required_evidence: Vec<String>,
    #[serde(default)]
    supporting_evidence: Vec<String>,
    #[serde(default)]
    min_expected_recall: Option<f32>,
}

#[derive(Debug)]
struct EvidenceExpectation {
    required_documents: BTreeSet<String>,
    expected_documents: BTreeSet<String>,
    min_expected_recall: f32,
}

impl BenchmarkCase {
    fn evidence_expectation(&self) -> EvidenceExpectation {
        let has_explicit_buckets =
            !self.required_evidence.is_empty() || !self.supporting_evidence.is_empty();
        assert!(
            !has_explicit_buckets || self.expected_evidence.is_empty(),
            "benchmark case {} mixes legacy expected_evidence with explicit required/supporting buckets",
            self.id
        );

        let required_documents: BTreeSet<String> = if has_explicit_buckets {
            self.required_evidence.iter().cloned().collect()
        } else if self.min_expected_recall.is_some() {
            BTreeSet::new()
        } else {
            self.expected_evidence.iter().cloned().collect()
        };

        let expected_documents: BTreeSet<String> = if has_explicit_buckets {
            self.expected_evidence
                .iter()
                .chain(self.required_evidence.iter())
                .chain(self.supporting_evidence.iter())
                .cloned()
                .collect()
        } else {
            self.expected_evidence.iter().cloned().collect()
        };

        assert!(
            !expected_documents.is_empty(),
            "benchmark case {} must define at least one expected document",
            self.id
        );

        // Legacy schema (`expected_evidence` only) keeps strict all-doc recall
        // unless the benchmark explicitly opts into a per-case threshold.
        let default_min_expected_recall = if has_explicit_buckets {
            required_documents.len() as f32 / expected_documents.len() as f32
        } else {
            1.0
        };

        let min_expected_recall = self.min_expected_recall.unwrap_or(default_min_expected_recall);
        assert!(
            (0.0..=1.0).contains(&min_expected_recall),
            "benchmark case {} has min_expected_recall={} outside [0.0, 1.0]",
            self.id,
            min_expected_recall
        );

        EvidenceExpectation { required_documents, expected_documents, min_expected_recall }
    }
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

fn count_expected_hits(expected: &BTreeSet<String>, seen: &BTreeSet<String>) -> usize {
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
        let expectation = case.evidence_expectation();
        let routed_hits = count_expected_hits(&expectation.expected_documents, &routed_docs);
        let baseline_hits = count_expected_hits(&expectation.expected_documents, &baseline_docs);
        let required_hits = count_expected_hits(&expectation.required_documents, &routed_docs);
        let routed_recall = routed_hits as f32 / expectation.expected_documents.len() as f32;
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
            required_hits,
            expectation.required_documents.len(),
            "routed evidence missed required documents for benchmark case {}",
            case.id
        );
        assert!(
            routed_recall >= expectation.min_expected_recall,
            "routed evidence recall {:.3} below threshold {:.3} for benchmark case {}",
            routed_recall,
            expectation.min_expected_recall,
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

#[test]
fn legacy_expected_evidence_defaults_to_strict_recall() {
    let case = BenchmarkCase {
        id: "legacy".to_string(),
        query: "q".to_string(),
        expected_route: "agentic_search".to_string(),
        expected_query_class: "multi_hop_research".to_string(),
        expected_evidence: vec!["doc-a".to_string(), "doc-b".to_string()],
        required_evidence: Vec::new(),
        supporting_evidence: Vec::new(),
        min_expected_recall: None,
    };

    let expectation = case.evidence_expectation();
    assert_eq!(expectation.required_documents, expectation.expected_documents);
    assert!((expectation.min_expected_recall - 1.0).abs() < f32::EPSILON);
}

#[test]
fn explicit_required_and_supporting_default_to_required_coverage() {
    let case = BenchmarkCase {
        id: "explicit".to_string(),
        query: "q".to_string(),
        expected_route: "agentic_search".to_string(),
        expected_query_class: "multi_hop_research".to_string(),
        expected_evidence: Vec::new(),
        required_evidence: vec!["doc-a".to_string()],
        supporting_evidence: vec!["doc-b".to_string()],
        min_expected_recall: None,
    };

    let expectation = case.evidence_expectation();
    assert_eq!(expectation.required_documents.len(), 1);
    assert_eq!(expectation.expected_documents.len(), 2);
    assert!((expectation.min_expected_recall - 0.5).abs() < f32::EPSILON);
}

#[test]
fn min_expected_recall_can_relax_legacy_expected_pool() {
    let case = BenchmarkCase {
        id: "threshold".to_string(),
        query: "q".to_string(),
        expected_route: "agentic_search".to_string(),
        expected_query_class: "multi_hop_research".to_string(),
        expected_evidence: vec!["doc-a".to_string(), "doc-b".to_string()],
        required_evidence: Vec::new(),
        supporting_evidence: Vec::new(),
        min_expected_recall: Some(0.5),
    };

    let expectation = case.evidence_expectation();
    assert!(expectation.required_documents.is_empty());
    assert_eq!(expectation.expected_documents.len(), 2);
    assert!((expectation.min_expected_recall - 0.5).abs() < f32::EPSILON);
}

#[test]
fn supporting_only_defaults_to_zero_recall_threshold() {
    let case = BenchmarkCase {
        id: "supporting-only".to_string(),
        query: "q".to_string(),
        expected_route: "agentic_search".to_string(),
        expected_query_class: "multi_hop_research".to_string(),
        expected_evidence: Vec::new(),
        required_evidence: Vec::new(),
        supporting_evidence: vec!["doc-a".to_string(), "doc-b".to_string()],
        min_expected_recall: None,
    };

    let expectation = case.evidence_expectation();
    assert!(expectation.required_documents.is_empty());
    assert_eq!(expectation.expected_documents.len(), 2);
    assert!((expectation.min_expected_recall - 0.0).abs() < f32::EPSILON);
}

#[test]
#[should_panic(expected = "mixes legacy expected_evidence with explicit required/supporting")]
fn mixed_legacy_and_explicit_evidence_buckets_are_rejected() {
    let case = BenchmarkCase {
        id: "mixed".to_string(),
        query: "q".to_string(),
        expected_route: "agentic_search".to_string(),
        expected_query_class: "multi_hop_research".to_string(),
        expected_evidence: vec!["doc-a".to_string()],
        required_evidence: vec!["doc-b".to_string()],
        supporting_evidence: Vec::new(),
        min_expected_recall: None,
    };

    let _ = case.evidence_expectation();
}
