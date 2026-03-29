//! HTTP client for the Apex RAG server API.
//!
//! Provides `TenantApiClient` for tenant-scoped API operations.

mod error;
mod tenant_id;
pub mod types;

pub use error::ClientError;
pub use tenant_id::TenantId;
