//! Agent runtime abstraction and implementations.
//!
//! [`AgentRuntime`] defines the interface for starting and resuming agent runs.
//! The [`graph_flow`] submodule provides the primary implementation backed by
//! the `graph-flow` crate.

pub mod graph_flow;

use crate::types::{AgentRunConfig, AgentRunResult, CheckpointDecision, RunId};

/// Runtime capable of executing agent graphs.
///
/// Implementations may use different execution engines (graph-flow, in-house
/// DAG runner, Restate, etc.) — callers interact only through this trait.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Start a new agent run. Returns after the first pause point (checkpoint)
    /// or when the graph completes.
    async fn start(&self, config: AgentRunConfig) -> anyhow::Result<AgentRunResult>;

    /// Resume a paused run with a checkpoint decision.
    async fn resume(
        &self,
        run_id: RunId,
        decision: CheckpointDecision,
    ) -> anyhow::Result<AgentRunResult>;

    /// Inspect the current state of a run without advancing it.
    async fn inspect(&self, run_id: RunId) -> anyhow::Result<Option<AgentRunResult>>;
}
