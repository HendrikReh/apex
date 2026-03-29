# PDF Text Extraction via PDFium — Design Spec

**Beads issue:** apex-h6e
**Date:** 2026-03-29
**Status:** Approved

---

## Goal

Wire `pdfium-render` into the existing `PdfExtractor` stub so that PDF files are ingested as text. Minimal MVP: page-by-page text extraction with form-feed separators. No heading detection, no OCR fallback, no metadata extraction (tracked as follow-ups: apex-sr2, apex-4yu, apex-0ko).

## Architecture

### Component: PdfExtractor

**Location:** `crates/rag-core/src/extract.rs` (replace existing stub at lines 120-135)

**Interface:** Implements `FormatExtractor` trait:
```rust
fn supported_types(&self) -> &'static [FileType];  // returns &[FileType::Pdf]
fn extract<'a>(&'a self, content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>>;
```

**Internals:**
- Stores the PDFium library path (`PathBuf`), NOT a `Pdfium` instance.
- On each `extract()` call:
  1. Clone the library path into a `spawn_blocking` closure.
  2. Inside the closure: bind PDFium from the library path, load PDF from the byte slice, iterate pages, collect `page.text().all()` for each page.
  3. Join page texts with `\u{000C}` (form-feed) separator.
  4. Return `ExtractionResult { text }`.

**Why no persistent `Pdfium` handle:** The `FormatExtractor` trait requires `Send + Sync`. PDFium's C binding handle may not be safely shareable across threads. Creating the binding per-call inside `spawn_blocking` avoids this entirely.

### Component: ExtractorRegistry wiring

**Current code:** `ExtractorRegistry::with_defaults()` at `extract.rs:72` constructs the registry with `TextExtractor` and `MarkdownExtractor`. Called by `IngestService::new()` at `ingest.rs:104`.

**Change:** `with_defaults()` gains a `config: &AppConfig` parameter. When `config.pdfium_library_path` is `Some(path)`:
- Validate the path exists on disk. If it doesn't, fail immediately (misconfiguration).
- Construct `PdfExtractor` with the validated path and include it in the registry.

When `config.pdfium_library_path` is `None`:
- Omit `PdfExtractor` from the registry. App boots normally.
- When a PDF is later submitted for ingestion, the registry returns an error: **"no extractor registered for PDF files; set pdfium_library_path in config/app.toml or PDFIUM_LIBRARY_PATH env var to enable PDF support"**.

**Rationale:** PDFium is optional. Text/markdown-only workflows, tests, and CI environments are unaffected. Only a configured-but-invalid path is a startup error.

### Component: AppConfig

**New field:** `pdfium_library_path: Option<PathBuf>`

**Config source:** `[extract]` section in `config/app.toml`:
```toml
[extract]
# Path to the PDFium native library (.dylib on macOS, .so on Linux)
# Omit to disable PDF extraction
# pdfium_library_path = "/usr/local/lib/libpdfium.dylib"
```

**Env override:** `PDFIUM_LIBRARY_PATH` (takes precedence over TOML, following existing config pattern: env > toml > default).

### Data Flow

```
PDF bytes
  -> spawn_blocking {
       Pdfium::new(bind_to_library(path))
       -> pdfium.load_pdf_from_byte_slice(bytes)
       -> for page in doc.pages() { page.text().all() }
       -> join with \u{000C}
     }
  -> ExtractionResult { text }
```

## Error Handling

| Scenario | Behavior |
|---|---|
| `pdfium_library_path` not configured | App boots. PDF ingest returns clear error naming the missing config. |
| `pdfium_library_path` points to nonexistent file | Startup fails with "pdfium library not found at <path>" |
| Corrupt PDF | Per-file ingest failure (existing pattern), surfaced as WARN in CLI output |
| Password-protected PDF | Per-file ingest failure |
| PDF with no extractable text (scanned) | Returns empty string. OCR fallback is a separate feature (apex-4yu). |

## Dependencies

**New crate dependency:** `pdfium-render` added to `crates/rag-core/Cargo.toml`.

**Runtime dependency:** PDFium native library (`.dylib`/`.so`/`.dll`) must be installed on the system. Not bundled.

## Testing

**Integration tests only.** PDF tests require the PDFium native library, making them environment-sensitive.

- Tests gated on `PDFIUM_LIBRARY_PATH` env var. When unset, tests skip with `#[ignore]` or a runtime check printing a clear skip message.
- Test fixture: a small PDF file committed to `crates/rag-core/tests/fixtures/` (a simple 2-page document with known text content).
- Test cases:
  1. Extract text from a 2-page PDF, verify form-feed separator between pages.
  2. Verify extracted text contains expected content.
  3. Verify `PdfExtractor::supported_types()` returns `&[FileType::Pdf]`.
- Registry test: verify that `with_defaults()` without `pdfium_library_path` produces a registry that rejects PDFs with the expected error message.

## Follow-up Issues

| Issue | Description | Priority |
|---|---|---|
| apex-sr2 | PDF heading detection via font-size heuristics | P3 |
| apex-4yu | Tesseract OCR fallback for scanned PDFs | P3 |
| apex-0ko | Extract native PDF metadata (title, author, dates) | P4 |

## Non-Goals

- Heading detection (apex-sr2)
- OCR for scanned PDFs (apex-4yu)
- PDF metadata extraction (apex-0ko)
- Password-protected PDF support
- PDF form field extraction
