# PDF Native Metadata Extraction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract native PDF metadata (title, author, dates, etc.) from PDFium and surface it through the ingest pipeline with sidecar precedence.

**Architecture:** Add `native_metadata: Option<HashMap<String, String>>` to `ExtractionResult`. PdfExtractor fills it via `doc.metadata().iter()` with a stable key mapping. The ingest pipeline carries it through all intermediate structs, merges it under `metadata.native.pdf` in the JSONB column, and resolves `documents.title` via the precedence chain: sidecar title > PDF native title > `""`.

**Tech Stack:** Rust, pdfium-render 0.8 (`PdfDocumentMetadataTagType`), serde_json, Python fpdf2 (fixture generation only)

---

### Task 1: Add `native_metadata` field to `ExtractionResult`

**Files:**
- Modify: `crates/rag-core/src/extract.rs:32-36` (struct definition)
- Modify: `crates/rag-core/src/extract.rs:331` (PdfExtractor return)
- Modify: `crates/rag-core/src/extract.rs:339-345` (extract_utf8_passthrough)

- [ ] **Step 1: Write the failing test**

Add a test to `crates/rag-core/src/extract.rs` inside `mod tests` that verifies non-PDF extractors return `native_metadata: None`:

```rust
#[tokio::test]
#[allow(clippy::disallowed_methods)] // test assertions
async fn non_pdf_extractors_return_no_native_metadata() {
    let registry = test_registry();

    let result = registry
        .extract(FileType::Text, b"hello", &ExtractionOptions::default())
        .await
        .expect("text extraction should succeed");

    assert!(
        result.native_metadata.is_none(),
        "non-PDF extractors should return native_metadata: None"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core non_pdf_extractors_return_no_native_metadata -- --nocapture`
Expected: FAIL — `ExtractionResult` has no field `native_metadata`.

- [ ] **Step 3: Add the field and update all construction sites**

In `crates/rag-core/src/extract.rs`:

Change the struct (line 32-36):

```rust
/// Normalized text extracted from a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub text: String,
    /// Native metadata from the source format (e.g. PDF document info).
    /// Non-PDF extractors return `None`.
    pub native_metadata: Option<HashMap<String, String>>,
}
```

Update `extract_utf8_passthrough` (line 339-345):

```rust
fn extract_utf8_passthrough(content: &[u8], format_name: &str) -> Result<ExtractionResult> {
    let text = std::str::from_utf8(content)
        .with_context(|| format!("decoding {format_name} content as UTF-8"))?
        .to_owned();

    Ok(ExtractionResult { text, native_metadata: None })
}
```

Update PdfExtractor return (line 331):

```rust
Ok(ExtractionResult { text: pages.join("\u{000C}"), native_metadata: None })
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core non_pdf_extractors_return_no_native_metadata -- --nocapture`
Expected: PASS

