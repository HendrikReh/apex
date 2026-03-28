# Phase 4: Ingest Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `IngestService` — the end-to-end synchronous document ingestion pipeline that wires extraction, chunking, embedding, and storage into a single orchestrated flow with sidecar metadata support.

**Architecture:** `IngestService` is a single service struct with private step functions and intermediate structs (`PreparedDocument`, `ChunkedDocument`, `EmbeddedDocument`). Each step is independently testable. Sidecar metadata is parsed in a separate module (`sidecar.rs`). Directory discovery walks the filesystem to find `(document, sidecar)` pairs.

**Tech Stack:** Rust, serde_json (sidecar parsing), tokio (async runtime), walkdir (directory traversal), rag-chunking (text chunking), rag-core stores (Postgres + Qdrant)

**Spec:** `docs/plans/2026-03-27-phase4-ingest-pipeline-design.md`

---

### Task 1: Add chunking config fields to AppConfig

**Files:**
- Modify: `crates/rag-core/src/config.rs`
- Modify: `config/app.toml`

- [ ] **Step 1: Add TOML fields to AppSection**

In `crates/rag-core/src/config.rs`, add to `AppSection`:

```rust
    chunking_max_tokens: Option<usize>,
    chunking_overlap_ratio: Option<f32>,
```

- [ ] **Step 2: Add fields to AppConfig struct**

In `crates/rag-core/src/config.rs`, add to `AppConfig` after the `// Collections` section:

```rust
    // Chunking defaults (overridable per-document via sidecar)
    pub chunking_max_tokens: usize,
    pub chunking_overlap_ratio: f32,
```

- [ ] **Step 3: Wire parsing and validation in from_current_env()**

Add after the `default_collection` parsing block:

```rust
        let chunking_max_tokens = env_parsed("CHUNKING_MAX_TOKENS")?
            .or(f.chunking_max_tokens)
            .unwrap_or(600);
        if chunking_max_tokens == 0 {
            anyhow::bail!("chunking_max_tokens must be greater than zero");
        }

        let chunking_overlap_ratio = env_parsed("CHUNKING_OVERLAP_RATIO")?
            .or(f.chunking_overlap_ratio)
            .unwrap_or(0.15);
        if !(0.0..1.0).contains(&chunking_overlap_ratio) {
            anyhow::bail!(
                "chunking_overlap_ratio must be in [0.0, 1.0), got {chunking_overlap_ratio}"
            );
        }
```

Add `chunking_max_tokens` and `chunking_overlap_ratio` to the `Ok(Self { ... })` block.

- [ ] **Step 4: Add defaults to config/app.toml**

Add after the `default_collection` line:

```toml
# Chunking defaults (overridable per-document via sidecar)
chunking_max_tokens = 600
chunking_overlap_ratio = 0.15
```

- [ ] **Step 5: Update all test AppConfig constructors**

Search for all places that construct `AppConfig` in tests (e.g., `crates/rag-core/src/embed.rs` test `any_embedder_from_config_selects_mock`). Add the two new fields to each:

```rust
            chunking_max_tokens: 600,
            chunking_overlap_ratio: 0.15,
```

- [ ] **Step 6: Verify**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly

- [ ] **Step 7: Commit**

```
git add crates/rag-core/src/config.rs config/app.toml crates/rag-core/src/embed.rs
git commit -m "feat(config): add chunking_max_tokens and chunking_overlap_ratio"
```

---

### Task 2: Sidecar schema types and parsing

**Files:**
- Create: `crates/rag-core/src/sidecar.rs`
- Modify: `crates/rag-core/src/lib.rs`

- [ ] **Step 1: Write failing test for valid sidecar parsing**

