//! Context key constants for the graph-flow execution context.
//!
//! Centralised here to prevent string drift across tasks.

pub const QUERY: &str = "query";
pub const COLLECTION: &str = "collection";
pub const TENANT: &str = "tenant";
pub const QUERY_TYPE: &str = "query_type";
pub const ROUTE_DECISION: &str = "route_decision";
pub const ROUTE_TO_AGENTIC_SEARCH: &str = "route_to_agentic_search";
pub const SEARCH_RESULTS: &str = "search_results";
pub const SUMMARY: &str = "summary";
pub const FINAL_ANSWER: &str = "final_answer";
pub const CHECKPOINT_APPROVED: &str = "checkpoint_approved";
pub const CHECKPOINT_REASON: &str = "checkpoint_reason";
pub const PENDING_CHECKPOINT: &str = "pending_checkpoint";
