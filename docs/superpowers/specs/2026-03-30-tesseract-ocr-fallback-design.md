# Tesseract OCR Fallback for Scanned PDFs — Design Spec

**Beads issue:** apex-4yu
**Date:** 2026-03-30
**Status:** Approved

---

## Goal

Add a Tesseract OCR fallback to `PdfExtractor` so that scanned PDF pages (image-only, no embedded text) produce usable extracted text. OCR triggers automatically when a page's extracted text is empty, or manually via sidecar `ocr.force`. Tesseract is optional — when not configured, scanned pages are skipped with a warning.

## Scope

**In scope:**

- Automatic OCR fallback when PDFium extracts empty/whitespace-only text from a PDF page
- Sidecar controls: `ocr.force`, `ocr.language_hints`, `ocr.timeout_secs`
- Global config: `tessdata_dir`, `ocr_timeout_secs`, `ocr_default_language`
- `TESSDATA_PREFIX` env var override for `tessdata_dir`
- Per-page OCR (only empty pages by default; all pages when forced)
- Subprocess timeout via `try_wait()` polling with explicit `child.kill()`

**Out of scope:**

- Standalone image file extraction (JPEG, PNG, TIFF as `FileType` variants)
- Heuristic empty-text detection (character count thresholds, page ratios)
- OCR quality tuning beyond language and DPI
- OCR controls on the HTTP API (`/ingest` endpoint) — `OcrOptions` is sidecar-local
- Configurable render DPI (hardcoded 300 DPI for MVP)

## Architecture

### Component: Extractor API (`crates/rag-core/src/extract.rs`)

Expand `FormatExtractor::extract()` to accept an options parameter:

```rust
#[derive(Debug, Clone, Default)]
pub struct ExtractionOptions {
    pub ocr: Option<OcrOptions>,
}

#[derive(Debug, Clone, Default)]
pub struct OcrOptions {
    pub force: bool,
    pub language_hints: Vec<String>,
    pub timeout_secs: Option<u64>,
}
```

- `ExtractionOptions::default()` has `ocr: None`.
- `TextExtractor` and `MarkdownExtractor` ignore the options parameter.
- `ExtractorRegistry::extract()` gains the same `options: &ExtractionOptions` parameter and passes it through.
- This is a breaking change to the internal trait — only 3 implementors, all in the same file.

### Component: Configuration (`crates/rag-core/src/config.rs` + `config/app.toml`)

New fields on the existing extract config:

```rust
pub tessdata_dir: Option<PathBuf>,   // None = OCR disabled
pub ocr_timeout_secs: u64,          // default: 30
pub ocr_default_language: String,    // default: "eng"
```

`tessdata_dir` is the only `Option` — it gates OCR availability. `ocr_timeout_secs` and `ocr_default_language` always have values (hardcoded defaults if not in config).

**`config/app.toml`:**

```toml
[extract]
# tessdata_dir = "/usr/local/share/tessdata"
# ocr_timeout_secs = 30
# ocr_default_language = "eng"
```

**Resolution (all happens in `AppConfig` loading, single source of truth):**

`tessdata_dir` precedence:
1. `TESSDATA_PREFIX` env var (highest)
2. `tessdata_dir` in `app.toml`
3. None → OCR disabled

After resolution, `AppConfig.tessdata_dir` holds the final value. Downstream consumers (`PdfExtractor`) never re-read env vars.

**Startup validation:** When `tessdata_dir` resolves to `Some(path)`, `PdfExtractor::new()` verifies that `{path}/{ocr_default_language}.traineddata` exists. Fail at startup if missing.

### Component: PdfExtractor (`crates/rag-core/src/extract.rs`)

**Struct expansion:**

```rust
pub struct PdfExtractor {
    library_path: PathBuf,
    tessdata_dir: Option<PathBuf>,
    ocr_timeout_secs: u64,
    ocr_default_language: String,
}
```

Constructor takes `&AppConfig` and consumes the already-resolved config values. `TESSDATA_PREFIX` env override and `tessdata_dir` resolution happen once in `AppConfig` loading, not here. Startup validation (language file check) runs during `PdfExtractor::new()`.

**Per-page extraction flow (inside existing `spawn_blocking`):**

