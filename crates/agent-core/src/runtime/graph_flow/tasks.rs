//! Graph-flow `Task` implementations for the vertical slice.
//!
//! Each task reads/writes the shared `graph_flow::Context` using the keys
//! defined in [`super::keys`]. Business data is (de)serialized through the
//! agent-core domain types — graph-flow's `Context` is just the transport.

use std::sync::Arc;

use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::info;

use crate::ports::{ApprovalPort, ChatPort, RetrievalPort};
use crate::types::{PendingCheckpoint, ScoredChunk};

use super::keys;

// ---------------------------------------------------------------------------
// Task IDs (public so the graph builder and tests can reference them)
// ---------------------------------------------------------------------------

pub const CLASSIFY_TASK: &str = "classify";
pub const HYBRID_SEARCH_TASK: &str = "hybrid_search";
pub const SUMMARIZE_TASK: &str = "summarize";
pub const APPROVAL_CHECKPOINT_TASK: &str = "approval_checkpoint";
pub const FINAL_ANSWER_TASK: &str = "final_answer";

// ---------------------------------------------------------------------------
// Classify
// ---------------------------------------------------------------------------

/// Deterministic query classification. No LLM call.
pub struct ClassifyTask;

#[async_trait::async_trait]
impl Task for ClassifyTask {
    fn id(&self) -> &str {
        CLASSIFY_TASK
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let query: String = context.get(keys::QUERY).await.ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed("missing query in context".into())
        })?;

        let query_type = crate::classify::classify_query(&query);
        info!(query_type = ?query_type, "classified query");

        context.set(keys::QUERY_TYPE, &query_type).await;

        Ok(TaskResult::new(
            Some(serde_json::to_string(&query_type).unwrap_or_default()),
            NextAction::Continue,
        ))
    }
}

// ---------------------------------------------------------------------------
// Hybrid search
// ---------------------------------------------------------------------------

/// Executes hybrid retrieval via the [`RetrievalPort`].
pub struct HybridSearchTask {
    pub retrieval: Arc<dyn RetrievalPort>,
}

#[async_trait::async_trait]
impl Task for HybridSearchTask {
    fn id(&self) -> &str {
        HYBRID_SEARCH_TASK
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let query: String = context
            .get(keys::QUERY)
            .await
            .ok_or_else(|| graph_flow::GraphError::TaskExecutionFailed("missing query".into()))?;
        let collection: String = context.get(keys::COLLECTION).await.ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed("missing collection".into())
        })?;
        let tenant: String = context
            .get(keys::TENANT)
            .await
            .ok_or_else(|| graph_flow::GraphError::TaskExecutionFailed("missing tenant".into()))?;

        let results =
            self.retrieval.search_hybrid(&collection, &query, &tenant).await.map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!("retrieval failed: {e}"))
            })?;

        let success = !results.is_empty();
        info!(result_count = results.len(), "hybrid search completed");

        context.set(keys::SEARCH_RESULTS, &results).await;
        context.set(keys::SEARCH_SUCCESS, &success).await;

        Ok(TaskResult::new(Some(format!("{} results", results.len())), NextAction::Continue))
    }
}

// ---------------------------------------------------------------------------
// Summarize
// ---------------------------------------------------------------------------

/// Generates a summary from search results via the [`ChatPort`].
pub struct SummarizeTask {
    pub chat: Arc<dyn ChatPort>,
}

#[async_trait::async_trait]
impl Task for SummarizeTask {
    fn id(&self) -> &str {
        SUMMARIZE_TASK
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let query: String = context
            .get(keys::QUERY)
            .await
            .ok_or_else(|| graph_flow::GraphError::TaskExecutionFailed("missing query".into()))?;
        let tenant: String = context
            .get(keys::TENANT)
            .await
            .ok_or_else(|| graph_flow::GraphError::TaskExecutionFailed("missing tenant".into()))?;
        let results: Vec<ScoredChunk> = context.get(keys::SEARCH_RESULTS).await.unwrap_or_default();

        if results.is_empty() {
            let msg = "No search results found to summarize.".to_string();
            context.set(keys::SUMMARY, &msg).await;
            return Ok(TaskResult::new(Some(msg), NextAction::Continue));
        }

        let summary = self.chat.summarize(&query, &results, &tenant).await.map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!("summarize failed: {e}"))
        })?;

        info!(summary_len = summary.len(), "summary generated");
        context.set(keys::SUMMARY, &summary).await;

        Ok(TaskResult::new(Some(summary), NextAction::Continue))
    }
}

// ---------------------------------------------------------------------------
// Approval checkpoint
// ---------------------------------------------------------------------------

/// Pauses execution for human approval via the [`ApprovalPort`].
///
/// If the port returns a decision immediately (e.g. auto-approve), the graph
/// continues. Otherwise it emits `WaitForInput` so the caller can resume later.
pub struct ApprovalCheckpointTask {
    pub approval: Arc<dyn ApprovalPort>,
}

#[async_trait::async_trait]
impl Task for ApprovalCheckpointTask {
    fn id(&self) -> &str {
        APPROVAL_CHECKPOINT_TASK
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // On resume after WaitForInput, graph-flow re-runs this task.
        // Check if a decision was injected into context before asking the port.
        if let Some(already_decided) = context.get::<bool>(keys::CHECKPOINT_APPROVED).await {
            info!(approved = already_decided, "checkpoint resumed with prior decision");
            return Ok(TaskResult::new(
                Some(format!("approved={already_decided}")),
                NextAction::Continue,
            ));
        }

        let summary: Option<String> = context.get(keys::SUMMARY).await;

        let checkpoint =
            PendingCheckpoint { after_task: SUMMARIZE_TASK.to_string(), summary: summary.clone() };

        let decision = self.approval.request_approval(&checkpoint).await.map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!("approval failed: {e}"))
        })?;

        match decision {
            Some(d) => {
                info!(approved = d.approved, "checkpoint decided immediately");
                context.set(keys::CHECKPOINT_APPROVED, &d.approved).await;
                Ok(TaskResult::new(Some(format!("approved={}", d.approved)), NextAction::Continue))
            }
            None => {
                info!("checkpoint paused — awaiting human approval");
                Ok(TaskResult::new(Some("awaiting approval".to_string()), NextAction::WaitForInput))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Final answer
// ---------------------------------------------------------------------------

/// Assembles the final answer from the accumulated context.
pub struct FinalAnswerTask;

#[async_trait::async_trait]
impl Task for FinalAnswerTask {
    fn id(&self) -> &str {
        FINAL_ANSWER_TASK
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // If the checkpoint was rejected, produce a rejection answer.
        let approved: bool = context.get(keys::CHECKPOINT_APPROVED).await.unwrap_or(true);

        let answer = if approved {
            context
                .get::<String>(keys::SUMMARY)
                .await
                .unwrap_or_else(|| "No summary available.".to_string())
        } else {
            "Run was rejected at approval checkpoint.".to_string()
        };

        context.set(keys::FINAL_ANSWER, &answer).await;
        info!("final answer assembled");

        Ok(TaskResult::new(Some(answer), NextAction::End))
    }
}
