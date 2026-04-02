//! Graph-flow-backed agent runtime.
//!
//! All graph-flow types (`Context`, `Session`, `Graph`, `NextAction`,
//! `WaitForInput`) are confined to this module. The rest of agent-core
//! interacts only through [`super::AgentRuntime`] and the domain types
//! in [`crate::types`].

mod keys;
pub mod tasks;

use std::sync::Arc;
use std::time::Instant;

use graph_flow::{Graph, GraphBuilder, InMemorySessionStorage, Session, SessionStorage};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::ports::{ApprovalPort, BaselineAnswerPort, ChatPort, RetrievalPort};
use crate::spec::AgentSpec;
use crate::types::{
    AgentRunConfig, AgentRunResult, AgentState, CheckpointDecision, PendingCheckpoint, QueryType,
    RouteDecision, RunId, ScoredChunk, StepRecord, StepStatus,
};

use tasks::*;

// ---------------------------------------------------------------------------
// Stored session metadata (not graph-flow types)
// ---------------------------------------------------------------------------

/// Metadata we track per run alongside the graph-flow session.
#[allow(dead_code)]
struct RunMeta {
    run_id: RunId,
    query: String,
    collection: String,
    tenant: String,
    steps: Vec<StepRecord>,
    /// The last observed state — used by `inspect()` to avoid lossy inference.
    last_state: AgentState,
    /// Preserved from `AgentRunConfig` so `resume()` honours the same limit.
    max_steps: usize,
}

// ---------------------------------------------------------------------------
// GraphFlowRuntime
// ---------------------------------------------------------------------------

/// [`crate::runtime::AgentRuntime`] backed by graph-flow.
pub struct GraphFlowRuntime {
    retrieval: Arc<dyn RetrievalPort>,
    chat: Arc<dyn ChatPort>,
    approval: Arc<dyn ApprovalPort>,
    baseline: Arc<dyn BaselineAnswerPort>,
    /// Optional spec driving graph construction. When `None`, falls back to
    /// the hardcoded 5-task spike graph.
    spec: Option<AgentSpec>,
    /// In-memory session storage. Replace with Postgres for production persistence.
    sessions: Arc<InMemorySessionStorage>,
    /// Run metadata indexed by run ID.
    meta: Arc<Mutex<std::collections::HashMap<RunId, RunMeta>>>,
}

struct UnavailableBaselineAnswerPort;

