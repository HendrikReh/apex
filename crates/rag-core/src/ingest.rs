//! End-to-end synchronous document ingestion pipeline.
//!
//! `IngestService` orchestrates extraction, chunking, embedding, and storage
//! for individual files and directory batches.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::stores::vectors::{DENSE_VECTOR_NAME, SPARSE_VECTOR_NAME};
use anyhow::{Context, Result, bail};
use qdrant_client::qdrant::SparseVector as QdrantSparseVector;
use qdrant_client::qdrant::{
    DenseVector, Distance, NamedVectors, PointStruct, Vector as QdrantVector, Vectors,
};
use tokio::sync::Mutex;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::bm25::{Bm25Embedder, SparseVector};
use crate::config::AppConfig;
use crate::embed::{AnyEmbedder, EmbedService};
use crate::extract::{self, ExtractorRegistry, FileType};
use crate::sidecar::Sidecar;
use crate::stores::Stores;
use crate::tenant::TenantId;

use rag_chunking::{ChunkWithSection, chunk_text_with_strategy_sectioned, select_strategy};

/// Maximum character window used by rag-chunking's semantic strategy fallback.
///
/// This only applies when semantic chunking is explicitly selected; the Phase 4
/// ingest path otherwise uses token or markdown chunking.
const DEFAULT_SEMANTIC_MAX_CHARS: usize = 20_000;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// A request to ingest a single file.
#[derive(Debug)]
pub struct IngestFileRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
    pub dry_run: bool,
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
    pub dry_run: bool,
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
            ExtractorRegistry::with_defaults(config).context("building extractor registry")?;
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

    // -----------------------------------------------------------------------
    // Public orchestration
    // -----------------------------------------------------------------------

    /// Ingest a single file: extract, chunk, embed, and persist.
    ///
    /// Returns [`IngestOutcome`] describing the result, including whether
    /// the document was skipped due to an unchanged checksum.
    #[allow(clippy::disallowed_methods)] // serde_json::to_value internally uses .expect()
    pub async fn ingest_file(&self, req: IngestFileRequest) -> Result<IngestOutcome> {
        let sidecar = self.load_sidecar(&req.path).await?;
        let prepared = self.extract_and_checksum(&req, &sidecar).await?;

        // Check for unchanged content.
        let existing_checksum = self
            .stores
            .get_document_checksum(req.tenant.as_str(), &prepared.document_id)
            .await
            .context("checking existing document checksum")?;

        // Dry-run: report what would happen without writing anything.
        if req.dry_run {
            let skipped = existing_checksum.as_deref() == Some(&prepared.checksum);
            return Ok(IngestOutcome {
                document_id: prepared.document_id,
                collection: prepared.collection,
                chunks_created: 0,
                skipped,
            });
        }

        if existing_checksum.as_deref() == Some(&prepared.checksum) {
            // Checksum match — update metadata if sidecar changed, skip re-chunking.
            let metadata_json = prepared
                .sidecar
                .as_ref()
                .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
            self.stores
                .upsert_document(
                    req.tenant.as_str(),
                    &prepared.document_id,
                    prepared.sidecar.as_ref().map_or("", |s| &s.document.title),
                    prepared.sidecar.as_ref().map(|s| s.language.as_str()),
                    metadata_json.as_ref(),
                    Some(&prepared.source_path),
                    prepared.sidecar.as_ref().and_then(|s| s.document.version.as_deref()),
                    Some(&prepared.checksum),
                    None,
                    None, // don't update token_count on skip
                    Some(&prepared.collection),
                )
                .await
                .context("updating metadata for unchanged document")?;

            return Ok(IngestOutcome {
                document_id: prepared.document_id,
                collection: prepared.collection,
                chunks_created: 0,
                skipped: true,
            });
        }

        let chunked = self.chunk_text(prepared).await?;
        let num_chunks = chunked.chunks.len();
        let collection = chunked.collection.clone();
        let document_id = chunked.document_id.clone();

        let embedded = self.embed_chunks(chunked).await?;
        self.persist(&req.tenant, &embedded).await?;

        // Update corpus stats (non-fatal).
        if let Err(e) = self
            .stores
            .update_corpus_stats(
                req.tenant.as_str(),
                &collection,
                &document_id,
                embedded.total_tokens,
            )
            .await
        {
            tracing::warn!(
                document_id = %document_id,
                error = %e,
                "failed to update corpus stats (non-fatal)"
            );
        }

        Ok(IngestOutcome { document_id, collection, chunks_created: num_chunks, skipped: false })
    }

    /// Ingest every supported file in a directory tree.
    pub async fn ingest_directory(
        &self,
        req: IngestDirectoryRequest,
    ) -> Result<IngestBatchOutcome> {
        let pairs = discover_document_pairs(&req.path)
            .with_context(|| format!("discovering documents in {}", req.path.display()))?;

        let mut outcome =
            IngestBatchOutcome { documents: 0, chunks: 0, skipped: 0, failures: Vec::new() };

        for pair in pairs {
            let file_req = IngestFileRequest {
                path: pair.path.clone(),
                tenant: req.tenant.clone(),
                collection_override: req.collection_override.clone(),
                dry_run: req.dry_run,
            };

            match self.ingest_file(file_req).await {
                Ok(result) => {
                    outcome.documents += 1;
                    outcome.chunks += result.chunks_created;
                    if result.skipped {
                        outcome.skipped += 1;
                    }
                }
                Err(e) => {
                    outcome
                        .failures
                        .push(DocumentFailure { path: pair.path, error: format!("{e:#}") });
                }
            }
        }

        Ok(outcome)
    }

    // -----------------------------------------------------------------------
    // Step functions
    // -----------------------------------------------------------------------

    /// Step 1: Read the file, extract text, compute a content checksum.
    async fn extract_and_checksum(
        &self,
        req: &IngestFileRequest,
        sidecar: &Option<Sidecar>,
    ) -> Result<PreparedDocument> {
        let extension = req
            .path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow::anyhow!("file has no extension: {}", req.path.display()))?;

        let file_type = FileType::from_extension(extension)
            .ok_or_else(|| anyhow::anyhow!("unsupported file extension: {extension}"))?;

        let content = tokio::fs::read(&req.path)
            .await
            .with_context(|| format!("reading file {}", req.path.display()))?;

        let extraction_options = extract::ExtractionOptions {
            ocr: sidecar
                .as_ref()
                .and_then(|s| s.ingestion.as_ref())
                .and_then(|i| i.ocr.as_ref())
                .map(|ocr| extract::OcrOptions {
                    force: ocr.force,
                    language_hints: ocr.language_hints.clone(),
                    timeout_secs: ocr.timeout_secs,
                }),
        };

        let result = self
            .extractors
            .extract(file_type, &content, &extraction_options)
            .await
            .with_context(|| format!("extracting text from {}", req.path.display()))?;

        let checksum = extract::checksum(&result.text);

        // Document ID: sidecar.document.id > filename stem.
        let document_id =
            sidecar.as_ref().and_then(|s| s.document.id.clone()).unwrap_or_else(|| {
                req.path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string()
            });

        // Collection: request override > sidecar ingestion collection > default.
        let collection = req
            .collection_override
            .clone()
            .or_else(|| {
                sidecar
                    .as_ref()
                    .and_then(|s| s.ingestion.as_ref())
                    .and_then(|i| i.collection.clone())
            })
            .unwrap_or_else(|| self.default_collection.clone());

        let source_path = req.path.display().to_string();

        Ok(PreparedDocument {
            document_id,
            collection,
            sidecar: sidecar.clone(),
            text: result.text,
            checksum,
            source_path,
            native_metadata: result.native_metadata,
        })
    }

    /// Step 2: Chunk the extracted text with the resolved strategy.
    async fn chunk_text(&self, prepared: PreparedDocument) -> Result<ChunkedDocument> {
        // Resolve chunking overrides from sidecar.
        let chunking_override = prepared
            .sidecar
            .as_ref()
            .and_then(|s| s.ingestion.as_ref())
            .and_then(|i| i.chunking.as_ref());

        let strategy_str = chunking_override.and_then(|c| c.strategy.as_deref());
        let strategy = select_strategy(strategy_str, &prepared.text, None);

        let max_tokens =
            chunking_override.and_then(|c| c.max_tokens).unwrap_or(self.chunking_max_tokens);

        let overlap_ratio =
            chunking_override.and_then(|c| c.overlap_ratio).unwrap_or(self.chunking_overlap_ratio);

        let chunks = chunk_text_with_strategy_sectioned(
            &prepared.text,
            strategy,
            max_tokens,
            overlap_ratio,
            DEFAULT_SEMANTIC_MAX_CHARS,
        )
        .await;

        if chunks.is_empty() {
            bail!("chunking produced zero chunks for document '{}'", prepared.document_id);
        }

        Ok(ChunkedDocument {
            document_id: prepared.document_id,
            collection: prepared.collection,
            sidecar: prepared.sidecar,
            checksum: prepared.checksum,
            source_path: prepared.source_path,
            chunks,
            actual_max_tokens: max_tokens,
            native_metadata: prepared.native_metadata,
        })
    }

    /// Step 3: Generate dense and sparse embeddings for each chunk.
    async fn embed_chunks(&self, chunked: ChunkedDocument) -> Result<EmbeddedDocument> {
        let chunk_texts: Vec<String> = chunked.chunks.iter().map(|c| c.text.clone()).collect();

        let dense_vectors =
            self.embedder.embed_batch(&chunk_texts).await.context("generating dense embeddings")?;

        let sparse_vectors: Vec<SparseVector> =
            chunk_texts.iter().map(|text| self.bm25.embed_document(text)).collect();

        let total_tokens =
            chunk_texts.len() as i64 * i64::try_from(chunked.actual_max_tokens).unwrap_or(600);

        Ok(EmbeddedDocument {
            document_id: chunked.document_id,
            collection: chunked.collection,
            sidecar: chunked.sidecar,
            checksum: chunked.checksum,
            source_path: chunked.source_path,
            chunks: chunked.chunks,
            dense_vectors,
            sparse_vectors,
            total_tokens,
            native_metadata: chunked.native_metadata,
        })
    }

    /// Step 4: Persist the document, chunks, and vectors to Postgres + Qdrant.
    #[allow(clippy::disallowed_methods)] // serde_json::to_value internally uses .expect()
    async fn persist(&self, tenant: &TenantId, doc: &EmbeddedDocument) -> Result<()> {
        let tenant_str = tenant.as_str();
        let vector_size = u64::try_from(self.embedder.dim()).context("embedder dim exceeds u64")?;

        // Ensure the Qdrant collection exists (cached).
        self.ensure_collection_cached(&doc.collection, vector_size).await?;

        // Upsert document row in Postgres.
        let metadata_json = doc
            .sidecar
            .as_ref()
            .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
        self.stores
            .upsert_document(
                tenant_str,
                &doc.document_id,
                doc.sidecar.as_ref().map_or("", |s| &s.document.title),
                doc.sidecar.as_ref().map(|s| s.language.as_str()),
                metadata_json.as_ref(),
                Some(&doc.source_path),
                doc.sidecar.as_ref().and_then(|s| s.document.version.as_deref()),
                Some(&doc.checksum),
                None,
                Some(doc.total_tokens),
                Some(&doc.collection),
            )
            .await
            .context("upserting document row")?;

        // Upsert chunks in Postgres.
        let chunk_texts: Vec<String> = doc.chunks.iter().map(|c| c.text.clone()).collect();
        self.stores
            .insert_chunks(tenant_str, &doc.document_id, &chunk_texts)
            .await
            .context("upserting chunks")?;

        let points = build_qdrant_points(tenant_str, doc)?;

        // Upsert vectors to Qdrant.
        self.stores
            .upsert_points(&doc.collection, points)
            .await
            .context("upserting vectors to Qdrant")?;

        // Delete stale Qdrant points (best-effort).
        let new_count = doc.chunks.len();
        if let Err(e) =
            self.delete_stale_points(tenant_str, &doc.document_id, &doc.collection, new_count).await
        {
            tracing::warn!(
                document_id = %doc.document_id,
                error = %e,
                "failed to clean up stale Qdrant points (non-fatal)"
            );
        }

        Ok(())
    }

    /// Ensure a Qdrant collection exists, caching the result.
    async fn ensure_collection_cached(&self, collection: &str, vector_size: u64) -> Result<()> {
        // Fast path: already known.
        {
            let cache = self.known_collections.lock().await;
            if cache.contains(collection) {
                return Ok(());
            }
        }

        // Slow path: create/validate the collection.
        match self.stores.ensure_collection(collection, vector_size, Distance::Cosine).await {
            Ok(()) => {
                let mut cache = self.known_collections.lock().await;
                cache.insert(collection.to_string());
                Ok(())
            }
            Err(e) => {
                // Evict from cache on failure.
                let mut cache = self.known_collections.lock().await;
                cache.remove(collection);
                Err(e).with_context(|| format!("ensuring Qdrant collection '{collection}'"))
            }
        }
    }

    /// Delete stale Qdrant points for chunk indices >= new_count.
    ///
    /// For MVP, this is a no-op. Stable UUIDs handle overwrites; stale
    /// high-index points are accepted until full re-index.
    async fn delete_stale_points(
        &self,
        _tenant: &str,
        _document_id: &str,
        _collection: &str,
        _new_count: usize,
    ) -> Result<()> {
        // TODO: implement stale point cleanup when needed.
        Ok(())
    }

    /// Load the optional sidecar metadata file for a document.
    async fn load_sidecar(&self, doc_path: &Path) -> Result<Option<Sidecar>> {
        let stem = doc_path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            anyhow::anyhow!("cannot determine file stem for {}", doc_path.display())
        })?;
        let sidecar_name = format!("{stem}.metadata.json");
        let sidecar_path = doc_path.with_file_name(&sidecar_name);

        if !sidecar_path.exists() {
            return Ok(None);
        }

        let bytes = tokio::fs::read(&sidecar_path)
            .await
            .with_context(|| format!("reading sidecar {}", sidecar_path.display()))?;

        let sidecar = Sidecar::from_json(&bytes)
            .with_context(|| format!("parsing sidecar {}", sidecar_path.display()))?;

        Ok(Some(sidecar))
    }
}

