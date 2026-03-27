//! Core library for the Apex RAG pipeline.
//!
//! Provides extraction, ingestion, embedding, retrieval, context assembly, and
//! storage abstractions with tenant-scoped operations.

pub mod config;
pub mod stores;
pub mod tenant;

pub use config::AppConfig;
pub use stores::Stores;
pub use tenant::TenantId;