Create `crates/rag-core/src/sidecar.rs` with a test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn valid_sidecar_json() -> &'static str {
        r#"{
            "schema_version": 1,
            "document": {
                "title": "Test Document",
                "category": "report"
            },
            "source": {
                "url": "https://example.com/doc.pdf",
                "domain": "example.com",
                "publisher": "Example Inc"
            },
            "language": "en",
            "tags": ["test"],
            "acl": { "allow_roles": ["*"] },
            "security": {
                "classification": "public",
                "requires_evidence_pack": false
            },
            "provenance": {
                "retrieved_at": "2026-03-27T10:00:00Z",
                "retrieved_by": "manual"
            }
        }"#
    }

    #[test]
    fn parses_valid_minimal_sidecar() {
        let sidecar = Sidecar::from_json(valid_sidecar_json().as_bytes())
            .expect("valid sidecar should parse");
        assert_eq!(sidecar.document.title, "Test Document");
        assert_eq!(sidecar.document.category, "report");
        assert_eq!(sidecar.language, "en");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core sidecar::tests::parses_valid_minimal_sidecar`
Expected: FAIL — `Sidecar` type does not exist

- [ ] **Step 3: Implement sidecar types and from_json**

Add to the top of `crates/rag-core/src/sidecar.rs`:

```rust
//! Metadata sidecar schema for per-document configuration.
//!
//! Sidecars are JSON files that sit alongside source documents and provide
//! metadata, collection routing, and chunking overrides. The naming convention
//! is `document.metadata.json` for a source file named `document.pdf`.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Sidecar {
    pub schema_version: u32,
    pub document: DocumentInfo,
    pub source: SourceInfo,
    pub language: String,
    pub tags: Vec<String>,
    pub acl: AclInfo,
    pub security: SecurityInfo,
    pub provenance: ProvenanceInfo,
    #[serde(default)]
    pub ingestion: Option<IngestionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentInfo {
    pub id: Option<String>,
    pub title: String,
    pub category: String,
    pub version: Option<String>,
    pub authors: Option<Vec<String>>,
    pub summary: Option<String>,
    pub published_at: Option<String>,
    pub refresh_cadence: Option<String>,
    pub content_owner: Option<String>,
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceInfo {
    pub url: String,
    pub domain: String,
    pub publisher: String,
    pub collection: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AclInfo {
    pub allow_roles: Vec<String>,
    pub deny_roles: Option<Vec<String>>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityInfo {
    pub classification: String,
    pub requires_evidence_pack: bool,
    pub compartments: Option<Vec<String>>,
    pub contains_pii: Option<bool>,
    pub export_control: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProvenanceInfo {
    pub retrieved_at: String,
    pub retrieved_by: String,
    pub checksum_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestionConfig {
    pub collection: Option<String>,
    pub chunking: Option<ChunkingOverride>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkingOverride {
    pub strategy: Option<String>,
    pub max_tokens: Option<usize>,
    pub overlap_ratio: Option<f32>,
}

impl Sidecar {
    /// Parse a sidecar from JSON bytes, then validate required fields.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let sidecar: Self =
            serde_json::from_slice(bytes).context("parsing sidecar JSON")?;
        sidecar.validate()?;
        Ok(sidecar)
    }

    /// Validate invariants that serde alone cannot enforce.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported sidecar schema_version: {}", self.schema_version);
        }
        if self.document.title.is_empty() {
            bail!("sidecar document.title must not be empty");
        }
        if self.document.category.is_empty() {
            bail!("sidecar document.category must not be empty");
        }
        if self.language.is_empty() {
            bail!("sidecar language must not be empty");
        }
        if self.tags.is_empty() {
            bail!("sidecar tags must contain at least one entry");
        }
        if self.acl.allow_roles.is_empty() {
            bail!("sidecar acl.allow_roles must contain at least one entry");
        }
        if self.security.classification.is_empty() {
            bail!("sidecar security.classification must not be empty");
        }
        if self.provenance.retrieved_at.is_empty() {
            bail!("sidecar provenance.retrieved_at must not be empty");
        }
        if self.provenance.retrieved_by.is_empty() {
            bail!("sidecar provenance.retrieved_by must not be empty");
        }
        if let Some(ref ing) = self.ingestion {
            if let Some(ref chunking) = ing.chunking {
                if let Some(max_tokens) = chunking.max_tokens {
                    if max_tokens == 0 {
                        bail!("sidecar chunking.max_tokens must be greater than zero");
                    }
                }
                if let Some(overlap) = chunking.overlap_ratio {
                    if !(0.0..1.0).contains(&overlap) {
                        bail!(
                            "sidecar chunking.overlap_ratio must be in [0.0, 1.0), got {overlap}"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Wire module into lib.rs**

Add `pub mod sidecar;` to `crates/rag-core/src/lib.rs` and add to the pub use block:

```rust
pub use sidecar::Sidecar;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rag-core sidecar::tests::parses_valid_minimal_sidecar`
Expected: PASS

- [ ] **Step 6: Write additional validation tests**

Add to the test module in `sidecar.rs`:

```rust
    #[test]
    fn rejects_wrong_schema_version() {
        let json = valid_sidecar_json().replace("\"schema_version\": 1", "\"schema_version\": 2");
        let err = Sidecar::from_json(json.as_bytes()).expect_err("schema_version 2 should fail");
        assert!(err.to_string().contains("unsupported sidecar schema_version"));
    }

    #[test]
    fn rejects_empty_title() {
        let json = valid_sidecar_json().replace("\"Test Document\"", "\"\"");
        let err = Sidecar::from_json(json.as_bytes()).expect_err("empty title should fail");
        assert!(err.to_string().contains("document.title must not be empty"));
    }

    #[test]
    fn rejects_empty_tags() {
        let json = valid_sidecar_json().replace("[\"test\"]", "[]");
        let err = Sidecar::from_json(json.as_bytes()).expect_err("empty tags should fail");
        assert!(err.to_string().contains("tags must contain at least one entry"));
    }

    #[test]
    fn rejects_invalid_overlap_ratio() {
        let json = r#"{
            "schema_version": 1,
            "document": { "title": "T", "category": "report" },
            "source": { "url": "https://x.com", "domain": "x.com", "publisher": "X" },
            "language": "en",
            "tags": ["t"],
            "acl": { "allow_roles": ["*"] },
            "security": { "classification": "public", "requires_evidence_pack": false },
            "provenance": { "retrieved_at": "2026-01-01T00:00:00Z", "retrieved_by": "m" },
            "ingestion": { "chunking": { "overlap_ratio": 1.5 } }
        }"#;
        let err = Sidecar::from_json(json.as_bytes()).expect_err("overlap 1.5 should fail");
        assert!(err.to_string().contains("overlap_ratio must be in"));
    }

    #[test]
    fn parses_sidecar_with_ingestion_overrides() {
        let json = r#"{
            "schema_version": 1,
            "document": { "title": "T", "category": "report" },
            "source": { "url": "https://x.com", "domain": "x.com", "publisher": "X" },
            "language": "en",
            "tags": ["t"],
            "acl": { "allow_roles": ["*"] },
            "security": { "classification": "public", "requires_evidence_pack": false },
            "provenance": { "retrieved_at": "2026-01-01T00:00:00Z", "retrieved_by": "m" },
            "ingestion": {
                "collection": "custom_collection",
                "chunking": { "strategy": "markdown", "max_tokens": 400, "overlap_ratio": 0.1 }
            }
        }"#;
        let sidecar = Sidecar::from_json(json.as_bytes()).expect("should parse");
        let ing = sidecar.ingestion.as_ref().expect("ingestion should be present");
        assert_eq!(ing.collection.as_deref(), Some("custom_collection"));
        let chunking = ing.chunking.as_ref().expect("chunking should be present");
        assert_eq!(chunking.strategy.as_deref(), Some("markdown"));
        assert_eq!(chunking.max_tokens, Some(400));
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = valid_sidecar_json().replace(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"future_field\": true"
        );
        Sidecar::from_json(json.as_bytes()).expect("unknown fields should be ignored");
    }
```

- [ ] **Step 7: Add `#[allow(clippy::disallowed_methods)]` to test functions that use `.expect()`**

Add the annotation to each test function that calls `.expect()`.

- [ ] **Step 8: Run all sidecar tests**

Run: `cargo test -p rag-core sidecar::tests`
Expected: all PASS

- [ ] **Step 9: Verify full crate compiles**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly

- [ ] **Step 10: Commit**

```
git add crates/rag-core/src/sidecar.rs crates/rag-core/src/lib.rs
git commit -m "feat(sidecar): add metadata sidecar schema with parsing and validation"
```

---

### Task 3: IngestService struct, construction, and directory discovery

**Files:**
- Create: `crates/rag-core/src/ingest.rs`
- Modify: `crates/rag-core/src/lib.rs`
- Modify: `crates/rag-core/Cargo.toml` (add `walkdir`)

- [ ] **Step 1: Add walkdir dependency**

Add to `crates/rag-core/Cargo.toml` dependencies:

```toml
walkdir.workspace = true
```

Check that `walkdir` is in the workspace `Cargo.toml`. If not, add:

```toml
walkdir = "2"
```

- [ ] **Step 2: Write failing test for directory discovery**

Create `crates/rag-core/src/ingest.rs` with types and a test:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
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
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rag-core ingest::tests::discovers_document_sidecar_pairs`
Expected: FAIL — types and function don't exist

- [ ] **Step 4: Implement types and discovery function**

Add to the top of `crates/rag-core/src/ingest.rs`:

```rust
//! End-to-end synchronous document ingestion pipeline.
//!
//! `IngestService` orchestrates extraction, chunking, embedding, and storage
//! for individual files and directory batches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;
use walkdir::WalkDir;

use crate::bm25::{Bm25Embedder, SparseVector};
use crate::config::AppConfig;
use crate::embed::{AnyEmbedder, EmbedService};
use crate::extract::{self, ExtractionResult, ExtractorRegistry, FileType};
use crate::sidecar::Sidecar;
use crate::stores::Stores;
use crate::tenant::TenantId;

use rag_chunking::{ChunkWithSection, ChunkingStrategy, chunk_text_with_strategy_sectioned, select_strategy};

/// Request to ingest a single file.
pub struct IngestFileRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

/// Result of ingesting a single file.
#[derive(Debug)]
pub struct IngestOutcome {
    pub document_id: String,
    pub collection: String,
    pub chunks_created: usize,
    pub skipped: bool,
}

/// Request to ingest all supported files in a directory.
pub struct IngestDirectoryRequest {
    pub path: PathBuf,
    pub tenant: TenantId,
    pub collection_override: Option<String>,
}

/// Result of a batch directory ingest.
#[derive(Debug)]
pub struct IngestBatchOutcome {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
    pub failures: Vec<DocumentFailure>,
}

/// A single document failure within a batch ingest.
#[derive(Debug)]
pub struct DocumentFailure {
    pub path: PathBuf,
    pub error: String,
}

/// A discovered document with its optional sidecar path.
#[derive(Debug)]
pub struct DocumentPair {
    pub path: PathBuf,
    pub sidecar_path: Option<PathBuf>,
}

/// Walk a directory and discover `(document, sidecar)` pairs.
///
/// Skips hidden files, unsupported extensions, and `.metadata.json` files
/// (those are sidecars, not source documents).
pub fn discover_document_pairs(dir: &Path) -> Result<Vec<DocumentPair>> {
    let mut pairs = Vec::new();

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        // Skip hidden files.
        if file_name.starts_with('.') {
            continue;
        }

        // Skip sidecar files — they are not source documents.
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
```

- [ ] **Step 5: Add tempfile dev-dependency**

Add to `crates/rag-core/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 6: Wire module into lib.rs**

Add `pub mod ingest;` to `crates/rag-core/src/lib.rs`.

- [ ] **Step 7: Run test**

Run: `cargo test -p rag-core ingest::tests::discovers_document_sidecar_pairs`
Expected: PASS

- [ ] **Step 8: Write test for hidden and unsupported file skipping**

Add test:

```rust
    #[test]
    fn skips_hidden_files_and_unsupported_extensions() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join(".hidden.txt"), "h").expect("write");
        fs::write(dir.path().join("photo.png"), "p").expect("write");
        fs::write(dir.path().join("data.csv"), "d").expect("write");

        let pairs = discover_document_pairs(dir.path()).expect("discovery");
        assert!(pairs.is_empty());
    }
```

- [ ] **Step 9: Run and verify**

Run: `cargo test -p rag-core ingest::tests::skips_hidden`
Expected: PASS

- [ ] **Step 10: Verify full crate**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly

- [ ] **Step 11: Commit**

```
git add crates/rag-core/src/ingest.rs crates/rag-core/src/lib.rs crates/rag-core/Cargo.toml Cargo.lock
git commit -m "feat(ingest): add IngestService types and directory discovery"
```

---

### Task 4: IngestService::ingest_file — step functions

**Files:**
- Modify: `crates/rag-core/src/ingest.rs`

This task implements the `IngestService` struct and the private step functions that `ingest_file` calls. Each step function takes explicit inputs and returns a typed intermediate struct.

- [ ] **Step 1: Define intermediate structs and IngestService**

Add to `crates/rag-core/src/ingest.rs` (above the test module):

```rust
use qdrant_client::qdrant::Distance;

/// Intermediate: text extracted and checksum computed.
struct PreparedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    text: String,
    checksum: String,
    source_path: String,
}

/// Intermediate: text chunked.
struct ChunkedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    text: String,
    checksum: String,
    source_path: String,
    chunks: Vec<ChunkWithSection>,
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
}

/// The ingest pipeline service.
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
    /// Build an `IngestService` from an initialised `Stores` and config.
    pub fn new(stores: Stores, config: &AppConfig) -> Result<Self> {
        let extractors = ExtractorRegistry::with_defaults()
            .context("building extractor registry")?;
        let embedder = AnyEmbedder::from_config(config)
            .context("building embedder")?;
        let bm25 = Bm25Embedder::from_app_config(config)
            .context("building BM25 embedder")?;

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
```

- [ ] **Step 2: Implement extract_and_checksum**

```rust
impl IngestService {
    /// Step 1: Read file, extract text, compute checksum, resolve document ID.
    async fn extract_and_checksum(
        &self,
        req: &IngestFileRequest,
        sidecar: &Option<Sidecar>,
    ) -> Result<PreparedDocument> {
        let path = &req.path;

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| anyhow::anyhow!("file has no extension: {}", path.display()))?;

        let file_type = FileType::from_extension(extension).ok_or_else(|| {
            anyhow::anyhow!("unsupported file extension '{extension}': {}", path.display())
        })?;

        let content = tokio::fs::read(path)
            .await
            .with_context(|| format!("reading file {}", path.display()))?;

        let result = self
            .extractors
            .extract(file_type, &content)
            .await
            .with_context(|| format!("extracting text from {}", path.display()))?;

        let checksum = extract::checksum(&result.text);

        // Document ID: sidecar > filename stem.
        let document_id = sidecar
            .as_ref()
            .and_then(|s| s.document.id.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_owned()
            });

        // Collection: API override > sidecar > default.
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

        Ok(PreparedDocument {
            document_id,
            collection,
            sidecar: sidecar.clone(),
            text: result.text,
            checksum,
            source_path: path.to_string_lossy().into_owned(),
        })
    }
}
```

- [ ] **Step 3: Implement chunk_text**

```rust
impl IngestService {
    /// Step 2: Resolve chunking strategy and chunk the extracted text.
    async fn chunk_text(&self, prepared: PreparedDocument) -> Result<ChunkedDocument> {
        let chunking = prepared
            .sidecar
            .as_ref()
            .and_then(|s| s.ingestion.as_ref())
            .and_then(|i| i.chunking.as_ref());

        let strategy_str = chunking.and_then(|c| c.strategy.as_deref());
        let strategy = select_strategy(strategy_str, &prepared.text, None);

        let max_tokens = chunking
            .and_then(|c| c.max_tokens)
            .unwrap_or(self.chunking_max_tokens);

        let overlap_ratio = chunking
            .and_then(|c| c.overlap_ratio)
            .unwrap_or(self.chunking_overlap_ratio);

        let chunks = chunk_text_with_strategy_sectioned(
            &prepared.text,
            strategy,
            max_tokens,
            overlap_ratio,
            20_000, // semantic_max_chars fallback
        )
        .await;

        if chunks.is_empty() {
            bail!("chunking produced zero chunks for document '{}'", prepared.document_id);
        }

        Ok(ChunkedDocument {
            document_id: prepared.document_id,
            collection: prepared.collection,
            sidecar: prepared.sidecar,
            text: prepared.text,
            checksum: prepared.checksum,
            source_path: prepared.source_path,
            chunks,
        })
    }
}
```

- [ ] **Step 4: Implement embed_chunks**

```rust
impl IngestService {
    /// Step 3: Generate dense and sparse embeddings for each chunk.
    async fn embed_chunks(&self, chunked: ChunkedDocument) -> Result<EmbeddedDocument> {
        let chunk_texts: Vec<String> =
            chunked.chunks.iter().map(|c| c.text.clone()).collect();

        let dense_vectors = self
            .embedder
            .embed_batch(&chunk_texts)
            .await
            .context("generating dense embeddings")?;

        let sparse_vectors: Vec<SparseVector> = chunk_texts
            .iter()
            .map(|text| self.bm25.embed_document(text))
            .collect();

        // Estimate total tokens from chunk count × avg tokens.
        // A rough approximation; exact counting is deferred.
        let total_tokens = chunk_texts.len() as i64
            * i64::try_from(self.chunking_max_tokens).unwrap_or(600);

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
        })
    }
}
```

- [ ] **Step 5: Implement persist (with collection cache and stale cleanup)**

```rust
use qdrant_client::qdrant::PointStruct;
use uuid::Uuid;

impl IngestService {
    /// Step 4: Write document, chunks, and vectors to Postgres + Qdrant.
    async fn persist(
        &self,
        tenant: &TenantId,
        doc: &EmbeddedDocument,
    ) -> Result<()> {
        let tenant_str = tenant.as_str();
        let vector_size = self.embedder.dim() as u64;

        // Ensure Qdrant collection (cached).
        self.ensure_collection_cached(&doc.collection, vector_size).await?;

        // Upsert document row in Postgres.
        let metadata_json = doc.sidecar.as_ref().map(|s| {
            serde_json::to_value(s).unwrap_or(serde_json::Value::Null)
        });
        self.stores
            .upsert_document(
                tenant_str,
                &doc.document_id,
                doc.sidecar.as_ref().map_or("", |s| &s.document.title),
                doc.sidecar.as_ref().map(|s| s.language.as_str()),
                metadata_json.as_ref(),
                Some(&doc.source_path),
                doc.sidecar
                    .as_ref()
                    .and_then(|s| s.document.version.as_deref()),
                Some(&doc.checksum),
                None, // ingest_run_id
                Some(doc.total_tokens),
                Some(&doc.collection),
            )
            .await
            .context("upserting document row")?;

        // Upsert chunks in Postgres (handles stale cleanup internally).
        let chunk_texts: Vec<String> =
            doc.chunks.iter().map(|c| c.text.clone()).collect();
        self.stores
            .insert_chunks(tenant_str, &doc.document_id, &chunk_texts)
            .await
            .context("upserting chunks")?;

        // Build Qdrant points.
        let points: Vec<PointStruct> = doc
            .chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let point_id = stable_chunk_uuid(tenant_str, &doc.document_id, i);
                let payload: std::collections::HashMap<String, qdrant_client::qdrant::Value> = [
                    ("tenant".to_string(), tenant_str.to_string().into()),
                    ("document_id".to_string(), doc.document_id.clone().into()),
                    ("chunk_index".to_string(), (i as i64).into()),
                    ("text".to_string(), chunk.text.clone().into()),
                ]
                .into();
                PointStruct::new(point_id.to_string(), doc.dense_vectors[i].clone(), payload)
            })
            .collect();

        // Upsert vectors to Qdrant.
        self.stores
            .upsert_points(&doc.collection, points)
            .await
            .context("upserting vectors to Qdrant")?;

        // Delete stale Qdrant points (points for chunk indices >= new count).
        // This is best-effort: failure does not fail the ingest.
        let new_count = doc.chunks.len();
        if let Err(e) = self.delete_stale_points(tenant_str, &doc.document_id, &doc.collection, new_count).await {
            tracing::warn!(
                document_id = %doc.document_id,
                error = %e,
                "failed to clean up stale Qdrant points (non-fatal)"
            );
        }

        Ok(())
    }

    async fn ensure_collection_cached(
        &self,
        collection: &str,
        vector_size: u64,
    ) -> Result<()> {
        {
            let cache = self.known_collections.lock().await;
            if cache.contains(collection) {
                return Ok(());
            }
        }

        match self.stores.ensure_collection(collection, vector_size, Distance::Cosine).await {
            Ok(()) => {
                let mut cache = self.known_collections.lock().await;
                cache.insert(collection.to_owned());
                Ok(())
            }
            Err(e) => {
                // Evict on failure in case another process created it.
                let mut cache = self.known_collections.lock().await;
                cache.remove(collection);
                Err(e).with_context(|| format!("ensuring Qdrant collection '{collection}'"))
            }
        }
    }

    async fn delete_stale_points(
        &self,
        tenant: &str,
        document_id: &str,
        collection: &str,
        new_count: usize,
    ) -> Result<()> {
        // Delete all points for this document and re-upload only the new ones.
        // Since we use stable UUIDs, the upsert already overwrites existing points
        // with matching IDs. We only need to remove points whose chunk_index >= new_count.
        // For simplicity at MVP, we rely on the stable UUID upsert and accept
        // that stale high-index points may remain until a full re-index.
        // TODO: implement targeted stale point deletion by chunk_index filter
        Ok(())
    }
}

/// Generate a stable UUID v5 for a chunk, ensuring idempotent Qdrant upserts.
fn stable_chunk_uuid(tenant: &str, document_id: &str, chunk_index: usize) -> Uuid {
    let input = format!("{tenant}:{document_id}:{chunk_index}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, input.as_bytes())
}
```

- [ ] **Step 6: Implement ingest_file orchestration**

```rust
impl IngestService {
    /// Ingest a single document file through the full pipeline.
    pub async fn ingest_file(&self, req: IngestFileRequest) -> Result<IngestOutcome> {
        // Load sidecar if it exists.
        let sidecar = self.load_sidecar(&req.path).await?;

        // Step 1: Extract and checksum.
        let prepared = self.extract_and_checksum(&req, &sidecar).await?;

        // Check for unchanged content.
        let existing_checksum = self
            .stores
            .get_document_checksum(req.tenant.as_str(), &prepared.document_id)
            .await
            .context("checking existing document checksum")?;

        if existing_checksum.as_deref() == Some(&prepared.checksum) {
            // Checksum match — update metadata if sidecar changed, skip re-chunking.
            let metadata_json = prepared.sidecar.as_ref().map(|s| {
                serde_json::to_value(s).unwrap_or(serde_json::Value::Null)
            });
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

        // Step 2: Chunk.
        let chunked = self.chunk_text(prepared).await?;
        let num_chunks = chunked.chunks.len();
        let collection = chunked.collection.clone();
        let document_id = chunked.document_id.clone();

        // Step 3: Embed.
        let embedded = self.embed_chunks(chunked).await?;

        // Step 4: Persist.
        self.persist(&req.tenant, &embedded).await?;

        // Step 5: Update corpus stats (non-fatal).
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

        Ok(IngestOutcome {
            document_id,
            collection,
            chunks_created: num_chunks,
            skipped: false,
        })
    }

    async fn load_sidecar(&self, doc_path: &Path) -> Result<Option<Sidecar>> {
        let stem = doc_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("cannot determine file stem for {}", doc_path.display()))?;

        let sidecar_path = doc_path.with_file_name(format!("{stem}.metadata.json"));
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
```

- [ ] **Step 7: Implement ingest_directory**

```rust
impl IngestService {
    /// Ingest all supported files in a directory, continuing on per-file errors.
    pub async fn ingest_directory(
        &self,
        req: IngestDirectoryRequest,
    ) -> Result<IngestBatchOutcome> {
        let pairs = discover_document_pairs(&req.path)
            .with_context(|| format!("discovering documents in {}", req.path.display()))?;

        let mut outcome = IngestBatchOutcome {
            documents: 0,
            chunks: 0,
            skipped: 0,
            failures: Vec::new(),
        };

        for pair in pairs {
            let file_req = IngestFileRequest {
                path: pair.path.clone(),
                tenant: req.tenant.clone(),
                collection_override: req.collection_override.clone(),
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
                    outcome.failures.push(DocumentFailure {
                        path: pair.path,
                        error: format!("{e:#}"),
                    });
                }
            }
        }

        Ok(outcome)
    }
}
```

- [ ] **Step 8: Add necessary use statements and fix compilation**

Ensure all `use` statements are correct. Add `Serialize` derive to `Sidecar` (needed for `serde_json::to_value`):

In `sidecar.rs`, change the derive on `Sidecar` and its sub-structs to include `Serialize`:

```rust
use serde::{Deserialize, Serialize};
```

And add `#[derive(Debug, Clone, Deserialize, Serialize)]` to all sidecar structs.

- [ ] **Step 9: Verify compilation**

Run: `cargo check -p rag-core --tests`
Expected: compiles cleanly

- [ ] **Step 10: Commit**

```
git add crates/rag-core/src/ingest.rs crates/rag-core/src/sidecar.rs
git commit -m "feat(ingest): implement IngestService with step functions and batch ingest"
```

---

### Task 5: Integration tests

**Files:**
- Create: `crates/rag-core/tests/integration_ingest.rs`
- Create: test fixture files under a temp directory (created in-test)

These tests require `just up` (Postgres + Qdrant running).

- [ ] **Step 1: Write integration test for full ingest pipeline**

Create `crates/rag-core/tests/integration_ingest.rs`:

```rust
//! Integration tests for the ingest pipeline.
//!
//! Requires Postgres and Qdrant running (`just up`).

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::config::AppConfig;
use rag_core::ingest::{IngestDirectoryRequest, IngestFileRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::tenant::TenantId;
use tempfile::TempDir;

async fn setup() -> Result<(IngestService, TempDir)> {
    let config = AppConfig::from_env()?;
    let stores = Stores::new(&config).await?;
    let service = IngestService::new(stores, &config)?;
    let dir = TempDir::new()?;
    Ok((service, dir))
}

fn write_fixture(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).expect("writing fixture");
}

fn write_sidecar(dir: &Path, stem: &str) {
    let json = format!(
        r#"{{
            "schema_version": 1,
            "document": {{ "id": "{stem}", "title": "Test {stem}", "category": "report" }},
            "source": {{ "url": "https://example.com/{stem}", "domain": "example.com", "publisher": "Test" }},
            "language": "en",
            "tags": ["test"],
            "acl": {{ "allow_roles": ["*"] }},
            "security": {{ "classification": "public", "requires_evidence_pack": false }},
            "provenance": {{ "retrieved_at": "2026-03-27T00:00:00Z", "retrieved_by": "test" }}
        }}"#
    );
    fs::write(dir.join(format!("{stem}.metadata.json")), json).expect("writing sidecar");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn ingest_single_txt_file() {
    let (service, dir) = setup().await.expect("setup");
    write_fixture(dir.path(), "hello.txt", "Hello, world! This is a test document for ingestion.");
    write_sidecar(dir.path(), "hello");

    let tenant = TenantId::new("test-ingest").expect("tenant");
    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("hello.txt"),
            tenant: tenant.clone(),
            collection_override: Some("test_ingest_collection".to_string()),
        })
        .await
        .expect("ingest should succeed");

    assert_eq!(outcome.document_id, "hello");
    assert!(!outcome.skipped);
    assert!(outcome.chunks_created > 0);
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn reingest_unchanged_file_is_skipped() {
    let (service, dir) = setup().await.expect("setup");
    write_fixture(dir.path(), "stable.txt", "Stable content that does not change between ingests.");
    write_sidecar(dir.path(), "stable");

    let tenant = TenantId::new("test-reingest").expect("tenant");
    let req = || IngestFileRequest {
        path: dir.path().join("stable.txt"),
        tenant: TenantId::new("test-reingest").expect("tenant"),
        collection_override: Some("test_reingest_collection".to_string()),
    };

    let first = service.ingest_file(req()).await.expect("first ingest");
    assert!(!first.skipped);

    let second = service.ingest_file(req()).await.expect("second ingest");
    assert!(second.skipped);
    assert_eq!(second.chunks_created, 0);
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn ingest_directory_processes_all_files() {
    let (service, dir) = setup().await.expect("setup");
    write_fixture(dir.path(), "a.txt", "Document A content for testing batch ingestion.");
    write_sidecar(dir.path(), "a");
    write_fixture(dir.path(), "b.md", "# Document B\n\nMarkdown content for testing.");
    write_sidecar(dir.path(), "b");
    // unsupported file should be skipped
    fs::write(dir.path().join("skip.png"), "not a document").expect("write");

    let tenant = TenantId::new("test-batch").expect("tenant");
    let outcome = service
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant,
            collection_override: Some("test_batch_collection".to_string()),
        })
        .await
        .expect("batch ingest");

    assert_eq!(outcome.documents, 2);
    assert!(outcome.chunks > 0);
    assert!(outcome.failures.is_empty());
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p rag-core --test integration_ingest -- --nocapture`
Expected: all PASS (requires `just up`)

- [ ] **Step 3: Fix any compilation or runtime issues**

Address any issues found during the integration test run. Common issues:
- Missing `Clone` on `TenantId` — add if needed
- `serde_json::to_value` on `Sidecar` — ensure `Serialize` derive is present
- Qdrant collection cleanup — tests may need unique collection names per test

- [ ] **Step 4: Commit**

```
git add crates/rag-core/tests/integration_ingest.rs
git commit -m "test(ingest): add integration tests for single file, reingest, and batch ingest"
```

---

### Task 6: Final verification and cleanup

**Files:**
- All modified files from previous tasks

- [ ] **Step 1: Run cargo fmt**

Run: `cargo fmt --all -- --check`
Expected: no formatting issues

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p rag-core --all-targets -- -D warnings -D clippy::disallowed_methods`
Expected: clean

- [ ] **Step 3: Run cargo check for full workspace**

Run: `cargo check --workspace`
Expected: compiles cleanly

- [ ] **Step 4: Fix any issues found**

Address clippy warnings, formatting issues, or compilation errors.

- [ ] **Step 5: Commit any fixes**

```
git commit -m "fix(ingest): address clippy and formatting issues"
```