// ---------------------------------------------------------------------------
// Intermediate structs
// ---------------------------------------------------------------------------

/// Intermediate: text extracted and checksum computed.
struct PreparedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    text: String,
    checksum: String,
    source_path: String,
    native_metadata: Option<HashMap<String, String>>,
}

/// Intermediate: text chunked.
struct ChunkedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    checksum: String,
    source_path: String,
    chunks: Vec<ChunkWithSection>,
    actual_max_tokens: usize,
    native_metadata: Option<HashMap<String, String>>,
}

/// Intermediate: chunks embedded (dense + sparse).
struct EmbeddedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    checksum: String,
    source_path: String,
    chunks: Vec<ChunkWithSection>,
    dense_vectors: Vec<Vec<f32>>,
    sparse_vectors: Vec<SparseVector>,
    total_tokens: i64,
    native_metadata: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a deterministic UUID v5 for a chunk, ensuring stable point IDs
/// across re-ingestions of the same document.
fn stable_chunk_uuid(tenant: &str, document_id: &str, chunk_index: usize) -> Uuid {
    let input = format!("{tenant}:{document_id}:{chunk_index}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, input.as_bytes())
}

fn build_qdrant_points(tenant: &str, doc: &EmbeddedDocument) -> Result<Vec<PointStruct>> {
    if doc.dense_vectors.len() != doc.chunks.len() {
        bail!(
            "embedder returned {} dense vectors for {} chunks",
            doc.dense_vectors.len(),
            doc.chunks.len()
        );
    }
    if doc.sparse_vectors.len() != doc.chunks.len() {
        bail!(
            "BM25 embedder returned {} sparse vectors for {} chunks",
            doc.sparse_vectors.len(),
            doc.chunks.len()
        );
    }

    Ok(doc
        .chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let point_id = stable_chunk_uuid(tenant, &doc.document_id, i);

            let payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
                ("tenant".to_string(), tenant.to_string().into()),
                ("document_id".to_string(), doc.document_id.clone().into()),
                ("chunk_index".to_string(), (i as i64).into()),
                ("text".to_string(), chunk.text.clone().into()),
            ]
            .into();

            // Build named vectors with both dense and sparse.
            let dense = QdrantVector::from(DenseVector { data: doc.dense_vectors[i].clone() });
            let sparse = QdrantVector::from(QdrantSparseVector {
                indices: doc.sparse_vectors[i].indices.clone(),
                values: doc.sparse_vectors[i].values.clone(),
            });

            let mut named = std::collections::HashMap::new();
            named.insert(DENSE_VECTOR_NAME.to_string(), dense);
            named.insert(SPARSE_VECTOR_NAME.to_string(), sparse);

            PointStruct {
                id: Some(point_id.to_string().into()),
                payload,
                vectors: Some(Vectors::from(NamedVectors { vectors: named })),
            }
        })
        .collect())
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
            e.depth() == 0 || e.file_name().to_str().map(|s| !s.starts_with('.')).unwrap_or(false)
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
        pairs.push(DocumentPair { path: path.to_owned() });
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

        assert!(pairs.iter().any(|p| p.path.ends_with("doc.txt")));
        assert!(pairs.iter().any(|p| p.path.ends_with("readme.md")));
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

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn stable_chunk_uuid_is_deterministic() {
        let id_a = stable_chunk_uuid("default", "doc-1", 0);
        let id_b = stable_chunk_uuid("default", "doc-1", 0);
        let id_c = stable_chunk_uuid("default", "doc-1", 1);
        assert_eq!(id_a, id_b, "same inputs should produce same UUID");
        assert_ne!(id_a, id_c, "different chunk index should differ");
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn build_qdrant_points_rejects_dense_vector_length_mismatch() {
        let doc = EmbeddedDocument {
            document_id: "doc-1".to_string(),
            collection: "test".to_string(),
            sidecar: None,
            checksum: "checksum".to_string(),
            source_path: "/tmp/doc.txt".to_string(),
            chunks: vec![
                ChunkWithSection {
                    text: "chunk one".to_string(),
                    section: rag_chunking::SectionInfo::default(),
                },
                ChunkWithSection {
                    text: "chunk two".to_string(),
                    section: rag_chunking::SectionInfo::default(),
                },
            ],
            dense_vectors: vec![vec![0.1, 0.2, 0.3]],
            sparse_vectors: Vec::new(),
            total_tokens: 0,
            native_metadata: None,
        };

        let err = build_qdrant_points("tenant", &doc).expect_err("mismatched vectors should fail");
        assert!(err.to_string().contains("embedder returned 1 dense vectors for 2 chunks"));
    }
}
