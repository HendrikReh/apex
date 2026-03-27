//! Core library for the Apex RAG pipeline.
//!
//! Provides extraction, ingestion, embedding, retrieval, context assembly, and
//! storage abstractions with tenant-scoped operations.

pub mod bm25;
pub mod config;
pub mod embed;
pub mod extract;
pub mod sidecar;
pub mod stores;
pub mod tenant;

pub use bm25::{Bm25Config, Bm25Embedder, SparseVector};
pub use config::AppConfig;
pub use embed::{AnyEmbedder, EmbedService, MockEmbedder, OpenAiEmbedder};
pub use extract::{
    ExtractionResult, ExtractorRegistry, FileType, FormatExtractor, MarkdownExtractor,
    PdfExtractor, TextExtractor,
};
pub use sidecar::Sidecar;
pub use stores::Stores;
pub use tenant::TenantId;
