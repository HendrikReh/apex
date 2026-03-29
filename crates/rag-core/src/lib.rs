//! Core library for the Apex RAG pipeline.
//!
//! Provides extraction, ingestion, embedding, retrieval, context assembly, and
//! storage abstractions with tenant-scoped operations.

pub mod bm25;
pub mod chat;
pub mod config;
pub mod context;
pub mod embed;
pub mod extract;
pub mod fusion;
pub mod ingest;
pub mod llm;
pub mod prompt;
pub mod retrieval;
pub mod sidecar;
pub mod stores;
pub mod tenant;

pub use bm25::{Bm25Config, Bm25Embedder, SparseVector};
pub use chat::{ChatDefaults, ChatRequest, ChatResponse, ChatService};
pub use config::AppConfig;
pub use context::{
    Citation, ContextBuilder, ContextChunk, ContextConfig, ContextResult, ContextStats,
    DedupeStrategy,
};
pub use embed::{AnyEmbedder, EmbedService, MockEmbedder, OpenAiEmbedder};
pub use extract::{
    ExtractionOptions, ExtractionResult, ExtractorRegistry, FileType, FormatExtractor,
    MarkdownExtractor, OcrOptions, PdfExtractor, TextExtractor,
};
pub use fusion::{FusedChunk, RetrievedChunk, rrf_fusion};
pub use ingest::IngestService;
pub use llm::{ChatBackend, ChatMessage, ChatRole, CompletionRequest, LlmResponse, TokenUsage};
pub use prompt::{PromptContext, PromptRenderer, render_context_chunks};
pub use retrieval::{HybridOverrides, RetrievalDefaults, RetrievalService};
pub use sidecar::Sidecar;
pub use stores::Stores;
pub use stores::conversations::{ConversationRow, MessageRole, MessageRow};
pub use stores::corpus_stats::CorpusStats;
pub use tenant::TenantId;
