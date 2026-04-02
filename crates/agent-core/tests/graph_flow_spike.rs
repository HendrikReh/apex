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

fn test_chunk(
    chunk_id: &str,
    document_id: &str,
    chunk_index: i32,
    text: &str,
    score: f32,
) -> ScoredChunk {
    ScoredChunk {
        chunk_id: chunk_id.into(),
        document_id: document_id.into(),
        chunk_index,
        text: text.into(),
        title: None,
        source_url: None,
        source_domain: None,
        language: None,
        tags: Vec::new(),
        section_heading: None,
        collection: None,
        score,
        score_type: "test".into(),
        sources: Vec::new(),
        source_scores: std::collections::HashMap::new(),
    }
}

#[async_trait::async_trait]
impl RetrievalPort for MockRetrieval {
    async fn search_dense(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(Vec::new())
    }

    async fn search_sparse(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(Vec::new())
    }

    async fn search_hybrid(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(vec![
            test_chunk(
                "chunk-1",
                "doc-1",
                0,
                "Retrieval-Augmented Generation combines retrieval with generation.",
                0.95,
            ),
            test_chunk(
                "chunk-2",
                "doc-1",
                1,
                "RAG reduces hallucination by grounding answers in retrieved documents.",
                0.87,
            ),
        ])
    }

    async fn search_fts(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(Vec::new())
    }

    async fn expand_chunk_neighbors(
        &self,
        _tenant: &str,
        _document_id: &str,
        _chunk_index: i32,
        _before: i32,
        _after: i32,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        Ok(Vec::new())
    }

    async fn fetch_document(
        &self,
        _tenant: &str,
        _document_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({}))
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

// ---------------------------------------------------------------------------
// Conditional edge tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn conditional_edge_takes_conditional_path_when_key_is_true() {
    // Build a minimal graph directly via graph-flow APIs to verify that
    // a conditional edge routes to the "yes" target when the key is set.
    use graph_flow::{Context, GraphBuilder, NextAction, Session, Task, TaskResult};

