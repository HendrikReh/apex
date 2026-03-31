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
}

// ---------------------------------------------------------------------------
// GraphFlowRuntime
// ---------------------------------------------------------------------------

/// [`crate::runtime::AgentRuntime`] backed by graph-flow.
pub struct GraphFlowRuntime {
    retrieval: Arc<dyn RetrievalPort>,
    chat: Arc<dyn ChatPort>,
    approval: Arc<dyn ApprovalPort>,
    /// In-memory session storage. Replace with Postgres for production persistence.
    sessions: Arc<InMemorySessionStorage>,
    /// Run metadata indexed by run ID.
    meta: Arc<Mutex<std::collections::HashMap<RunId, RunMeta>>>,
    max_steps: usize,
}

impl GraphFlowRuntime {
    pub fn new(
        retrieval: Arc<dyn RetrievalPort>,
        chat: Arc<dyn ChatPort>,
        approval: Arc<dyn ApprovalPort>,
    ) -> Self {
        Self {
            retrieval,
            chat,
            approval,
            sessions: Arc::new(InMemorySessionStorage::new()),
            meta: Arc::new(Mutex::new(std::collections::HashMap::new())),
            max_steps: 20,
        }
    }

    /// Build the 5-task graph for the vertical slice.
    fn build_graph(&self) -> Graph {
        GraphBuilder::new("agent_spike")
            .add_task(Arc::new(ClassifyTask))
            .add_task(Arc::new(HybridSearchTask { retrieval: self.retrieval.clone() }))
            .add_task(Arc::new(SummarizeTask { chat: self.chat.clone() }))
            .add_task(Arc::new(ApprovalCheckpointTask { approval: self.approval.clone() }))
            .add_task(Arc::new(FinalAnswerTask))
            .set_start_task(CLASSIFY_TASK)
            .add_edge(CLASSIFY_TASK, HYBRID_SEARCH_TASK)
            .add_edge(HYBRID_SEARCH_TASK, SUMMARIZE_TASK)
            .add_edge(SUMMARIZE_TASK, APPROVAL_CHECKPOINT_TASK)
            .add_edge(APPROVAL_CHECKPOINT_TASK, FINAL_ANSWER_TASK)
            .build()
    }

    /// Execute the graph session until it pauses or completes, collecting
    /// step records along the way.
    async fn execute_loop(
        &self,
        graph: &Graph,
        session: &mut Session,
        steps: &mut Vec<StepRecord>,
    ) -> anyhow::Result<AgentState> {
        for _ in 0..self.max_steps {
            let task_id = session.current_task_id.clone();
            let step_start = Instant::now();
            let started_at = chrono::Utc::now();

            let result = graph
                .execute_session(session)
                .await
                .map_err(|e| anyhow::anyhow!("graph execution error at task {task_id}: {e}"))?;

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

        warn!("max steps ({}) reached", self.max_steps);
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

        let pending_checkpoint = if state == AgentState::AwaitingApproval {
            Some(PendingCheckpoint {
                after_task: SUMMARIZE_TASK.to_string(),
                summary: summary.clone(),
            })
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
        let graph = self.build_graph();

        // Seed the context with run inputs.
        let context = graph_flow::Context::new();
        context.set(keys::QUERY, &config.query).await;
        context.set(keys::COLLECTION, &config.collection).await;
        context.set(keys::TENANT, &config.tenant).await;

        let mut session = Session::new_from_task(run_id.to_string(), CLASSIFY_TASK);
        session.context = context;
        let mut steps = Vec::new();

        let state = self.execute_loop(&graph, &mut session, &mut steps).await?;

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
            },
        );

        Ok(result)
    }

    async fn resume(
        &self,
        run_id: RunId,
        decision: CheckpointDecision,
    ) -> anyhow::Result<AgentRunResult> {
        let mut session = self
            .sessions
            .get(&run_id.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("session load error: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("no session found for run {run_id}"))?;

        // Inject the decision into the context so the checkpoint task sees it.
        session.context.set(keys::CHECKPOINT_APPROVED, &decision.approved).await;

        let graph = self.build_graph();
        let mut meta = self.meta.lock().await;
        let run_meta =
            meta.get_mut(&run_id).ok_or_else(|| anyhow::anyhow!("no metadata for run {run_id}"))?;

        let state = self.execute_loop(&graph, &mut session, &mut run_meta.steps).await?;

        self.sessions
            .save(session.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to save session: {e}"))?;

        let result =
            self.extract_results(&session.context, run_id, state, run_meta.steps.clone()).await;

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
        let run_meta = meta.get(&run_id);
        let steps = run_meta.map(|m| m.steps.clone()).unwrap_or_default();

        // Determine state from session status — if we got here and there's a
        // session, it's either completed or waiting.
        let has_answer = session.context.get::<String>(keys::FINAL_ANSWER).await.is_some();
        let state = if has_answer { AgentState::Completed } else { AgentState::AwaitingApproval };

        let result = self.extract_results(&session.context, run_id, state, steps).await;

        Ok(Some(result))
    }
}