- [ ] **Step 5: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(rag-core): add native_metadata field to ExtractionResult"
```

---

### Task 2: Extract metadata in PdfExtractor

**Files:**
- Modify: `crates/rag-core/src/extract.rs:249-336` (PdfExtractor::extract)
- Test: `crates/rag-core/src/extract.rs` (mod tests)

- [ ] **Step 1: Write the failing test**

Add a test to `crates/rag-core/src/extract.rs` inside `mod tests`:

```rust
#[tokio::test]
#[ignore] // requires PDFium installed
#[allow(clippy::disallowed_methods)]
async fn pdf_extractor_returns_native_metadata() {
    let config = match test_pdf_config(None) {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
            return;
        }
    };

    let extractor = PdfExtractor::new(&config).expect("PdfExtractor should construct");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
    if !fixture.exists() {
        eprintln!("skipping: fixture not found at {}", fixture.display());
        return;
    }

    let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    let metadata = result
        .native_metadata
        .as_ref()
        .expect("two-pages.pdf should have native metadata");

    assert!(
        metadata.contains_key("creation_date"),
        "two-pages.pdf should have creation_date, got keys: {:?}",
        metadata.keys().collect::<Vec<_>>()
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core pdf_extractor_returns_native_metadata -- --ignored --nocapture`
Expected: FAIL — `native_metadata` is `None` (PdfExtractor currently returns `None`).

- [ ] **Step 3: Implement metadata extraction**

In `crates/rag-core/src/extract.rs`, in the `PdfExtractor::extract` method, add the import at the top of the file (after the existing `use` statements):

```rust
use pdfium_render::pdf::document::metadata::PdfDocumentMetadataTagType;
```

Inside the `spawn_blocking` closure, after loading the document (after line 265 — after `let doc = ...`), add metadata extraction:

```rust
let native_metadata: HashMap<String, String> = doc
    .metadata()
    .iter()
    .filter_map(|tag| {
        let key = match tag.tag_type() {
            PdfDocumentMetadataTagType::Title => "title",
            PdfDocumentMetadataTagType::Author => "author",
            PdfDocumentMetadataTagType::Subject => "subject",
            PdfDocumentMetadataTagType::Keywords => "keywords",
            PdfDocumentMetadataTagType::Creator => "creator",
            PdfDocumentMetadataTagType::Producer => "producer",
            PdfDocumentMetadataTagType::CreationDate => "creation_date",
            PdfDocumentMetadataTagType::ModificationDate => "modification_date",
        };
        let value = tag.value();
        if value.is_empty() {
            None
        } else {
            Some((key.to_string(), value.to_string()))
        }
    })
    .collect();
let native_metadata =
    if native_metadata.is_empty() { None } else { Some(native_metadata) };
```

Update the return statement (line 331):

```rust
Ok(ExtractionResult {
    text: pages.join("\u{000C}"),
    native_metadata,
})
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core pdf_extractor_returns_native_metadata -- --ignored --nocapture`
Expected: PASS

- [ ] **Step 5: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

- [ ] **Step 6: Commit**

```bash
git add crates/rag-core/src/extract.rs
git commit -m "feat(rag-core): extract native metadata from PDF documents via PDFium"
```

---

### Task 3: Create `titled.pdf` test fixture

**Files:**
- Create: `crates/rag-core/tests/fixtures/titled.pdf`
- Create: `crates/rag-core/tests/fixtures/generate_titled_pdf.py` (reproducible script)
- Test: `crates/rag-core/src/extract.rs` (mod tests)

- [ ] **Step 1: Write the fixture generation script**

Create `crates/rag-core/tests/fixtures/generate_titled_pdf.py`:

```python
#!/usr/bin/env python3
"""Generate a PDF with /Title and /Author metadata for testing.

Usage:
    uv add fpdf2
    python generate_titled_pdf.py
"""
from fpdf import FPDF

pdf = FPDF()
pdf.set_title("Apex Test Document")
pdf.set_author("Test Author")
pdf.add_page()
pdf.set_font("Helvetica", size=12)
pdf.cell(text="This PDF has title and author metadata.")
pdf.output("titled.pdf")
print("Generated titled.pdf")
```

- [ ] **Step 2: Run the script to generate the fixture**

```bash
cd crates/rag-core/tests/fixtures
pip install fpdf2 --quiet 2>/dev/null || uv pip install fpdf2 --quiet
python generate_titled_pdf.py
```

- [ ] **Step 3: Verify the fixture has metadata**

```bash
pdfinfo crates/rag-core/tests/fixtures/titled.pdf
```

Expected output includes:
```
Title:           Apex Test Document
Author:          Test Author
```

- [ ] **Step 4: Write the test**

Add a test to `crates/rag-core/src/extract.rs` inside `mod tests`:

```rust
#[tokio::test]
#[ignore] // requires PDFium installed
#[allow(clippy::disallowed_methods)]
async fn pdf_extractor_returns_title_metadata_from_titled_fixture() {
    let config = match test_pdf_config(None) {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
            return;
        }
    };

    let extractor = PdfExtractor::new(&config).expect("PdfExtractor should construct");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/titled.pdf");
    if !fixture.exists() {
        eprintln!("skipping: fixture not found at {}", fixture.display());
        return;
    }

    let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    let metadata = result
        .native_metadata
        .as_ref()
        .expect("titled.pdf should have native metadata");

    assert_eq!(
        metadata.get("title").map(String::as_str),
        Some("Apex Test Document"),
        "titled.pdf should have title 'Apex Test Document'"
    );
    assert_eq!(
        metadata.get("author").map(String::as_str),
        Some("Test Author"),
        "titled.pdf should have author 'Test Author'"
    );
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rag-core pdf_extractor_returns_title_metadata_from_titled_fixture -- --ignored --nocapture`
Expected: PASS

- [ ] **Step 6: Write test for PDF with no metadata**

Add a test to `crates/rag-core/src/extract.rs` inside `mod tests`:

```rust
#[tokio::test]
#[ignore] // requires PDFium installed
#[allow(clippy::disallowed_methods)]
async fn pdf_extractor_returns_none_metadata_for_bare_pdf() {
    let config = match test_pdf_config(None) {
        Some(c) => c,
        None => {
            eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
            return;
        }
    };

    let extractor = PdfExtractor::new(&config).expect("PdfExtractor should construct");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.pdf");
    if !fixture.exists() {
        eprintln!("skipping: fixture not found at {}", fixture.display());
        return;
    }

    let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("extraction should succeed");

    assert!(
        result.native_metadata.is_none(),
        "hello_ocr.pdf has no metadata, expected None, got: {:?}",
        result.native_metadata
    );
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test -p rag-core pdf_extractor_returns_none_metadata_for_bare_pdf -- --ignored --nocapture`
Expected: PASS

- [ ] **Step 8: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

- [ ] **Step 9: Commit**

```bash
git add crates/rag-core/tests/fixtures/titled.pdf \
        crates/rag-core/tests/fixtures/generate_titled_pdf.py \
        crates/rag-core/src/extract.rs
git commit -m "test(rag-core): add titled.pdf fixture and metadata extraction tests"
```

---

### Task 4: Carry `native_metadata` through pipeline structs

**Files:**
- Modify: `crates/rag-core/src/ingest.rs:524-555` (PreparedDocument, ChunkedDocument, EmbeddedDocument)
- Modify: `crates/rag-core/src/ingest.rs:258-324` (extract_and_checksum)
- Modify: `crates/rag-core/src/ingest.rs:327-366` (chunk_text)
- Modify: `crates/rag-core/src/ingest.rs:369-392` (embed_chunks)

This is a mechanical pass-through change with no behavior change yet.

- [ ] **Step 1: Add field to `PreparedDocument`**

In `crates/rag-core/src/ingest.rs`, add `HashMap` to the imports at the top:

```rust
use std::collections::HashMap;
```

(Note: `HashSet` is already imported. Change the line to import both.)

Update `PreparedDocument` (line 524-531):

```rust
struct PreparedDocument {
    document_id: String,
    collection: String,
    sidecar: Option<Sidecar>,
    text: String,
    checksum: String,
    source_path: String,
    native_metadata: Option<HashMap<String, String>>,
}
```

- [ ] **Step 2: Add field to `ChunkedDocument`**

Update `ChunkedDocument` (line 534-542):

```rust
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
```

- [ ] **Step 3: Add field to `EmbeddedDocument`**

Update `EmbeddedDocument` (line 544-555):

```rust
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
```

- [ ] **Step 4: Pass through in `extract_and_checksum`**

In `extract_and_checksum` (line 316-323), update the `PreparedDocument` construction:

```rust
Ok(PreparedDocument {
    document_id,
    collection,
    sidecar: sidecar.clone(),
    text: result.text,
    checksum,
    source_path,
    native_metadata: result.native_metadata,
})
```

- [ ] **Step 5: Pass through in `chunk_text`**

In `chunk_text` (line 357-365), update the `ChunkedDocument` construction:

```rust
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
```

- [ ] **Step 6: Pass through in `embed_chunks`**

In `embed_chunks` (line 381-391), update the `EmbeddedDocument` construction:

```rust
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
```

- [ ] **Step 7: Update test helper that constructs `EmbeddedDocument`**

In the `build_qdrant_points_rejects_dense_vector_length_mismatch` test (line 727-749), add the field:

```rust
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
```

- [ ] **Step 8: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

Expected: compiles successfully — no behavior change, just struct plumbing.

- [ ] **Step 9: Commit**

```bash
git add crates/rag-core/src/ingest.rs
git commit -m "refactor(rag-core): carry native_metadata through ingest pipeline structs"
```

---

### Task 5: Metadata merge and title resolution helpers

**Files:**
- Modify: `crates/rag-core/src/ingest.rs` (add helper functions + unit tests)

- [ ] **Step 1: Write failing tests for `resolve_title`**

Add to `mod tests` in `crates/rag-core/src/ingest.rs`:

```rust
#[test]
fn resolve_title_prefers_sidecar() {
    let sidecar = sidecar_with_title("Sidecar Title");
    let mut native = HashMap::new();
    native.insert("title".to_string(), "PDF Title".to_string());

    assert_eq!(resolve_title(Some(&sidecar), Some(&native)), "Sidecar Title");
}

#[test]
fn resolve_title_falls_back_to_native() {
    let mut native = HashMap::new();
    native.insert("title".to_string(), "PDF Title".to_string());

    assert_eq!(resolve_title(None, Some(&native)), "PDF Title");
}

#[test]
fn resolve_title_returns_empty_when_neither() {
    assert_eq!(resolve_title(None, None), "");
}

/// Build a minimal valid sidecar with the given title for test assertions.
fn sidecar_with_title(title: &str) -> Sidecar {
    Sidecar::from_json(
        serde_json::json!({
            "schema_version": 1,
            "document": { "title": title, "category": "test" },
            "source": { "url": "u", "domain": "d", "publisher": "p" },
            "language": "en",
            "tags": ["t"],
            "acl": { "allow_roles": ["*"] },
            "security": { "classification": "public", "requires_evidence_pack": false },
            "provenance": { "retrieved_at": "now", "retrieved_by": "me" }
        })
        .to_string()
        .as_bytes(),
    )
    .expect("test sidecar should parse")
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rag-core resolve_title -- --nocapture`
Expected: FAIL — `resolve_title` function does not exist.

- [ ] **Step 3: Implement `resolve_title`**

Add to `crates/rag-core/src/ingest.rs` in the helpers section (before `discover_document_pairs`):

```rust
/// Resolve document title with precedence: sidecar > PDF native > empty.
fn resolve_title<'a>(
    sidecar: Option<&'a Sidecar>,
    native_metadata: Option<&'a HashMap<String, String>>,
) -> &'a str {
    if let Some(s) = sidecar {
        return &s.document.title;
    }
    if let Some(nm) = native_metadata {
        if let Some(title) = nm.get("title") {
            if !title.is_empty() {
                return title;
            }
        }
    }
    ""
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rag-core resolve_title -- --nocapture`
Expected: PASS

- [ ] **Step 5: Write failing tests for `build_metadata_json`**

Add to `mod tests` in `crates/rag-core/src/ingest.rs`:

```rust
#[test]
#[allow(clippy::disallowed_methods)] // serde_json::json! + test assertions
fn build_metadata_json_returns_none_when_both_absent() {
    assert!(build_metadata_json(None, None).is_none());
}

#[test]
#[allow(clippy::disallowed_methods)] // serde_json::json! + test assertions
fn build_metadata_json_returns_sidecar_only() {
    let sidecar = sidecar_with_title("Title");
    let result = build_metadata_json(Some(&sidecar), None)
        .expect("should return Some when sidecar exists");

    assert!(result.is_object());
    assert_eq!(
        result.get("document").and_then(|d| d.get("title")).and_then(|t| t.as_str()),
        Some("Title"),
    );
}

#[test]
#[allow(clippy::disallowed_methods)] // serde_json::json! + test assertions
fn build_metadata_json_returns_native_only() {
    let mut native = HashMap::new();
    native.insert("title".to_string(), "PDF Title".to_string());
    native.insert("author".to_string(), "Author".to_string());

    let result =
        build_metadata_json(None, Some(&native)).expect("should return Some for native metadata");

    assert_eq!(
        result.get("native").and_then(|n| n.get("pdf")).and_then(|p| p.get("title")).and_then(|t| t.as_str()),
        Some("PDF Title"),
    );
    assert_eq!(
        result.get("native").and_then(|n| n.get("pdf")).and_then(|p| p.get("author")).and_then(|a| a.as_str()),
        Some("Author"),
    );
}

#[test]
#[allow(clippy::disallowed_methods)] // serde_json::json! + test assertions
fn build_metadata_json_merges_native_into_sidecar() {
    let sidecar = sidecar_with_title("Sidecar Title");
    let mut native = HashMap::new();
    native.insert("title".to_string(), "PDF Title".to_string());
    native.insert("creation_date".to_string(), "D:20260101".to_string());

    let result = build_metadata_json(Some(&sidecar), Some(&native))
        .expect("should merge sidecar + native");

    // Sidecar fields preserved
    assert_eq!(
        result.get("document").and_then(|d| d.get("title")).and_then(|t| t.as_str()),
        Some("Sidecar Title"),
    );
    // Native metadata namespaced under native.pdf
    let pdf_meta = result
        .get("native")
        .and_then(|n| n.get("pdf"))
        .expect("should have native.pdf");
    assert_eq!(pdf_meta.get("title").and_then(|t| t.as_str()), Some("PDF Title"));
    assert_eq!(
        pdf_meta.get("creation_date").and_then(|t| t.as_str()),
        Some("D:20260101"),
    );
}

#[test]
#[allow(clippy::disallowed_methods)] // serde_json::json! + test assertions
fn build_metadata_json_sidecar_native_key_takes_precedence() {
    // Simulate a future sidecar that already has a native.pdf.title key.
    // We can't easily add arbitrary keys to the typed Sidecar struct,
    // so test merge_native_into_metadata directly with a pre-built Value.
    let sidecar = sidecar_with_title("Title");
    let mut base = serde_json::to_value(&sidecar).expect("serialize");
    base.as_object_mut()
        .expect("object")
        .insert(
            "native".to_string(),
            serde_json::json!({ "pdf": { "title": "Sidecar PDF Title" } }),
        );

    // Build native metadata that would conflict
    let mut native = HashMap::new();
    native.insert("title".to_string(), "Overwritten Title".to_string());
    native.insert("author".to_string(), "New Author".to_string());

    let result = merge_native_into_metadata(base.clone(), &native);

    let pdf = result
        .get("native")
        .and_then(|n| n.get("pdf"))
        .expect("native.pdf");

    // Existing sidecar key preserved
    assert_eq!(
        pdf.get("title").and_then(|t| t.as_str()),
        Some("Sidecar PDF Title"),
        "sidecar native.pdf.title should take precedence"
    );
    // Missing key filled from native
    assert_eq!(
        pdf.get("author").and_then(|t| t.as_str()),
        Some("New Author"),
        "missing native.pdf.author should be filled from PDF metadata"
    );
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test -p rag-core build_metadata_json -- --nocapture`
Expected: FAIL — functions do not exist.

- [ ] **Step 7: Implement `build_metadata_json` and `merge_native_into_metadata`**

Add to `crates/rag-core/src/ingest.rs` in the helpers section:

```rust
/// Build the JSONB metadata value for a document.
///
/// Combines sidecar JSON with native PDF metadata namespaced under
/// `native.pdf`. Sidecar values take precedence at every nesting level.
#[allow(clippy::disallowed_methods)] // serde_json::to_value internally uses .expect()
fn build_metadata_json(
    sidecar: Option<&Sidecar>,
    native_metadata: Option<&HashMap<String, String>>,
) -> Option<serde_json::Value> {
    match (sidecar, native_metadata) {
        (None, None) => None,
        (Some(s), None) => {
            Some(serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        }
        (None, Some(nm)) => Some(serde_json::json!({
            "native": { "pdf": native_map_to_value(nm) }
        })),
        (Some(s), Some(nm)) => {
            let base = serde_json::to_value(s).unwrap_or(serde_json::Value::Null);
            Some(merge_native_into_metadata(base, nm))
        }
    }
}

/// Merge native PDF metadata into an existing metadata JSON object.
///
/// Namespaces under `native.pdf`, preserving any existing keys at
/// `native`, `native.pdf`, and `native.pdf.*` levels (sidecar precedence).
fn merge_native_into_metadata(
    mut base: serde_json::Value,
    native_metadata: &HashMap<String, String>,
) -> serde_json::Value {
    if let Some(obj) = base.as_object_mut() {
        let native = obj
            .entry("native")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(native_obj) = native.as_object_mut() {
            let pdf = native_obj
                .entry("pdf")
                .or_insert_with(|| serde_json::json!({}));
            if let Some(pdf_obj) = pdf.as_object_mut() {
                for (k, v) in native_metadata {
                    pdf_obj
                        .entry(k.clone())
                        .or_insert_with(|| serde_json::Value::String(v.clone()));
                }
            }
        }
    }
    base
}

/// Convert a native metadata HashMap into a serde_json object Value.
fn native_map_to_value(nm: &HashMap<String, String>) -> serde_json::Value {
    serde_json::Value::Object(
        nm.iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    )
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p rag-core build_metadata_json -- --nocapture`
Run: `cargo test -p rag-core resolve_title -- --nocapture`
Expected: All PASS

- [ ] **Step 9: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

- [ ] **Step 10: Commit**

```bash
git add crates/rag-core/src/ingest.rs
git commit -m "feat(rag-core): add metadata merge and title resolution helpers"
```

---

### Task 6: Wire helpers into both upsert paths

**Files:**
- Modify: `crates/rag-core/src/ingest.rs:133-183` (skip-on-unchanged path in `ingest_file`)
- Modify: `crates/rag-core/src/ingest.rs:396-423` (persist method)

- [ ] **Step 1: Replace metadata + title in skip-on-unchanged path**

In `crates/rag-core/src/ingest.rs`, in `ingest_file`, replace the skip-on-unchanged block (lines 155-183):

Change:

```rust
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
```

To:

```rust
if existing_checksum.as_deref() == Some(&prepared.checksum) {
    // Checksum match — update metadata if sidecar changed, skip re-chunking.
    let metadata_json = build_metadata_json(
        prepared.sidecar.as_ref(),
        prepared.native_metadata.as_ref(),
    );
    let title = resolve_title(
        prepared.sidecar.as_ref(),
        prepared.native_metadata.as_ref(),
    );
    self.stores
        .upsert_document(
            req.tenant.as_str(),
            &prepared.document_id,
            title,
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
```

- [ ] **Step 2: Replace metadata + title in persist method**

In `persist` (lines 396-423), replace the metadata/title construction:

Change:

```rust
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
```

To:

```rust
// Upsert document row in Postgres.
let metadata_json = build_metadata_json(
    doc.sidecar.as_ref(),
    doc.native_metadata.as_ref(),
);
let title = resolve_title(
    doc.sidecar.as_ref(),
    doc.native_metadata.as_ref(),
);
self.stores
    .upsert_document(
        tenant_str,
        &doc.document_id,
        title,
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
```

- [ ] **Step 3: Remove `#[allow(clippy::disallowed_methods)]` from `persist` if no longer needed**

The `persist` method had the allow for `serde_json::to_value`. Since `build_metadata_json` now handles that call (and has its own allow), check whether `persist` still needs it. If no other call in `persist` triggers the lint, remove the attribute from line 395-396.

Similarly, `ingest_file` had the allow at line 132-133. Since `build_metadata_json` now handles the serde call, check whether `ingest_file` still needs it. If no other call in `ingest_file` triggers the lint, remove the attribute.

- [ ] **Step 4: Run cargo fmt and cargo check**

```bash
cargo fmt --all
cargo check -p rag-core
```

Expected: compiles — both upsert paths now use the shared helpers.

- [ ] **Step 5: Commit**

```bash
git add crates/rag-core/src/ingest.rs
git commit -m "feat(rag-core): wire metadata merge and title resolution into ingest pipeline"
```

---

### Task 7: Verify full pipeline (manual sanity check)

This task is a verification pass — no new code, just running existing tests.

- [ ] **Step 1: Run all non-ignored rag-core tests**

```bash
cargo test -p rag-core
```

Expected: all pass. The `ExtractionResult` struct change is backward-compatible (new field with `None` default behavior).

- [ ] **Step 2: Run PDFium-dependent tests (if PDFium is available)**

```bash
cargo test -p rag-core -- --ignored --nocapture 2>&1 | head -60
```

Expected: metadata tests pass. OCR tests pass (existing behavior preserved).

- [ ] **Step 3: Run cargo check on entire workspace**

```bash
cargo check --workspace
```

Expected: no errors. `ExtractionResult` is re-exported from `rag-core/src/lib.rs` (line 30) — any downstream crate that pattern-matches on it will get a compile error if it doesn't handle the new field, but no crate currently does this.

- [ ] **Step 4: Run cargo fmt check**

```bash
cargo fmt --all -- --check
```

Expected: no formatting issues.
