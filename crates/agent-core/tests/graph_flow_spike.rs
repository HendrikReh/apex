//! Integration test for the vertical slice: classify → search → summarize → checkpoint → answer.

use std::sync::Arc;

use agent_core::ports::{ApprovalPort, ChatPort, RetrievalPort};
use agent_core::runtime::AgentRuntime;
use agent_core::runtime::graph_flow::GraphFlowRuntime;
use agent_core::spec::AgentSpec;
use agent_core::types::{
    AgentRunConfig, AgentState, CheckpointDecision, PendingCheckpoint, ScoredChunk,
};

// ---------------------------------------------------------------------------
// Mock ports
// ---------------------------------------------------------------------------

struct MockRetrieval;

#[async_trait::async_trait]
impl RetrievalPort for MockRetrieval {
    async fn search_hybrid(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(vec![
            ScoredChunk {
                chunk_id: "chunk-1".into(),
                document_id: "doc-1".into(),
                chunk_index: 0,
                text: "Retrieval-Augmented Generation combines retrieval with generation.".into(),
                score: 0.95,
            },
            ScoredChunk {
                chunk_id: "chunk-2".into(),
                document_id: "doc-1".into(),
                chunk_index: 1,
                text: "RAG reduces hallucination by grounding answers in retrieved documents."
                    .into(),
                score: 0.87,
            },
        ])
    }
}

struct MockChat;

#[async_trait::async_trait]
impl ChatPort for MockChat {
    async fn summarize(
        &self,
        query: &str,
        chunks: &[ScoredChunk],
        _tenant: &str,
    ) -> anyhow::Result<String> {
        Ok(format!(
            "Summary for '{}': Based on {} retrieved chunks, RAG combines retrieval with generation.",
            query,
            chunks.len()
        ))
    }
}

/// Auto-approve port — no pause.
struct AutoApprove;

#[async_trait::async_trait]
impl ApprovalPort for AutoApprove {
    async fn request_approval(
        &self,
        _checkpoint: &PendingCheckpoint,
    ) -> anyhow::Result<Option<CheckpointDecision>> {
        Ok(Some(CheckpointDecision { approved: true, reason: None }))
    }
}

/// Manual approval port — always pauses.
struct ManualApproval;

