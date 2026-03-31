//! Context key constants for the graph-flow execution context.
//!
//! Centralised here to prevent string drift across tasks.

pub const QUERY: &str = "query";
pub const COLLECTION: &str = "collection";
pub const TENANT: &str = "tenant";
pub const QUERY_TYPE: &str = "query_type";
pub const SEARCH_RESULTS: &str = "search_results";
pub const SEARCH_SUCCESS: &str = "search_success";
pub const SUMMARY: &str = "summary";
pub const FINAL_ANSWER: &str = "final_answer";
pub const CHECKPOINT_APPROVED: &str = "checkpoint_approved";