```
// Pre-loop: resolve effective OCR settings once
let ocr_options = options.ocr.clone().unwrap_or_default()
let effective_language = if ocr_options.language_hints.is_empty() {
    self.ocr_default_language.clone()
} else {
    ocr_options.language_hints.join("+")   // "eng+deu"
}
let effective_timeout = ocr_options.timeout_secs.unwrap_or(self.ocr_timeout_secs)
let force_ocr = ocr_options.force
let ocr_available = self.tessdata_dir.is_some()

// Fail early if forced but unavailable
if force_ocr && !ocr_available {
    bail!("OCR forced but Tesseract not configured (tessdata_dir not set)")
}

let mut ocr_skipped_pages = 0u32;

for each page in pdf.pages():
    native_text = page.text().all()

    if force_ocr || native_text.trim().is_empty():
        if !ocr_available:
            ocr_skipped_pages += 1
            pages.push(native_text)
            continue

        bitmap = page.render_with_config(...)   // 300 DPI, RGB
        png_bytes = encode_bitmap_as_png(bitmap)
        ocr_text = run_tesseract(
            png_bytes, &effective_language, effective_timeout, &tessdata_dir
        )?
        pages.push(ocr_text)
    else:
        pages.push(native_text)

if ocr_skipped_pages > 0:
    tracing::warn!(
        skipped_pages = ocr_skipped_pages,
        "OCR fallback skipped for {ocr_skipped_pages} page(s) \
         because Tesseract is not configured"
    )

return pages.join("\u{000C}")
```

**`run_tesseract()` subprocess:**

- Spawn `tesseract stdin stdout --tessdata-dir {dir} -l {lang}`
- Write PNG to stdin, close stdin
- Read stdout/stderr on separate threads (avoid pipe deadlock)
- Poll `try_wait()` every 100ms against deadline
- On timeout: `child.kill()` + `child.wait()`, bail with timeout error
- On success: return stdout as String, trimmed

### Component: Sidecar Integration (`crates/rag-core/src/ingest.rs`)

The existing sidecar metadata structure (`.metadata.json`) gains an `ocr` object:

```json
{
  "ocr": {
    "force": true,
    "language_hints": ["eng", "deu"],
    "timeout_secs": 60
  }
}
```

`IngestService::ingest_file()` maps sidecar OCR fields → `OcrOptions` → `ExtractionOptions` before calling the extractor registry.

### Component: IngestService call site (`crates/rag-core/src/ingest.rs`)

The `extract_and_checksum()` helper (or equivalent call site) constructs `ExtractionOptions` from sidecar metadata and passes it to `registry.extract()`. Non-PDF files get `ExtractionOptions::default()`.

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Scanned page, OCR not configured | Warn once per document, skip OCR, return empty text for those pages |
| `ocr.force = true`, OCR not configured | Hard error — bail with clear message |
| Tesseract subprocess fails | Error propagated as extraction failure for that file |
| Tesseract subprocess times out | `child.kill()`, bail with timeout error including page number |
| Pipe read failure | Error propagated with context |
| PDFium bitmap render failure | Error propagated with context |
| `tessdata_dir` configured but language file missing | Startup error (fail-fast) |

## Testing

### Integration tests at PdfExtractor level

Require Tesseract installed — guarded by `which tesseract` check. Do NOT require Postgres or Qdrant.

- **OCR fallback on scanned page:** Extract a minimal image-only PDF fixture containing a known token (e.g., rendered text "TESSERACT"). Verify extracted output contains the expected token.
- **Forced OCR without Tesseract configured:** Construct `PdfExtractor` with `tessdata_dir = None`, extract with `ocr.force = true`. Verify hard error with "not configured" message.

### Unit tests (no external deps)

- **OCR settings resolution:** Empty `language_hints` → default language; `["eng", "deu"]` → `"eng+deu"`.
- **`ExtractionOptions::default()`** has `ocr: None`.
- **Config validation:** `tessdata_dir` set but missing `{ocr_default_language}.traineddata` → startup error.
- **OCR decision logic:** Factor the per-page decision into a testable function. Cases: `force=true` → OCR regardless of text; `force=false` + empty text → OCR; `force=false` + non-empty text → no OCR; any trigger + OCR unavailable → skip (increment counter).
- **Warning counter:** `ocr_skipped_pages` increments when OCR needed but unavailable; does not increment for native-text pages.

## Non-Goals

- OCR quality benchmarking
- Multi-engine OCR support (only Tesseract)
- HTTP API surface for OCR controls
- Configurable render DPI
- Standalone image file types
