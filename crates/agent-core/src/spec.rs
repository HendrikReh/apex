//! YAML-based agent specification loading.
//!
//! An [`AgentSpec`] defines graph topology, checkpoint behaviour, and metadata
//! for an agent. Specs are loaded from YAML files (typically under
//! `config/agents/`) and drive graph construction at runtime.
//!
//! The struct layout follows the projectAlpha spec format, scoped to what the
//! current runtime supports. Fields for retrieval profiles, guardrails, and
//! context templates are added later as those subsystems land.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Spec version
// ---------------------------------------------------------------------------

const SUPPORTED_VERSIONS: &[&str] = &["1.0"];

// ---------------------------------------------------------------------------
// Top-level spec
// ---------------------------------------------------------------------------

/// High-level agent specification loaded from YAML.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentSpec {
    /// Unique identifier for this agent (e.g. `"rag_spike"`).
    pub agent_id: String,
    /// Human-readable description.
    pub description: String,
    /// Spec format version (currently `"1.0"`).
    pub spec_version: String,
    /// Ordered list of task IDs this agent uses.
    pub tasks: Vec<String>,
    /// DAG topology.
    pub graph: AgentGraphSpec,
    /// Optional checkpoint configurations.
    #[serde(default)]
    pub checkpoints: Vec<AgentCheckpointConfig>,
    /// Optional data governance policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policies: Option<AgentPolicies>,
}

// ---------------------------------------------------------------------------
// Graph topology
// ---------------------------------------------------------------------------

/// Directed acyclic graph definition.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentGraphSpec {
    /// Task ID where execution begins.
    pub start_task: String,
    /// All task IDs in the graph.
    pub tasks: Vec<String>,
    /// Edges defining execution order.
    pub edges: Vec<AgentGraphEdge>,
}

/// A directed edge between two tasks.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentGraphEdge {
    pub from: String,
    pub to: String,
    /// If set, this edge is only followed when the named context key is truthy.
    #[serde(default)]
    pub condition_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Policies
// ---------------------------------------------------------------------------

/// Data governance constraints for this agent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentPolicies {
    /// Whether citations are required in the final answer.
    #[serde(default)]
    pub require_citations: bool,
    /// Whether policy context must be included in the prompt.
    #[serde(default)]
    pub require_policy_context: bool,
    /// Collections this agent is allowed to access.
    #[serde(default)]
    pub allowed_collections: Vec<String>,
}

// ---------------------------------------------------------------------------
// Checkpoint configuration
// ---------------------------------------------------------------------------

/// Configuration for a human-in-the-loop checkpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentCheckpointConfig {
    /// Unique ID for this checkpoint.
    pub checkpoint_id: String,
    /// The task after which the checkpoint fires.
    pub after_task: String,
    /// Type of approval required.
    #[serde(default)]
    pub approval_type: ApprovalType,
    /// Seconds to wait before `on_timeout` kicks in.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// What happens when the timeout expires.
    #[serde(default)]
    pub on_timeout: TimeoutAction,
}

/// Type of approval required at a checkpoint.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalType {
    /// Requires a human to approve.
    #[default]
    Human,
    /// Automatically approved (useful for testing or low-risk flows).
    Auto,
}

/// What happens when a checkpoint times out.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimeoutAction {
    /// Approve the checkpoint.
    Approve,
    /// Reject the checkpoint.
    #[default]
    Reject,
}

