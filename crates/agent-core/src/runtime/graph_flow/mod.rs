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

use crate::ports::{ApprovalPort, ChatPort, RetrievalPort};
use crate::spec::AgentSpec;
use crate::types::{
    AgentRunConfig, AgentRunResult, AgentState, CheckpointDecision, PendingCheckpoint, QueryType,
    RunId, ScoredChunk, StepRecord, StepStatus,
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
    /// Optional spec driving graph construction. When `None`, falls back to
    /// the hardcoded 5-task spike graph.
    spec: Option<AgentSpec>,
    /// In-memory session storage. Replace with Postgres for production persistence.
    sessions: Arc<InMemorySessionStorage>,
    /// Run metadata indexed by run ID.
    meta: Arc<Mutex<std::collections::HashMap<RunId, RunMeta>>>,
}

impl GraphFlowRuntime {
    /// Create a runtime with the hardcoded spike graph (no spec).
    pub fn new(
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
    ) -> Self {
        Self {
            retrieval,
            chat,
            approval,
            spec: None,
            sessions: Arc::new(InMemorySessionStorage::new()),
            meta: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a runtime driven by a YAML agent spec.
    pub fn from_spec(
        spec: AgentSpec,
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
    ) -> Self {
        Self {
            retrieval,
            chat,
            approval,
            spec: Some(spec),
            sessions: Arc::new(InMemorySessionStorage::new()),
            meta: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Map a task ID from a spec to its concrete [`graph_flow::Task`] implementation.
    fn make_task(&self, task_id: &str) -> Option<Arc<dyn graph_flow::Task>> {
        match task_id {
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

        for edge in &spec.graph.edges {
            // TODO: plumb edge.condition_key to graph-flow builder once it
            // supports conditional routing. Currently all edges are unconditional.
            builder = builder.add_edge(&edge.from, &edge.to);
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
