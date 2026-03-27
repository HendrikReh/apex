//! End-to-end synchronous document ingestion pipeline.
//!
//! `IngestService` orchestrates extraction, chunking, embedding, and storage
//! for individual files and directory batches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use walkdir::WalkDir;

use crate::bm25::Bm25Embedder;
use crate::config::AppConfig;
use crate::embed::AnyEmbedder;
use crate::extract::{ExtractorRegistry, FileType};
use crate::stores::Stores;
use crate::tenant::TenantId;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// A request to ingest a single file.
#[derive(Debug)]
pub struct IngestFileRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

/// Outcome of ingesting a single document.
#[derive(Debug)]
pub struct IngestOutcome {
    pub document_id: String,
    pub collection: String,
    pub chunks_created: usize,
    pub skipped: bool,
}

/// A request to ingest every supported file in a directory tree.
#[derive(Debug)]
pub struct IngestDirectoryRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

/// Summary of a batch (directory) ingest run.
#[derive(Debug)]
pub struct IngestBatchOutcome {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    pub failures: Vec<DocumentFailure>,
}

/// Record of a single document that failed during batch ingest.
#[derive(Debug)]
pub struct DocumentFailure {
    pub path: PathBuf,
    pub error: String,
}

/// A source document paired with its optional sidecar metadata file.
#[derive(Debug)]
pub struct DocumentPair {
    pub path: PathBuf,
    pub sidecar_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// IngestService
// ---------------------------------------------------------------------------

/// Orchestrates the full ingest pipeline: extract -> chunk -> embed -> store.
pub struct IngestService {
    stores: Stores,
    extractors: ExtractorRegistry,
    embedder: AnyEmbedder,
    bm25: Bm25Embedder,
    default_collection: String,
    chunking_max_tokens: usize,
    chunking_overlap_ratio: f32,
    known_collections: Mutex<HashSet<String>>,
}

impl IngestService {
    /// Build a new `IngestService` from shared stores and application config.
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let extractors =
            ExtractorRegistry::with_defaults().context("building extractor registry")?;
        let embedder = AnyEmbedder::from_config(config).context("building embedder")?;
        let bm25 = Bm25Embedder::from_app_config(config).context("building BM25 embedder")?;

        Ok(Self {
            default_collection: stores.default_collection.clone(),
            stores,
            extractors,
            embedder,
            bm25,
            chunking_max_tokens: config.chunking_max_tokens,
            chunking_overlap_ratio: config.chunking_overlap_ratio,
            known_collections: Mutex::new(HashSet::new()),
        })
    }
}

// ---------------------------------------------------------------------------
// Directory discovery
// ---------------------------------------------------------------------------

/// Walk `dir` recursively, returning every supported document paired with its
/// optional `.metadata.json` sidecar.
///
/// Hidden files (starting with `.`), unsupported extensions, and standalone
/// sidecar files are excluded from the result set.
pub fn discover_document_pairs(dir: &Path) -> Result<Vec<DocumentPair>> {
    let mut pairs = Vec::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            // Prune hidden directories (but not the root entry itself).
            e.depth() == 0
                || e.file_name()
                    .to_str()
                    .map(|s| !s.starts_with('.'))
                    .unwrap_or(false)
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Skip sidecar files -- they are not source documents.
        if file_name.ends_with(".metadata.json") {
            continue;
        }

        // Skip unsupported extensions.
        let extension = match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => ext,
            None => continue,
        };
        if FileType::from_extension(extension).is_none() {
            continue;
        }

        // Check for a companion sidecar.
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let sidecar_name = format!("{stem}.metadata.json");
        let sidecar_path = path.with_file_name(&sidecar_name);
        let sidecar_path = if sidecar_path.exists() { Some(sidecar_path) } else { None };

        pairs.push(DocumentPair { path: path.to_owned(), sidecar_path });
    }

    Ok(pairs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn discovers_document_sidecar_pairs() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("doc.txt"), "hello").expect("write");
        fs::write(dir.path().join("doc.metadata.json"), "{}").expect("write");
        fs::write(dir.path().join("readme.md"), "# Hi").expect("write");
        // hidden file should be skipped
        fs::write(dir.path().join(".hidden.txt"), "secret").expect("write");
        // unsupported extension should be skipped
        fs::write(dir.path().join("image.png"), "bytes").expect("write");
        // sidecar without a document should be skipped
        fs::write(dir.path().join("orphan.metadata.json"), "{}").expect("write");

        let pairs = discover_document_pairs(dir.path()).expect("discovery");

        assert_eq!(pairs.len(), 2);

        let txt_pair = pairs.iter().find(|p| p.path.ends_with("doc.txt")).expect("txt pair");
        assert!(txt_pair.sidecar_path.is_some());

        let md_pair = pairs.iter().find(|p| p.path.ends_with("readme.md")).expect("md pair");
        assert!(md_pair.sidecar_path.is_none());
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn skips_hidden_files_and_unsupported_extensions() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join(".hidden.txt"), "h").expect("write");
        fs::write(dir.path().join("photo.png"), "p").expect("write");
        fs::write(dir.path().join("data.csv"), "d").expect("write");

        let pairs = discover_document_pairs(dir.path()).expect("discovery");
        assert!(pairs.is_empty());
    }
}