    struct SetFlagTask;
    #[async_trait::async_trait]
    impl Task for SetFlagTask {
        fn id(&self) -> &str {
            "set_flag"
        }
        async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
            ctx.set("take_shortcut", &true).await;
            Ok(TaskResult::new(Some("flag set".into()), NextAction::Continue))
        }
    }

    struct YesTask;
    #[async_trait::async_trait]
    impl Task for YesTask {
        fn id(&self) -> &str {
            "yes_path"
        }
        async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
            ctx.set("which_path", &"yes".to_string()).await;
            Ok(TaskResult::new(Some("yes".into()), NextAction::End))
        }
    }

    struct NoTask;
    #[async_trait::async_trait]
    impl Task for NoTask {
        fn id(&self) -> &str {
            "no_path"
        }
        async fn run(&self, ctx: Context) -> graph_flow::Result<TaskResult> {
            ctx.set("which_path", &"no".to_string()).await;
            Ok(TaskResult::new(Some("no".into()), NextAction::End))
        }
    }

    let graph = GraphBuilder::new("cond_true_test")
        .add_task(Arc::new(SetFlagTask))
        .add_task(Arc::new(YesTask))
        .add_task(Arc::new(NoTask))
        .set_start_task("set_flag")
        .add_conditional_edge(
            "set_flag",
            |ctx: &Context| -> bool { ctx.get_sync::<bool>("take_shortcut").unwrap_or(false) },
            "yes_path",
            "no_path",
        )
        .build();

    let mut session = Session::new_from_task("test-run".into(), "set_flag");

    // Execute until completion.
    for _ in 0..10 {
        let result = graph.execute_session(&mut session).await.expect("execution failed");
        match result.status {
            graph_flow::ExecutionStatus::Completed => break,
            graph_flow::ExecutionStatus::Paused { .. } => continue,
            other => panic!("unexpected status: {other:?}"),
        }
    }

    let path: String = session.context.get("which_path").await.expect("path not set");
    assert_eq!(path, "yes", "expected conditional (yes) path when key is true");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn conditional_edge_takes_unconditional_path_when_key_unset() {
    // Spec with a conditional edge: classify → summarize (condition: skip_search)
    // and an unconditional edge: classify → hybrid_search.
    // Since "skip_search" is never set to true, the unconditional path should be taken.
    let yaml = r#"
agent_id: cond_test
description: "Conditional edge test"
spec_version: "1.0"
tasks:
  - classify
  - hybrid_search
  - summarize
  - final_answer
graph:
  start_task: classify
  tasks: [classify, hybrid_search, summarize, final_answer]
  edges:
    - { from: classify, to: hybrid_search }
    - { from: classify, to: summarize, condition_key: skip_search }
    - { from: hybrid_search, to: summarize }
    - { from: summarize, to: final_answer }
"#;
    let spec = AgentSpec::from_yaml_str(yaml).expect("spec should parse");
    let runtime = GraphFlowRuntime::from_spec(
        spec,
        Arc::new(MockRetrieval),
        Arc::new(MockChat),
        Arc::new(AutoApprove),
    );

    let result = runtime.start(config()).await.expect("start failed");

    // The unconditional path should be taken: classify → hybrid_search → summarize → final_answer
    // This means search_results should be populated (hybrid_search ran).
    assert_eq!(result.state, AgentState::Completed);
    assert!(
        result.search_results.is_some(),
        "search results should exist because the unconditional path (through hybrid_search) was taken"
    );
    assert_eq!(result.search_results.as_ref().map(|r| r.len()), Some(2));
    assert!(result.answer.is_some());
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn multiple_conditional_edges_from_same_source_returns_error() {
    let yaml = r#"
agent_id: multi_cond
description: "Multiple conditionals from same source"
spec_version: "1.0"
tasks:
  - classify
  - hybrid_search
  - summarize
  - final_answer
graph:
  start_task: classify
  tasks: [classify, hybrid_search, summarize, final_answer]
  edges:
    - { from: classify, to: hybrid_search, condition_key: key_a }
    - { from: classify, to: summarize, condition_key: key_b }
    - { from: classify, to: final_answer }
    - { from: hybrid_search, to: final_answer }
    - { from: summarize, to: final_answer }
"#;
    let err = AgentSpec::from_yaml_str(yaml).expect_err("should reject multiple conditionals");
    let msg = err.to_string();
    assert!(msg.contains("conditional edges"), "expected multiple-conditional error, got: {msg}");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn conditional_edge_without_fallback_returns_error() {
    // Spec where classify has only a conditional edge and no unconditional fallback.
    let yaml = r#"
agent_id: no_fallback
description: "Missing fallback test"
spec_version: "1.0"
tasks:
  - classify
  - hybrid_search
  - summarize
  - final_answer
graph:
  start_task: classify
  tasks: [classify, hybrid_search, summarize, final_answer]
  edges:
    - { from: classify, to: hybrid_search, condition_key: do_search }
    - { from: hybrid_search, to: summarize }
    - { from: summarize, to: final_answer }
"#;
    let err = AgentSpec::from_yaml_str(yaml).expect_err("should fail without fallback");
    let msg = err.to_string();
    assert!(msg.contains("no unconditional fallback"), "expected fallback error, got: {msg}");
}

// ---------------------------------------------------------------------------
// Failed state tests
// ---------------------------------------------------------------------------

/// Retrieval port that always errors.
struct FailingRetrieval;

#[async_trait::async_trait]
impl RetrievalPort for FailingRetrieval {
    async fn search_dense(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        anyhow::bail!("simulated retrieval failure")
    }

    async fn search_sparse(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        anyhow::bail!("simulated retrieval failure")
    }

    async fn search_hybrid(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        anyhow::bail!("simulated retrieval failure")
    }

    async fn search_fts(
        &self,
        _collection: &str,
        _query: &str,
        _tenant: &str,
        _limit: u64,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        anyhow::bail!("simulated retrieval failure")
    }

    async fn expand_chunk_neighbors(
        &self,
        _tenant: &str,
        _document_id: &str,
        _chunk_index: i32,
        _before: i32,
        _after: i32,
    ) -> anyhow::Result<Vec<ScoredChunk>> {
        anyhow::bail!("simulated retrieval failure")
    }

    async fn fetch_document(
        &self,
        _tenant: &str,
        _document_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("simulated retrieval failure")
    }
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn retrieval_error_produces_failed_state() {
    let runtime = GraphFlowRuntime::new(
        Arc::new(FailingRetrieval),
        Arc::new(MockChat),
        Arc::new(AutoApprove),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::Failed, "retrieval error should produce Failed state");
    assert!(result.answer.is_none(), "failed run should have no answer");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn inspect_reports_failed_state_correctly() {
    let runtime = GraphFlowRuntime::new(
        Arc::new(FailingRetrieval),
        Arc::new(MockChat),
        Arc::new(AutoApprove),
    );

    let result = runtime.start(config()).await.expect("start failed");
    assert_eq!(result.state, AgentState::Failed);

    let inspected = runtime.inspect(result.run_id).await.expect("inspect failed");
    let inspected = inspected.expect("inspect should find the run");
    assert_eq!(
        inspected.state,
        AgentState::Failed,
        "inspect should report Failed, not AwaitingApproval"
    );
}
