//! Graph-flow agent orchestration for RAG pipelines.
//!
//! This crate defines the domain types, port traits, and runtime abstraction
//! for executing agent graphs. The primary runtime implementation uses
//! [`graph_flow`] as the DAG runner, but the [`AgentRuntime`] trait keeps the
//! orchestration engine swappable.
//!
//! ## Module layout
//!
//! - [`ports`] — Adapter traits (`RetrievalPort`, `ChatPort`, `ApprovalPort`)
//! - [`types`] — Domain value types (`AgentState`, query/result structs)
//! - [`classify`] — Deterministic query classification heuristics
//! - [`runtime`] — `AgentRuntime` trait and the graph-flow-backed implementation

pub mod classify;
pub mod ports;
pub mod runtime;
pub mod spec;
pub mod types;

pub use ports::{ApprovalPort, ChatPort, RetrievalPort};
pub use runtime::AgentRuntime;
pub use spec::{AgentReactActionType, AgentReactConfig, AgentReactStopConditions, AgentSpec};
pub use types::{AgentRunResult, AgentState, QueryType};