fn default_timeout_seconds() -> u64 {
    3600
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl AgentSpec {
    /// Parse a spec from a YAML string.
    pub fn from_yaml_str(yaml: &str) -> anyhow::Result<Self> {
        let spec: Self =
            serde_yaml_ng::from_str(yaml).context("failed to parse agent spec YAML")?;
        spec.validate()?;
        Ok(spec)
    }

    /// Load a spec from a YAML file.
    pub async fn from_yaml_file(path: &Path) -> anyhow::Result<Self> {
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read agent spec at {}", path.display()))?;
        Self::from_yaml_str(&content)
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

impl AgentSpec {
    /// Validate internal consistency of the spec.
    fn validate(&self) -> anyhow::Result<()> {
        self.validate_version()?;
        self.validate_graph_references()?;
        self.validate_acyclicity()?;
        self.validate_checkpoint_references()?;
        Ok(())
    }

    fn validate_version(&self) -> anyhow::Result<()> {
        if !SUPPORTED_VERSIONS.contains(&self.spec_version.as_str()) {
            return Err(anyhow!(
                "unsupported spec_version '{}' (supported: {})",
                self.spec_version,
                SUPPORTED_VERSIONS.join(", ")
            ));
        }
        Ok(())
    }

    fn validate_graph_references(&self) -> anyhow::Result<()> {
        let task_set: std::collections::HashSet<&str> =
            self.graph.tasks.iter().map(|t| t.as_str()).collect();

        if !task_set.contains(self.graph.start_task.as_str()) {
            return Err(anyhow!("start_task '{}' not found in graph.tasks", self.graph.start_task));
        }

        let mut missing = Vec::new();
        for edge in &self.graph.edges {
            if !task_set.contains(edge.from.as_str()) {
                missing.push(edge.from.clone());
            }
            if !task_set.contains(edge.to.as_str()) {
                missing.push(edge.to.clone());
            }
        }

        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            return Err(anyhow!("graph edges reference unknown tasks: {}", missing.join(", ")));
        }

        // Every task in the top-level list must appear in graph.tasks.
        for task in &self.tasks {
            if !task_set.contains(task.as_str()) {
                return Err(anyhow!("top-level task '{}' not found in graph.tasks", task));
            }
        }

        Ok(())
    }

    /// Verify the edge set forms a DAG (no cycles).
    fn validate_acyclicity(&self) -> anyhow::Result<()> {
        // Build adjacency list.
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for task in &self.graph.tasks {
            adj.entry(task.as_str()).or_default();
        }
        for edge in &self.graph.edges {
            adj.entry(edge.from.as_str()).or_default().push(edge.to.as_str());
        }

        // Three-colour DFS: white = unvisited, gray = in-progress, black = done.
        let mut white: HashSet<&str> = adj.keys().copied().collect();
        let mut gray: HashSet<&str> = HashSet::new();

        fn dfs<'a>(
            node: &'a str,
            adj: &HashMap<&'a str, Vec<&'a str>>,
            white: &mut HashSet<&'a str>,
            gray: &mut HashSet<&'a str>,
        ) -> Option<String> {
            white.remove(node);
            gray.insert(node);

            if let Some(neighbors) = adj.get(node) {
                for &next in neighbors {
                    if gray.contains(next) {
                        return Some(format!("{node} -> {next}"));
                    }
                    if white.contains(next)
                        && let Some(cycle) = dfs(next, adj, white, gray)
                    {
                        return Some(cycle);
                    }
                }
            }

            gray.remove(node);
            None
        }

        while let Some(&node) = white.iter().next() {
            if let Some(cycle_edge) = dfs(node, &adj, &mut white, &mut gray) {
                return Err(anyhow!("graph contains a cycle (at edge: {cycle_edge})"));
            }
        }

        Ok(())
    }

    fn validate_checkpoint_references(&self) -> anyhow::Result<()> {
        let task_set: std::collections::HashSet<&str> =
            self.graph.tasks.iter().map(|t| t.as_str()).collect();

        for cp in &self.checkpoints {
            if !task_set.contains(cp.after_task.as_str()) {
                return Err(anyhow!(
                    "checkpoint '{}' references unknown task '{}'",
                    cp.checkpoint_id,
                    cp.after_task
                ));
            }

            // Verify that after_task actually has an edge to an
            // approval_checkpoint task, otherwise the checkpoint config
            // will never be matched at runtime.
            let reaches_checkpoint = self
                .graph
                .edges
                .iter()
                .any(|e| e.from == cp.after_task && e.to == "approval_checkpoint");
            if !reaches_checkpoint {
                return Err(anyhow!(
                    "checkpoint '{}': after_task '{}' has no edge to 'approval_checkpoint'",
                    cp.checkpoint_id,
                    cp.after_task
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SPEC: &str = r#"
agent_id: rag_spike
description: "Vertical-slice RAG agent: classify -> search -> summarize -> checkpoint -> answer."
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
    timeout_seconds: 1800
    on_timeout: reject
"#;

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn parse_valid_spec() {
        let spec = AgentSpec::from_yaml_str(VALID_SPEC).expect("should parse");
        assert_eq!(spec.agent_id, "rag_spike");
        assert_eq!(spec.tasks.len(), 5);
        assert_eq!(spec.graph.start_task, "classify");
        assert_eq!(spec.graph.edges.len(), 4);
        assert_eq!(spec.checkpoints.len(), 1);
        assert_eq!(spec.checkpoints[0].checkpoint_id, "post_summary");
        assert_eq!(spec.checkpoints[0].approval_type, ApprovalType::Human);
        assert_eq!(spec.checkpoints[0].timeout_seconds, 1800);
        assert_eq!(spec.checkpoints[0].on_timeout, TimeoutAction::Reject);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn defaults_for_checkpoint() {
        let yaml = r#"
agent_id: minimal
description: minimal checkpoint test
spec_version: "1.0"
tasks:
  - a
  - approval_checkpoint
graph:
  start_task: a
  tasks: [a, approval_checkpoint]
  edges:
    - { from: a, to: approval_checkpoint }
checkpoints:
  - checkpoint_id: cp1
    after_task: a
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        let cp = &spec.checkpoints[0];
        assert_eq!(cp.approval_type, ApprovalType::Human);
        assert_eq!(cp.timeout_seconds, 3600);
        assert_eq!(cp.on_timeout, TimeoutAction::Reject);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn no_checkpoints_is_valid() {
        let yaml = r#"
agent_id: simple
description: no checkpoints
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        assert!(spec.checkpoints.is_empty());
    }

    #[test]
    fn rejects_unsupported_version() {
        let yaml = r#"
agent_id: bad
description: bad version
spec_version: "99.0"
tasks: [a]
graph:
  start_task: a
  tasks: [a]
  edges: []
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("unsupported spec_version"));
    }

    #[test]
    fn rejects_unknown_start_task() {
        let yaml = r#"
agent_id: bad
description: missing start
spec_version: "1.0"
tasks: [a]
graph:
  start_task: nonexistent
  tasks: [a]
  edges: []
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("start_task 'nonexistent' not found"));
    }

    #[test]
    fn rejects_edge_to_unknown_task() {
        let yaml = r#"
agent_id: bad
description: bad edge
spec_version: "1.0"
tasks: [a]
graph:
  start_task: a
  tasks: [a]
  edges:
    - { from: a, to: ghost }
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn rejects_checkpoint_to_unknown_task() {
        let yaml = r#"
agent_id: bad
description: bad checkpoint
spec_version: "1.0"
tasks: [a]
graph:
  start_task: a
  tasks: [a]
  edges: []
checkpoints:
  - checkpoint_id: cp1
    after_task: nonexistent
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("checkpoint 'cp1' references unknown task"));
    }

    #[test]
    fn rejects_unreachable_checkpoint() {
        let yaml = r#"
agent_id: bad
description: checkpoint after_task has no edge to approval_checkpoint
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
checkpoints:
  - checkpoint_id: cp1
    after_task: a
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(
            err.to_string().contains("no edge to 'approval_checkpoint'"),
            "expected unreachable checkpoint error, got: {err}"
        );
    }

    #[test]
    fn rejects_top_level_task_not_in_graph() {
        let yaml = r#"
agent_id: bad
description: orphan task
spec_version: "1.0"
tasks: [a, orphan]
graph:
  start_task: a
  tasks: [a]
  edges: []
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("orphan"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn conditional_edge() {
        let yaml = r#"
agent_id: cond
description: conditional edge
spec_version: "1.0"
tasks: [a, b, c]
graph:
  start_task: a
  tasks: [a, b, c]
  edges:
    - { from: a, to: b }
    - { from: a, to: c, condition_key: skip_b }
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        assert_eq!(spec.graph.edges[1].condition_key.as_deref(), Some("skip_b"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn parses_policies_section() {
        let yaml = r#"
agent_id: policy_test
description: policies section test
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
policies:
  require_citations: true
  require_policy_context: false
  allowed_collections:
    - docs
    - notes
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        let pol = spec.policies.expect("policies should be present");
        assert!(pol.require_citations);
        assert!(!pol.require_policy_context);
        assert_eq!(pol.allowed_collections, vec!["docs", "notes"]);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn policies_is_optional() {
        let yaml = r#"
agent_id: no_policies
description: no policies
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        assert!(spec.policies.is_none());
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn policies_empty_collections() {
        let yaml = r#"
agent_id: empty_cols
description: empty allowed_collections
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
policies:
  require_citations: false
  require_policy_context: false
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        let pol = spec.policies.expect("policies");
        assert!(pol.allowed_collections.is_empty());
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn policies_partial_block_defaults_booleans() {
        let yaml = r#"
agent_id: partial_pol
description: partial policies block
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
policies:
  allowed_collections:
    - docs
"#;
        let spec = AgentSpec::from_yaml_str(yaml).expect("should parse");
        let pol = spec.policies.expect("policies should be present");
        assert!(!pol.require_citations, "should default to false");
        assert!(!pol.require_policy_context, "should default to false");
        assert_eq!(pol.allowed_collections, vec!["docs"]);
    }

    #[test]
    fn rejects_direct_cycle() {
        let yaml = r#"
agent_id: bad
description: direct cycle
spec_version: "1.0"
tasks: [a, b]
graph:
  start_task: a
  tasks: [a, b]
  edges:
    - { from: a, to: b }
    - { from: b, to: a }
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("cycle"), "expected cycle error, got: {err}");
    }

    #[test]
    fn rejects_indirect_cycle() {
        let yaml = r#"
agent_id: bad
description: indirect cycle
spec_version: "1.0"
tasks: [a, b, c]
graph:
  start_task: a
  tasks: [a, b, c]
  edges:
    - { from: a, to: b }
    - { from: b, to: c }
    - { from: c, to: a }
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("cycle"), "expected cycle error, got: {err}");
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let yaml = r#"
agent_id: bad
description: self loop
spec_version: "1.0"
tasks: [a]
graph:
  start_task: a
  tasks: [a]
  edges:
    - { from: a, to: a }
"#;
        let err = AgentSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("cycle"), "expected cycle error, got: {err}");
    }
}
