//! Domain value types for agent orchestration.
//!
//! These types are owned by agent-core and never leak graph-flow internals.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Query classification
// ---------------------------------------------------------------------------

/// Classification of a user query for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryType {
    Structured,
    Unstructured,
    Mixed,
}

// ---------------------------------------------------------------------------
// Search results (port output)
// ---------------------------------------------------------------------------

/// A single scored chunk returned from retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredChunk {
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Agent session and state
// ---------------------------------------------------------------------------

/// Unique identifier for an agent run.
pub type RunId = Uuid;

/// High-level lifecycle state of an agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Run is actively executing tasks.
    Running,
    /// Paused awaiting human approval at a checkpoint.
    AwaitingApproval,
    /// Run completed successfully.
    Completed,
    /// Run terminated with an error.
    Failed,
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

/// Information about a pending checkpoint requiring approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingCheckpoint {
    /// Which task produced the checkpoint.
    pub after_task: String,
    /// Summary text available for the approver.
    pub summary: Option<String>,
    /// Checkpoint identifier from the agent spec (if spec-driven).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// How long the caller should wait before applying `on_timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// What to do if the checkpoint times out: `"approve"` or `"reject"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_timeout: Option<String>,
}

/// Decision made on a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointDecision {
    pub approved: bool,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Run result
// ---------------------------------------------------------------------------

/// Outcome of an agent run (or a single step).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub run_id: RunId,
    pub state: AgentState,
    /// Final answer text, if the run completed.
    pub answer: Option<String>,
    /// Summary produced by the summarize step.
    pub summary: Option<String>,
    /// Search results that informed the answer.
    pub search_results: Option<Vec<ScoredChunk>>,
    /// Query classification result.
    pub query_type: Option<QueryType>,
    /// Pending checkpoint info when state is `AwaitingApproval`.
    pub pending_checkpoint: Option<PendingCheckpoint>,
    /// Timeline of executed steps.
    pub steps: Vec<StepRecord>,
}

/// Record of a single executed task within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub task_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub elapsed_ms: u64,
    pub status: StepStatus,
}

/// Outcome of a single step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Completed,
    Paused,
    Failed,
}

// ---------------------------------------------------------------------------
// Run configuration
// ---------------------------------------------------------------------------

/// Configuration for starting an agent run.
#[derive(Debug, Clone)]
pub struct AgentRunConfig {
    pub query: String,
    pub collection: String,
    pub tenant: String,
    /// Maximum number of graph steps before forced termination.
    pub max_steps: usize,
}