#[async_trait::async_trait]
impl BaselineAnswerPort for UnavailableBaselineAnswerPort {
    async fn answer_single_shot(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<crate::types::GroundedAnswer> {
        anyhow::bail!("baseline answer port is not configured for this runtime")
    }
}

impl GraphFlowRuntime {
    /// Create a runtime with the hardcoded spike graph (no spec).
    pub fn new(
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
        baseline: Arc<dyn BaselineAnswerPort>,
    ) -> Self {
        Self {
            retrieval,
            chat,
            approval,
            baseline,
            spec: None,
            sessions: Arc::new(InMemorySessionStorage::new()),
            meta: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a runtime with the hardcoded spike graph and no baseline-answer port.
    pub fn new_without_baseline(
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
    ) -> Self {
        Self::new(retrieval, chat, approval, Arc::new(UnavailableBaselineAnswerPort))
    }

    /// Create a runtime driven by a YAML agent spec.
    pub fn from_spec(
        spec: AgentSpec,
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
        baseline: Arc<dyn BaselineAnswerPort>,
    ) -> Self {
        Self {
            retrieval,
            chat,
            approval,
            baseline,
            spec: Some(spec),
            sessions: Arc::new(InMemorySessionStorage::new()),
            meta: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a runtime driven by a YAML agent spec and no baseline-answer port.
    pub fn from_spec_without_baseline(
        spec: AgentSpec,
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
    ) -> Self {
        Self::from_spec(spec, retrieval, chat, approval, Arc::new(UnavailableBaselineAnswerPort))
    }

    /// Map a task ID from a spec to its concrete [`graph_flow::Task`] implementation.
    fn make_task(&self, task_id: &str) -> Option<Arc<dyn graph_flow::Task>> {
        match task_id {
            ROUTE_QUERY_TASK => Some(Arc::new(RouteQueryTask)),
            BASELINE_ANSWER_TASK => {
                Some(Arc::new(BaselineAnswerTask { baseline: self.baseline.clone() }))
            }
            RETRIEVE_EVIDENCE_TASK => {
                Some(Arc::new(RetrieveEvidenceTask { retrieval: self.retrieval.clone() }))
            }
            COMPOSE_ANSWER_TASK => Some(Arc::new(ComposeAnswerTask { chat: self.chat.clone() })),
            CLASSIFY_TASK => Some(Arc::new(ClassifyTask)),
            HYBRID_SEARCH_TASK => {
                Some(Arc::new(HybridSearchTask { retrieval: self.retrieval.clone() }))
            }
            SUMMARIZE_TASK => Some(Arc::new(SummarizeTask { chat: self.chat.clone() })),
            APPROVAL_CHECKPOINT_TASK => {
                let config = self.checkpoint_config_for_task(task_id);
                Some(Arc::new(ApprovalCheckpointTask { approval: self.approval.clone(), config }))
            }
            FINAL_ANSWER_TASK => Some(Arc::new(FinalAnswerTask)),
            _ => None,
        }
    }

    /// Find the checkpoint config that applies to a given checkpoint task by
    /// looking at which tasks precede it in the graph and matching against
    /// `after_task` in the checkpoint configs.
    fn checkpoint_config_for_task(
        &self,
        checkpoint_task_id: &str,
    ) -> Option<crate::spec::AgentCheckpointConfig> {
        let spec = self.spec.as_ref()?;
        let predecessors: Vec<&str> = spec
            .graph
            .edges
            .iter()
            .filter(|e| e.to == checkpoint_task_id)
            .map(|e| e.from.as_str())
            .collect();
        spec.checkpoints.iter().find(|cp| predecessors.contains(&cp.after_task.as_str())).cloned()
    }

    /// Build the graph. When a spec is present, its topology drives
    /// construction; otherwise falls back to the hardcoded spike graph.
    fn build_graph(&self) -> anyhow::Result<Graph> {
        match &self.spec {
            Some(spec) => self.build_graph_from_spec(spec),
            None => Ok(self.build_default_graph()),
        }
    }

    /// Hardcoded 5-task spike graph (backward-compat for existing tests).
    fn build_default_graph(&self) -> Graph {
        GraphBuilder::new("agent_spike")
            .add_task(Arc::new(ClassifyTask))
            .add_task(Arc::new(HybridSearchTask { retrieval: self.retrieval.clone() }))
            .add_task(Arc::new(SummarizeTask { chat: self.chat.clone() }))
            .add_task(Arc::new(ApprovalCheckpointTask {
                approval: self.approval.clone(),
                config: None,
            }))
            .add_task(Arc::new(FinalAnswerTask))
            .set_start_task(CLASSIFY_TASK)
            .add_edge(CLASSIFY_TASK, HYBRID_SEARCH_TASK)
            .add_edge(HYBRID_SEARCH_TASK, SUMMARIZE_TASK)
            .add_edge(SUMMARIZE_TASK, APPROVAL_CHECKPOINT_TASK)
            .add_edge(APPROVAL_CHECKPOINT_TASK, FINAL_ANSWER_TASK)
            .build()
    }

    /// Build a graph from a parsed [`AgentSpec`].
    fn build_graph_from_spec(&self, spec: &AgentSpec) -> anyhow::Result<Graph> {
        let mut builder = GraphBuilder::new(&spec.agent_id);

        for task_id in &spec.graph.tasks {
            let task = self.make_task(task_id).ok_or_else(|| {
                anyhow::anyhow!("spec references unknown task implementation '{task_id}'")
            })?;
            builder = builder.add_task(task);
        }

        builder = builder.set_start_task(&spec.graph.start_task);

        // Group edges by source task for conditional routing.
        let mut edges_by_from: std::collections::HashMap<&str, Vec<&crate::spec::AgentGraphEdge>> =
            std::collections::HashMap::new();
        for edge in &spec.graph.edges {
            edges_by_from.entry(edge.from.as_str()).or_default().push(edge);
        }

        for edges in edges_by_from.values() {
            let unconditional: Vec<&crate::spec::AgentGraphEdge> =
                edges.iter().copied().filter(|e| e.condition_key.is_none()).collect();
            let conditional: Vec<&crate::spec::AgentGraphEdge> =
                edges.iter().copied().filter(|e| e.condition_key.is_some()).collect();

            if conditional.is_empty() {
                // All edges are unconditional — add them directly.
                for edge in edges {
                    builder = builder.add_edge(&edge.from, &edge.to);
                }
            } else {
                // Conditional edges require an unconditional fallback to serve
                // as the "else" branch. Without one, both true and false paths
                // would route to the same target, making the condition a no-op.
                let fallback_to =
                    unconditional.first().map(|e| e.to.as_str()).ok_or_else(|| {
                        let from = &conditional[0].from;
                        anyhow::anyhow!(
                            "task '{from}' has conditional edge(s) but no unconditional \
                         fallback edge — add an unconditional edge as the 'else' path"
                        )
                    })?;

                // Only one conditional edge per source is supported —
                // graph-flow's behavior for multiple registrations is undefined.
                if conditional.len() > 1 {
                    let from = &conditional[0].from;
                    anyhow::bail!(
                        "task '{from}' has {} conditional edges — \
                         only one conditional edge per source is supported",
                        conditional.len()
                    );
                }

                for cond_edge in &conditional {
                    // Safe: filter guarantees condition_key is Some.
                    let key = cond_edge.condition_key.clone().unwrap_or_default();
                    let yes_target = cond_edge.to.clone();
                    let no_target = fallback_to.to_string();

                    builder = builder.add_conditional_edge(
                        &cond_edge.from,
                        move |ctx: &graph_flow::Context| -> bool {
                            ctx.get_sync::<bool>(&key).unwrap_or(false)
                        },
                        yes_target,
                        no_target,
                    );
                }

                // Unconditional edges beyond the first (used as fallback) are
                // added as regular edges.
                for unc_edge in unconditional.iter().skip(1) {
                    builder = builder.add_edge(&unc_edge.from, &unc_edge.to);
                }
            }
        }

        Ok(builder.build())
    }

    /// Execute the graph session until it pauses or completes, collecting
    /// step records along the way.
    async fn execute_loop(
        &self,
        graph: &Graph,
        session: &mut Session,
        steps: &mut Vec<StepRecord>,
        max_steps: usize,
    ) -> anyhow::Result<AgentState> {
        for _ in 0..max_steps {
            let task_id = session.current_task_id.clone();
            let step_start = Instant::now();
            let started_at = chrono::Utc::now();

            let result = match graph.execute_session(session).await {
                Ok(r) => r,
                Err(e) => {
                    let elapsed_ms = step_start.elapsed().as_millis() as u64;
                    warn!(error = %e, task = %task_id, "task execution error");
                    steps.push(StepRecord {
                        task_id,
                        started_at,
                        elapsed_ms,
                        status: StepStatus::Failed,
                    });
                    return Ok(AgentState::Failed);
                }
            };

            let elapsed_ms = step_start.elapsed().as_millis() as u64;

            match result.status {
                graph_flow::ExecutionStatus::Completed => {
                    steps.push(StepRecord {
                        task_id,
                        started_at,
                        elapsed_ms,
                        status: StepStatus::Completed,
                    });
                    info!("graph completed");
                    return Ok(AgentState::Completed);
                }
                graph_flow::ExecutionStatus::WaitingForInput => {
                    steps.push(StepRecord {
                        task_id,
                        started_at,
                        elapsed_ms,
                        status: StepStatus::Paused,
                    });
                    info!("graph paused — awaiting input");
                    return Ok(AgentState::AwaitingApproval);
                }
                graph_flow::ExecutionStatus::Paused { .. } => {
                    steps.push(StepRecord {
                        task_id,
                        started_at,
                        elapsed_ms,
                        status: StepStatus::Completed,
                    });
                    // Paused with next_task means we should continue stepping.
                }
                graph_flow::ExecutionStatus::Error(ref e) => {
                    warn!(error = %e, task = %task_id, "task failed");
                    steps.push(StepRecord {
                        task_id,
                        started_at,
                        elapsed_ms,
                        status: StepStatus::Failed,
                    });
                    return Ok(AgentState::Failed);
                }
            }
        }

        warn!("max steps ({}) reached", max_steps);
        Ok(AgentState::Failed)
    }

    /// Extract domain results from the graph-flow context.
    async fn extract_results(
        &self,
        context: &graph_flow::Context,
        run_id: RunId,
        state: AgentState,
        steps: Vec<StepRecord>,
    ) -> AgentRunResult {
        let answer: Option<String> = context.get(keys::FINAL_ANSWER).await;
        let summary: Option<String> = context.get(keys::SUMMARY).await;
        let search_results: Option<Vec<ScoredChunk>> = context.get(keys::SEARCH_RESULTS).await;
        let query_type: Option<QueryType> = context.get(keys::QUERY_TYPE).await;
        let route_decision: Option<RouteDecision> = context.get(keys::ROUTE_DECISION).await;

        // Read PendingCheckpoint from context (written by ApprovalCheckpointTask).
        // Only present when the run is awaiting approval.
        let pending_checkpoint: Option<PendingCheckpoint> = if state == AgentState::AwaitingApproval
        {
            context.get(keys::PENDING_CHECKPOINT).await
        } else {
            None
        };

        AgentRunResult {
            run_id,
            state,
            answer,
            summary,
            search_results,
            query_type,
            route_decision,
            pending_checkpoint,
            steps,
        }
    }
}

#[async_trait::async_trait]
impl super::AgentRuntime for GraphFlowRuntime {
    async fn start(&self, config: AgentRunConfig) -> anyhow::Result<AgentRunResult> {
        let run_id = Uuid::new_v4();
        let graph = self.build_graph()?;

        // Seed the context with run inputs.
        let context = graph_flow::Context::new();
        context.set(keys::QUERY, &config.query).await;
        context.set(keys::COLLECTION, &config.collection).await;
        context.set(keys::TENANT, &config.tenant).await;

        let start_task =
            self.spec.as_ref().map(|s| s.graph.start_task.as_str()).unwrap_or(CLASSIFY_TASK);
        let mut session = Session::new_from_task(run_id.to_string(), start_task);
        session.context = context;
        let mut steps = Vec::new();

        let state = self.execute_loop(&graph, &mut session, &mut steps, config.max_steps).await?;

        // Persist session for potential resume.
        self.sessions
            .save(session.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to save session: {e}"))?;

        let result = self.extract_results(&session.context, run_id, state, steps.clone()).await;

        // Save metadata for inspect/resume.
        self.meta.lock().await.insert(
            run_id,
            RunMeta {
                run_id,
                query: config.query,
                collection: config.collection,
                tenant: config.tenant,
                steps,
                last_state: state,
                max_steps: config.max_steps,
            },
        );

        Ok(result)
    }

    async fn resume(
        &self,
        run_id: RunId,
        decision: CheckpointDecision,
    ) -> anyhow::Result<AgentRunResult> {
        // Guard + extract in a single lock scope: check state, atomically
        // transition to Running (preventing concurrent resumes), and clone
        // out the data we need for execute_loop.
        let (mut steps, max_steps) = {
            let mut meta = self.meta.lock().await;
            let run_meta = meta
                .get_mut(&run_id)
                .ok_or_else(|| anyhow::anyhow!("no metadata for run {run_id}"))?;
            if run_meta.last_state != AgentState::AwaitingApproval {
                anyhow::bail!(
                    "run {run_id} is {:?}, not AwaitingApproval — cannot resume",
                    run_meta.last_state
                );
            }
            run_meta.last_state = AgentState::Running;
            (run_meta.steps.clone(), run_meta.max_steps)
        };

        let mut session = self
            .sessions
            .get(&run_id.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("session load error: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("no session found for run {run_id}"))?;

        // Inject the decision into the context so the checkpoint task sees it.
        session.context.set(keys::CHECKPOINT_APPROVED, &decision.approved).await;
        if let Some(ref reason) = decision.reason {
            session.context.set(keys::CHECKPOINT_REASON, reason).await;
        }

        let graph = self.build_graph()?;

        // Subtract steps already consumed so the total across start + resume(s)
        // never exceeds the original max_steps budget.
        let remaining = max_steps.saturating_sub(steps.len());
        let state = self.execute_loop(&graph, &mut session, &mut steps, remaining).await?;

        self.sessions
            .save(session.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to save session: {e}"))?;

        // Re-acquire the lock to merge results back.
        {
            let mut meta = self.meta.lock().await;
            if let Some(run_meta) = meta.get_mut(&run_id) {
                run_meta.steps = steps.clone();
                run_meta.last_state = state;
            }
        }

        let result = self.extract_results(&session.context, run_id, state, steps).await;

        Ok(result)
    }

    async fn inspect(&self, run_id: RunId) -> anyhow::Result<Option<AgentRunResult>> {
        let session = self
            .sessions
            .get(&run_id.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("session load error: {e}"))?;

        let session = match session {
            Some(s) => s,
            None => return Ok(None),
        };

        let meta = self.meta.lock().await;
        let run_meta = match meta.get(&run_id) {
            Some(m) => m,
            None => return Ok(None),
        };

        let result = self
            .extract_results(&session.context, run_id, run_meta.last_state, run_meta.steps.clone())
            .await;

        Ok(Some(result))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test assertions and lock checks use expect()/unwrap()
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::ports::BaselineAnswerPort;
    use crate::runtime::AgentRuntime;
    use crate::types::{
        AgentRunConfig, AgentState, GroundedAnswer, QueryClass, RetrievalProfileId, RouteDecision,
        RoutePath,
    };
    struct StubRetrieval;
    struct StubChat;
    struct StubApproval;

    #[derive(Debug, Default, Clone)]
    struct RoutedCallState {
        baseline_queries: Vec<String>,
        retrieval_calls: Vec<&'static str>,
        compose_queries: Vec<String>,
        expand_requests: Vec<(String, String, i32, i32, i32)>,
    }

    struct TrackingRetrieval {
        state: Arc<Mutex<RoutedCallState>>,
        dense_results: Vec<ScoredChunk>,
        fts_results: Vec<ScoredChunk>,
        hybrid_results: Vec<ScoredChunk>,
        expanded_results: Vec<ScoredChunk>,
    }

    struct TrackingChat {
        state: Arc<Mutex<RoutedCallState>>,
    }

    struct TrackingBaseline {
        state: Arc<Mutex<RoutedCallState>>,
        grounded_answer: GroundedAnswer,
    }

    #[async_trait::async_trait]
    impl RetrievalPort for StubRetrieval {
        async fn search_dense(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            Ok(Vec::new())
        }

        async fn search_sparse(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            Ok(Vec::new())
        }

        async fn search_hybrid(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            Ok(Vec::new())
        }

        async fn search_fts(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            Ok(Vec::new())
        }

        async fn expand_chunk_neighbors(
            &self,
            _: &str,
            _: &str,
            _: i32,
            _: i32,
            _: i32,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            Ok(Vec::new())
        }

        async fn fetch_document(&self, _: &str, _: &str) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
    }

    #[async_trait::async_trait]
    impl ChatPort for StubChat {
        async fn summarize(&self, _: &str, _: &[ScoredChunk], _: &str) -> anyhow::Result<String> {
            Ok(String::new())
        }
    }

    #[async_trait::async_trait]
    impl ApprovalPort for StubApproval {
        async fn request_approval(
            &self,
            _: &PendingCheckpoint,
        ) -> anyhow::Result<Option<CheckpointDecision>> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl RetrievalPort for TrackingRetrieval {
        async fn search_dense(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            self.state.lock().expect("retrieval state").retrieval_calls.push("dense");
            Ok(self.dense_results.clone())
        }

        async fn search_sparse(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            self.state.lock().expect("retrieval state").retrieval_calls.push("sparse");
            Ok(self.dense_results.clone())
        }

        async fn search_hybrid(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            self.state.lock().expect("retrieval state").retrieval_calls.push("hybrid");
            Ok(self.hybrid_results.clone())
        }

        async fn search_fts(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            self.state.lock().expect("retrieval state").retrieval_calls.push("fts");
            Ok(self.fts_results.clone())
        }

        async fn expand_chunk_neighbors(
            &self,
            tenant: &str,
            document_id: &str,
            chunk_index: i32,
            before: i32,
            after: i32,
        ) -> anyhow::Result<Vec<ScoredChunk>> {
            let mut state = self.state.lock().expect("retrieval state");
            state.retrieval_calls.push("expand");
            state.expand_requests.push((
                tenant.to_string(),
                document_id.to_string(),
                chunk_index,
                before,
                after,
            ));
            Ok(self.expanded_results.clone())
        }

        async fn fetch_document(&self, _: &str, _: &str) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
    }

    #[async_trait::async_trait]
    impl ChatPort for TrackingChat {
        async fn summarize(
            &self,
            query: &str,
            chunks: &[ScoredChunk],
            _: &str,
        ) -> anyhow::Result<String> {
            self.state.lock().expect("chat state").compose_queries.push(query.to_string());
            Ok(format!("composed '{}' from {} chunks", query, chunks.len()))
        }
    }

    #[async_trait::async_trait]
    impl BaselineAnswerPort for TrackingBaseline {
        async fn answer_single_shot(
            &self,
            query: &str,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<GroundedAnswer> {
            self.state.lock().expect("baseline state").baseline_queries.push(query.to_string());
            Ok(self.grounded_answer.clone())
        }
    }

    fn test_chunk_with(
        chunk_id: &str,
        document_id: &str,
        chunk_index: i32,
        text: &str,
    ) -> ScoredChunk {
        ScoredChunk {
            chunk_id: chunk_id.to_string(),
            document_id: document_id.to_string(),
            chunk_index,
            text: text.to_string(),
            title: None,
            source_url: None,
            source_domain: None,
            language: Some("en".to_string()),
            tags: Vec::new(),
            section_heading: None,
            collection: Some("docs".to_string()),
            score: 0.9,
            score_type: "test".to_string(),
            sources: Vec::new(),
            source_scores: std::collections::HashMap::new(),
        }
    }

    fn routed_spec() -> AgentSpec {
        AgentSpec::from_yaml_str(
            r#"
agent_id: agentic_search_v1
description: "Milestone 1 routed search graph"
spec_version: "1.0"
required_tools:
  - retrieval.dense
  - retrieval.sparse
  - retrieval.hybrid
  - retrieval.fts
  - retrieval.expand_chunk_neighbors
  - retrieval.fetch_document
tasks:
  - route_query
  - baseline_answer
  - retrieve_evidence
  - compose_answer
  - final_answer
graph:
  start_task: route_query
  tasks:
    - route_query
    - baseline_answer
    - retrieve_evidence
    - compose_answer
    - final_answer
  edges:
    - { from: route_query, to: retrieve_evidence, condition_key: route_to_agentic_search }
    - { from: route_query, to: baseline_answer }
    - { from: retrieve_evidence, to: compose_answer }
    - { from: baseline_answer, to: final_answer }
    - { from: compose_answer, to: final_answer }
"#,
        )
        .expect("routed spec should parse")
    }

    fn routed_config(query: &str) -> AgentRunConfig {
        AgentRunConfig {
            query: query.to_string(),
            collection: "docs".to_string(),
            tenant: "tenant-a".to_string(),
            max_steps: 10,
        }
    }

    #[tokio::test]
    async fn extract_results_reads_route_decision_from_context() {
        let runtime = GraphFlowRuntime::new_without_baseline(
            Arc::new(StubRetrieval),
            Arc::new(StubChat),
            Arc::new(StubApproval),
        );
        let context = graph_flow::Context::new();
        let decision = RouteDecision {
            selected_path: RoutePath::AgenticSearch,
            query_class: QueryClass::Procedural,
            retrieval_profile: RetrievalProfileId::LexicalFirst,
            ambiguity: false,
            needs_multi_hop: false,
            needs_high_evidence: true,
            time_sensitive: false,
            normalized_filters: Vec::new(),
            reasons: vec!["procedural".to_string()],
        };
        context.set(keys::ROUTE_DECISION, &decision).await;

        let result = runtime
            .extract_results(&context, Uuid::new_v4(), AgentState::Completed, Vec::new())
            .await;

        assert_eq!(result.route_decision, Some(decision));
    }

    #[tokio::test]
    async fn routed_spec_uses_baseline_answer_for_single_pass_queries() {
        let state = Arc::new(Mutex::new(RoutedCallState::default()));
        let runtime = GraphFlowRuntime::from_spec(
            routed_spec(),
            Arc::new(TrackingRetrieval {
                state: state.clone(),
                dense_results: Vec::new(),
                fts_results: Vec::new(),
                hybrid_results: vec![test_chunk_with(
                    "chunk-retrieved",
                    "doc-1",
                    0,
                    "retrieved evidence",
                )],
                expanded_results: Vec::new(),
            }),
            Arc::new(TrackingChat { state: state.clone() }),
            Arc::new(StubApproval),
            Arc::new(TrackingBaseline {
                state: state.clone(),
                grounded_answer: GroundedAnswer {
                    answer: "baseline answer".to_string(),
                    search_results: vec![test_chunk_with(
                        "chunk-baseline",
                        "doc-1",
                        0,
                        "baseline evidence",
                    )],
                    citations: vec!["doc-1".to_string()],
                    model: "baseline-model".to_string(),
                },
            }),
        );

        let result = runtime
            .start(routed_config("What is Rust?"))
            .await
            .expect("single-pass route should complete");

        assert_eq!(result.state, AgentState::Completed);
        assert_eq!(result.answer.as_deref(), Some("baseline answer"));
        assert_eq!(
            result.route_decision.as_ref().map(|d| d.selected_path),
            Some(RoutePath::SinglePassRag)
        );

        let state = state.lock().expect("call state");
        assert_eq!(state.baseline_queries, vec!["What is Rust?".to_string()]);
        assert!(state.retrieval_calls.is_empty(), "single-pass route should not retrieve evidence");
        assert!(state.compose_queries.is_empty(), "single-pass route should not compose");
    }

    #[tokio::test]
    async fn routed_spec_uses_retrieval_and_composition_for_agentic_queries() {
        let state = Arc::new(Mutex::new(RoutedCallState::default()));
        let runtime = GraphFlowRuntime::from_spec(
            routed_spec(),
            Arc::new(TrackingRetrieval {
                state: state.clone(),
                dense_results: Vec::new(),
                fts_results: vec![test_chunk_with("chunk-fts", "doc-1", 0, "fts evidence")],
                hybrid_results: vec![test_chunk_with(
                    "chunk-hybrid",
                    "doc-2",
                    0,
                    "hybrid evidence",
                )],
                expanded_results: Vec::new(),
            }),
            Arc::new(TrackingChat { state: state.clone() }),
            Arc::new(StubApproval),
            Arc::new(TrackingBaseline {
                state: state.clone(),
                grounded_answer: GroundedAnswer {
                    answer: "baseline should not run".to_string(),
                    search_results: Vec::new(),
                    citations: Vec::new(),
                    model: "baseline-model".to_string(),
                },
            }),
        );

        let result = runtime
            .start(routed_config("How do I rotate API keys in the auth runbook?"))
            .await
            .expect("agentic route should complete");

        assert_eq!(result.state, AgentState::Completed);
        assert_eq!(
            result.route_decision.as_ref().map(|d| d.selected_path),
            Some(RoutePath::AgenticSearch)
        );
        assert_eq!(
            result.route_decision.as_ref().map(|d| d.retrieval_profile),
            Some(RetrievalProfileId::LexicalFirst)
        );
        assert_eq!(
            result.answer.as_deref(),
            Some("composed 'How do I rotate API keys in the auth runbook?' from 2 chunks")
        );

        let state = state.lock().expect("call state");
        assert!(state.baseline_queries.is_empty(), "agentic route should bypass baseline answer");
        assert_eq!(state.retrieval_calls, vec!["fts", "hybrid"]);
        assert_eq!(
            state.compose_queries,
            vec!["How do I rotate API keys in the auth runbook?".to_string()]
        );
    }

    #[tokio::test]
    async fn routed_spec_deduplicates_overlapping_lexical_first_results() {
        let state = Arc::new(Mutex::new(RoutedCallState::default()));
        let runtime = GraphFlowRuntime::from_spec(
            routed_spec(),
            Arc::new(TrackingRetrieval {
                state: state.clone(),
                dense_results: Vec::new(),
                fts_results: vec![test_chunk_with("chunk-shared", "doc-1", 0, "fts evidence")],
                hybrid_results: vec![
                    test_chunk_with("chunk-shared", "doc-1", 0, "hybrid duplicate"),
                    test_chunk_with("chunk-unique", "doc-2", 0, "hybrid unique"),
                ],
                expanded_results: Vec::new(),
            }),
            Arc::new(TrackingChat { state: state.clone() }),
            Arc::new(StubApproval),
            Arc::new(TrackingBaseline {
                state: state.clone(),
                grounded_answer: GroundedAnswer {
                    answer: "baseline should not run".to_string(),
                    search_results: Vec::new(),
                    citations: Vec::new(),
                    model: "baseline-model".to_string(),
                },
            }),
        );

        let result = runtime
            .start(routed_config("How do I rotate API keys in the auth runbook?"))
            .await
            .expect("agentic route should complete");

        assert_eq!(
            result.answer.as_deref(),
            Some("composed 'How do I rotate API keys in the auth runbook?' from 2 chunks")
        );
        assert_eq!(result.search_results.as_ref().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn routed_spec_expands_neighbors_for_broad_then_expand_queries() {
        let state = Arc::new(Mutex::new(RoutedCallState::default()));
        let runtime = GraphFlowRuntime::from_spec(
            routed_spec(),
            Arc::new(TrackingRetrieval {
                state: state.clone(),
                dense_results: Vec::new(),
                fts_results: Vec::new(),
                hybrid_results: vec![test_chunk_with(
                    "chunk-anchor",
                    "doc-1",
                    0,
                    "anchor evidence",
                )],
                expanded_results: vec![test_chunk_with(
                    "chunk-neighbor",
                    "doc-1",
                    1,
                    "neighbor evidence",
                )],
            }),
            Arc::new(TrackingChat { state: state.clone() }),
            Arc::new(StubApproval),
            Arc::new(TrackingBaseline {
                state: state.clone(),
                grounded_answer: GroundedAnswer {
                    answer: "baseline should not run".to_string(),
                    search_results: Vec::new(),
                    citations: Vec::new(),
                    model: "baseline-model".to_string(),
                },
            }),
        );

        let result = runtime
            .start(routed_config("Compare Rust and Python tradeoffs for async services"))
            .await
            .expect("broad-then-expand route should complete");

        assert_eq!(result.state, AgentState::Completed);
        assert_eq!(
            result.route_decision.as_ref().map(|d| d.retrieval_profile),
            Some(RetrievalProfileId::BroadThenExpand)
        );
        assert_eq!(
            result.answer.as_deref(),
            Some("composed 'Compare Rust and Python tradeoffs for async services' from 2 chunks")
        );

        let state = state.lock().expect("call state");
        assert_eq!(state.retrieval_calls, vec!["hybrid", "expand"]);
        assert_eq!(
            state.expand_requests,
            vec![("tenant-a".to_string(), "doc-1".to_string(), 0, 1, 1)]
        );
    }

    #[tokio::test]
    async fn routed_spec_deduplicates_anchor_from_neighbor_expansion() {
        let state = Arc::new(Mutex::new(RoutedCallState::default()));
        let runtime = GraphFlowRuntime::from_spec(
            routed_spec(),
            Arc::new(TrackingRetrieval {
                state: state.clone(),
                dense_results: Vec::new(),
                fts_results: Vec::new(),
                hybrid_results: vec![test_chunk_with(
                    "chunk-anchor",
                    "doc-1",
                    0,
                    "anchor evidence",
                )],
                expanded_results: vec![
                    test_chunk_with("chunk-anchor", "doc-1", 0, "anchor duplicate"),
                    test_chunk_with("chunk-neighbor", "doc-1", 1, "neighbor evidence"),
                ],
            }),
            Arc::new(TrackingChat { state: state.clone() }),
            Arc::new(StubApproval),
            Arc::new(TrackingBaseline {
                state: state.clone(),
                grounded_answer: GroundedAnswer {
                    answer: "baseline should not run".to_string(),
                    search_results: Vec::new(),
                    citations: Vec::new(),
                    model: "baseline-model".to_string(),
                },
            }),
        );

        let result = runtime
            .start(routed_config("Compare Rust and Python tradeoffs for async services"))
            .await
            .expect("broad-then-expand route should complete");

        assert_eq!(
            result.answer.as_deref(),
            Some("composed 'Compare Rust and Python tradeoffs for async services' from 2 chunks")
        );
        assert_eq!(result.search_results.as_ref().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn loads_agentic_search_v1_spec_file() {
        let spec = AgentSpec::from_yaml_file(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/agents/agentic_search_v1.yaml"
        )))
        .await
        .expect("agentic_search_v1 spec should load");

        assert_eq!(spec.agent_id, "agentic_search_v1");
        assert_eq!(spec.graph.start_task, "route_query");
        let required_tools: HashSet<_> = spec.required_tools.iter().cloned().collect();
        let expected_tools: HashSet<_> = [
            "retrieval.dense".to_string(),
            "retrieval.sparse".to_string(),
            "retrieval.hybrid".to_string(),
            "retrieval.fts".to_string(),
            "retrieval.expand_chunk_neighbors".to_string(),
            "retrieval.fetch_document".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(required_tools, expected_tools);
        assert!(spec.graph.tasks.iter().any(|task| task == "final_answer"));
        assert_eq!(spec.graph.edges.len(), 5);
    }
}
