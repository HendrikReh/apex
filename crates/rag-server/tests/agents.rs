#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

mod common;

use reqwest::StatusCode;
use test_support::spawn_app;
use uuid::Uuid;

fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[tokio::test]
#[ignore] // requires `just up`
async fn execute_agent_then_approve_run() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agent-coll-{suffix}");
    let tenant = format!("test-agent-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let agents = client
        .get(format!("{}/agents", server.base_url()))
        .header("x-tenant", &tenant)
        .send()
        .await
        .expect("list agents");
    assert_eq!(agents.status(), StatusCode::OK);
    let agents_body: serde_json::Value = agents.json().await.expect("agents json");
    assert!(
        agents_body
            .as_array()
            .expect("agents array")
            .iter()
            .any(|agent| agent["agent_id"] == "rag_spike"),
        "expected rag_spike agent to be present: {agents_body}"
    );

    let execute = client
        .post(format!("{}/agents/rag_spike/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let execute_body: serde_json::Value = execute.json().await.expect("execute json");
    assert_eq!(execute_body["state"], "awaiting_approval");
    assert_eq!(execute_body["pending_checkpoint"]["checkpoint_id"], "post_summary");
    let run_id = execute_body["run_id"].as_str().expect("run_id");

    let listed_runs = client
        .get(format!("{}/runs", server.base_url()))
        .header("x-tenant", &tenant)
        .send()
        .await
        .expect("list runs");
    assert_eq!(listed_runs.status(), StatusCode::OK);
    let listed_runs_body: serde_json::Value = listed_runs.json().await.expect("runs json");
    assert!(
        listed_runs_body
            .as_array()
            .expect("runs array")
            .iter()
            .any(|run| run["run_id"] == run_id && run["agent_id"] == "rag_spike"),
        "expected run to be listed: {listed_runs_body}"
    );

    let approve = client
        .post(format!("{}/runs/{run_id}/approve", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({ "reason": "looks good" }))
        .send()
        .await
        .expect("approve");
    assert_eq!(approve.status(), StatusCode::OK);
    let approve_body: serde_json::Value = approve.json().await.expect("approve json");
    assert_eq!(approve_body["state"], "completed");
    assert_eq!(approve_body["answer"], "Mock LLM response.");
    assert!(approve_body["pending_checkpoint"].is_null());
}

#[tokio::test]
#[ignore] // requires `just up`
async fn execute_agent_then_reject_run() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agent-reject-coll-{suffix}");
    let tenant = format!("test-agent-reject-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/rag_spike/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let execute_body: serde_json::Value = execute.json().await.expect("execute json");
    let run_id = execute_body["run_id"].as_str().expect("run_id");

    let reject = client
        .post(format!("{}/runs/{run_id}/reject", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({ "reason": "not relevant" }))
        .send()
        .await
        .expect("reject");
    assert_eq!(reject.status(), StatusCode::OK);
    let reject_body: serde_json::Value = reject.json().await.expect("reject json");
    assert_eq!(reject_body["state"], "completed");
    assert!(
        reject_body["answer"].as_str().unwrap_or("").contains("not relevant"),
        "expected rejection reason in answer: {reject_body}"
    );
}

#[tokio::test]
#[ignore] // requires `just up`
async fn runs_are_tenant_scoped_for_read_and_decision_paths() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agent-tenant-coll-{suffix}");
    let tenant_a = format!("test-agent-a-{suffix}");
    let tenant_b = format!("test-agent-b-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant_a)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/rag_spike/execute", server.base_url()))
        .header("x-tenant", &tenant_a)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let execute_body: serde_json::Value = execute.json().await.expect("execute json");
    let run_id = execute_body["run_id"].as_str().expect("run_id");

    let list_tenant_b = client
        .get(format!("{}/runs", server.base_url()))
        .header("x-tenant", &tenant_b)
        .send()
        .await
        .expect("list tenant b");
    assert_eq!(list_tenant_b.status(), StatusCode::OK);
    let list_tenant_b_body: serde_json::Value = list_tenant_b.json().await.expect("runs json");
    assert!(
        list_tenant_b_body.as_array().expect("array").is_empty(),
        "tenant B must not see tenant A runs: {list_tenant_b_body}"
    );

    let get_tenant_b = client
        .get(format!("{}/runs/{run_id}", server.base_url()))
        .header("x-tenant", &tenant_b)
        .send()
        .await
        .expect("get tenant b");
    assert_eq!(get_tenant_b.status(), StatusCode::NOT_FOUND);

    let approve_tenant_b = client
        .post(format!("{}/runs/{run_id}/approve", server.base_url()))
        .header("x-tenant", &tenant_b)
        .json(&serde_json::json!({ "reason": "cross-tenant attempt" }))
        .send()
        .await
        .expect("approve tenant b");
    assert_eq!(approve_tenant_b.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // requires `just up`
async fn execute_agentic_search_v1_simple_query_uses_baseline_path() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agentic-search-v1-coll-{suffix}");
    let tenant = format!("test-agentic-search-v1-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "What is in the document?",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let execute_body: serde_json::Value = execute.json().await.expect("execute json");
    assert_eq!(execute_body["state"], "completed");
    assert_eq!(execute_body["route_decision"]["selected_path"], "single_pass_rag");
    assert_eq!(execute_body["answer"], "Mock LLM response.");
}

#[tokio::test]
#[ignore] // requires `just up`
async fn execute_agentic_search_v1_comparison_query_uses_agentic_path() {
    let (app, _state) = common::full_app().await;
    let server = spawn_app(app).await.expect("spawn");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.txt");

    let client = reqwest::Client::new();
    let suffix = unique_suffix();
    let collection = format!("test-agentic-search-v1-comparison-{suffix}");
    let tenant = format!("test-agentic-search-v1-comparison-{suffix}");

    let ingest = client
        .post(format!("{}/ingest", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "paths": [fixture.to_str().unwrap()],
            "collection": &collection
        }))
        .send()
        .await
        .expect("ingest");
    assert_eq!(ingest.status(), StatusCode::OK);

    let execute = client
        .post(format!("{}/agents/agentic_search_v1/execute", server.base_url()))
        .header("x-tenant", &tenant)
        .json(&serde_json::json!({
            "query": "Compare Rust and Python tradeoffs for async services",
            "collection": &collection
        }))
        .send()
        .await
        .expect("execute");
    assert_eq!(execute.status(), StatusCode::OK);
    let execute_body: serde_json::Value = execute.json().await.expect("execute json");
    assert_eq!(execute_body["state"], "completed");
    assert_eq!(execute_body["route_decision"]["selected_path"], "agentic_search");
    assert_eq!(execute_body["route_decision"]["retrieval_profile"], "broad_then_expand");
    assert_eq!(execute_body["answer"], "Mock LLM response.");
    assert!(
        !execute_body["search_results"].as_array().expect("search_results").is_empty(),
        "expected agentic search to return evidence"
    );
    assert!(
        execute_body["search_results"][0]["sources"].is_array(),
        "expected enriched search result sources"
    );
}