#[async_trait::async_trait]
impl ApprovalPort for ManualApproval {
    async fn request_approval(
        &self,
        _checkpoint: &PendingCheckpoint,
    ) -> anyhow::Result<Option<CheckpointDecision>> {
        Ok(None) // No decision yet — pause the graph.
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn config() -> AgentRunConfig {
    AgentRunConfig {
        query: "What is retrieval-augmented generation?".into(),
        collection: "test-docs".into(),
        tenant: "default".into(),
        max_steps: 20,
    }
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn full_run_with_auto_approve() {
    let runtime =
        GraphFlowRuntime::new(Arc::new(MockRetrieval), Arc::new(MockChat), Arc::new(AutoApprove));

    let result = runtime.start(config()).await.expect("start failed");

    assert_eq!(result.state, AgentState::Completed);
    assert!(result.answer.is_some(), "expected a final answer");
    assert!(result.summary.is_some(), "expected a summary");
    assert!(result.search_results.is_some(), "expected search results");
    assert_eq!(result.search_results.as_ref().map(|r| r.len()), Some(2));
    assert!(result.query_type.is_some(), "expected query classification");
    assert!(result.pending_checkpoint.is_none(), "should not be awaiting approval");
    assert!(!result.steps.is_empty(), "expected step records");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn pause_and_resume_with_manual_approval() {
    let runtime = GraphFlowRuntime::new(
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    // Start — should pause at the checkpoint.
    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::AwaitingApproval);
    assert!(result.pending_checkpoint.is_some(), "expected pending checkpoint");
    assert!(result.summary.is_some(), "summary should be available before approval");

    let run_id = result.run_id;

    // Resume with approval.
    let result = runtime
        .resume(run_id, CheckpointDecision { approved: true, reason: Some("looks good".into()) })
        .await
        .expect("resume failed");

    assert_eq!(result.state, AgentState::Completed);
    assert!(result.answer.is_some(), "expected final answer after approval");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn rejection_at_checkpoint() {
    let runtime = GraphFlowRuntime::new(
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::AwaitingApproval);

    let run_id = result.run_id;

    // Resume with rejection.
    let result = runtime
        .resume(run_id, CheckpointDecision { approved: false, reason: Some("not relevant".into()) })
        .await
        .expect("resume failed");

    assert_eq!(result.state, AgentState::Completed);
    let answer = result.answer.as_deref().unwrap_or("");
    assert!(answer.contains("rejected"), "expected rejection message, got: {answer}");
    assert!(answer.contains("not relevant"), "expected reason in answer, got: {answer}");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn inspect_returns_session_state() {
    let runtime = GraphFlowRuntime::new(
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    let result = runtime.start(config()).await.expect("start failed");
    let run_id = result.run_id;

    let inspected = runtime.inspect(run_id).await.expect("inspect failed");
    assert!(inspected.is_some(), "should find the paused session");
    assert_eq!(inspected.as_ref().map(|r| r.state), Some(AgentState::AwaitingApproval));
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn inspect_unknown_run_returns_none() {
    let runtime =
        GraphFlowRuntime::new(Arc::new(MockRetrieval), Arc::new(MockChat), Arc::new(AutoApprove));

    let result = runtime.inspect(uuid::Uuid::new_v4()).await.expect("inspect failed");
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// Spec-driven tests
// ---------------------------------------------------------------------------

const RAG_SPIKE_SPEC: &str = r#"
agent_id: rag_spike
description: "Vertical-slice RAG agent."
spec_version: "1.0"
tasks:
  - classify
  - hybrid_search
  - summarize
  - approval_checkpoint
  - final_answer
graph:
  start_task: classify
  tasks:
    - classify
    - hybrid_search
    - summarize
    - approval_checkpoint
    - final_answer
  edges:
    - { from: classify, to: hybrid_search }
    - { from: hybrid_search, to: summarize }
    - { from: summarize, to: approval_checkpoint }
    - { from: approval_checkpoint, to: final_answer }
checkpoints:
  - checkpoint_id: post_summary
    after_task: summarize
    approval_type: human
"#;

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn spec_driven_full_run() {
    let spec = AgentSpec::from_yaml_str(RAG_SPIKE_SPEC).expect("spec should parse");
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(AutoApprove),
    );

    let result = runtime.start(config()).await.expect("start failed");

    assert_eq!(result.state, AgentState::Completed);
    assert!(result.answer.is_some(), "expected a final answer");
    assert!(result.summary.is_some(), "expected a summary");
    assert_eq!(result.search_results.as_ref().map(|r| r.len()), Some(2));
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn spec_driven_pause_and_resume() {
    let spec = AgentSpec::from_yaml_str(RAG_SPIKE_SPEC).expect("spec should parse");
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::AwaitingApproval);

    let result = runtime
        .resume(result.run_id, CheckpointDecision { approved: true, reason: None })
        .await
        .expect("resume failed");

    assert_eq!(result.state, AgentState::Completed);
    assert!(result.answer.is_some());
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn spec_from_yaml_file() {
    let spec =
        AgentSpec::from_yaml_file(std::path::Path::new("../../config/agents/rag_spike.yaml"))
            .await
            .expect("should load spec from file");
    assert_eq!(spec.agent_id, "rag_spike");
    assert_eq!(spec.graph.edges.len(), 4);
    assert_eq!(spec.checkpoints.len(), 1);

    // Verify the loaded spec drives a full run.
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(AutoApprove),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::Completed);
    assert!(result.answer.is_some());
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn spec_auto_approve_skips_port() {
    // Spec with approval_type: auto — should complete without querying ManualApproval port.
    let yaml = r#"
agent_id: auto_approve
description: "Auto-approve test"
spec_version: "1.0"
tasks:
  - classify
  - hybrid_search
  - summarize
  - approval_checkpoint
  - final_answer
graph:
  start_task: classify
  tasks: [classify, hybrid_search, summarize, approval_checkpoint, final_answer]
  edges:
    - { from: classify, to: hybrid_search }
    - { from: hybrid_search, to: summarize }
    - { from: summarize, to: approval_checkpoint }
    - { from: approval_checkpoint, to: final_answer }
checkpoints:
  - checkpoint_id: auto_cp
    after_task: summarize
    approval_type: auto
"#;
    let spec = AgentSpec::from_yaml_str(yaml).expect("spec should parse");

    // Use ManualApproval port — which always returns None (would pause).
    // But approval_type: auto in the spec should bypass the port entirely.
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::Completed, "auto-approve should complete without pausing");
    assert!(result.answer.is_some());
    assert!(result.pending_checkpoint.is_none());
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn spec_checkpoint_config_surfaces_in_pending() {
    let spec = AgentSpec::from_yaml_str(RAG_SPIKE_SPEC).expect("spec should parse");
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(ManualApproval),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::AwaitingApproval);

    let cp = result.pending_checkpoint.as_ref().expect("expected pending checkpoint");
    assert_eq!(cp.after_task, "summarize");
    assert_eq!(cp.checkpoint_id.as_deref(), Some("post_summary"));
    assert_eq!(cp.timeout_seconds, Some(3600)); // default from spec
    assert_eq!(cp.on_timeout.as_deref(), Some("reject")); // default from spec
}
