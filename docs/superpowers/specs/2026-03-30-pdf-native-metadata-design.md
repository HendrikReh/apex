# PDF Native Metadata Extraction

**Issue:** apex-0ko
**Date:** 2026-03-30
**Status:** Design

## Goal

Extract native PDF metadata (title, author, dates, subject, keywords, creator, producer) from PDFium and surface it through the ingest pipeline. PDFs without sidecars should no longer have empty titles when the PDF itself declares one.

## Scope

### In scope
- Extend `ExtractionResult` to carry optional native metadata
- Extract metadata from PDFium's `document.metadata()` API in `PdfExtractor`
- Namespace PDF metadata under `metadata.native.pdf` in the document JSONB column
- Fall back to PDF title for `documents.title` when no sidecar title exists
- Sidecar values always take precedence over PDF-native values

### Out of scope
- Parsing or normalizing PDF date strings (preserve raw PDFium values; normalization is a separate task)
- Metadata from non-PDF formats (Markdown, TXT have no native metadata)
- Heading detection (separate issue apex-sr2)

## Design

### 1. ExtractionResult change

Add an optional metadata map to `ExtractionResult`:

```rust
pub struct ExtractionResult {
    pub text: String,
    pub native_metadata: Option<HashMap<String, String>>,
}
```

Non-PDF extractors return `None`. `PdfExtractor` populates it from `doc.metadata().iter()`.

### 2. Stable key mapping

PDFium exposes `PdfDocumentMetadataTagType` variants. Map them to stable lowercase snake_case keys:

| PDFium tag type | Stored key |
|---|---|
| `Title` | `title` |
| `Author` | `author` |
| `Subject` | `subject` |
| `Keywords` | `keywords` |
| `Creator` | `creator` |
| `Producer` | `producer` |
| `CreationDate` | `creation_date` |
| `ModificationDate` | `modification_date` |

Empty string values are omitted (not stored).

### 3. PdfExtractor implementation

Inside the `spawn_blocking` closure, after loading the document:

```rust
let native_metadata: HashMap<String, String> = doc
    .metadata()
    .iter()
    .filter_map(|tag| {
        let key = match tag.tag_type() {
            Title => "title",
            Author => "author",
            Subject => "subject",
            Keywords => "keywords",
            Creator => "creator",
            Producer => "producer",
            CreationDate => "creation_date",
            ModificationDate => "modification_date",
        };
        let value = tag.value().to_string();
        if value.is_empty() { None } else { Some((key.to_string(), value)) }
    })
    .collect();
```

Return `Some(native_metadata)` if non-empty, `None` if the PDF declares no metadata.

### 4. Ingest pipeline: metadata merge

In `extract_and_checksum`, carry `native_metadata` through `PreparedDocument`.

In the `upsert_document` call, build the JSONB metadata as:

```json
{
  "native": {
    "pdf": {
      "title": "...",
      "author": "...",
      "creation_date": "D:20260101120000+00'00'"
    }
  }
}
```

When a sidecar exists, sidecar JSON is the base and `native.pdf` is merged in. Sidecar precedence applies at every level: if sidecar JSON already contains a `native`, `native.pdf`, or any `native.pdf.*` key, those values are preserved and only missing keys are filled from the PDF. This prevents both top-level collisions and nested key overwrites.

### 5. Title resolution

Title for the `documents.title` column follows this precedence:

1. Sidecar `document.title` (explicit, curated)
2. PDF native `title` metadata (producer-supplied)
3. Empty string `""` (fallback)

This resolution applies in **both** upsert call sites in `ingest_file()`: the normal persist path and the skip-on-unchanged metadata update path. Both must use the same title resolution and metadata merge logic to avoid inconsistent behavior on re-ingest.

### 6. Non-PDF extractors

`MarkdownExtractor` and `TextExtractor` return `ExtractionResult { text, native_metadata: None }`. No behavior change.

## Testing

### Extractor-level
- `two-pages.pdf` fixture returns `native_metadata` with `creation_date` populated (confirmed via `pdfinfo`)
- Non-PDF extraction returns `native_metadata: None`

### Ingest-level
- PDF with sidecar: `documents.title` uses sidecar title, not PDF title. `metadata.native.pdf` is present alongside sidecar data.
- PDF without sidecar: `documents.title` falls back to PDF native title if present. **Requires a test fixture with `/Title` set** — `two-pages.pdf` has `CreationDate` but no title. Either create a dedicated fixture (e.g., `titled.pdf` generated via a reproducible script) or add `/Title` to the existing `two-pages.pdf` generation step.
- PDF with no native metadata: `native_metadata` field is `None`, no `native.pdf` key in JSONB.

## Crates touched

- `rag-core` (extract.rs, ingest.rs, lib.rs re-export)
- No changes to `rag-chunking`, `rag-server`, `rag-cli`, or `rag-client`

## Risk

**Low** — additive change to an existing struct. Non-PDF extractors unaffected. Sidecar precedence preserves existing behavior. No migration needed (uses existing JSONB column).
