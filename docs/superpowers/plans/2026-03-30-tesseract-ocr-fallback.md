# Tesseract OCR Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Tesseract OCR fallback to PdfExtractor so scanned PDF pages produce usable text, with sidecar controls for force/language/timeout.

**Architecture:** Expand `FormatExtractor::extract()` with `ExtractionOptions` parameter, add OCR config to AppConfig + sidecar, implement per-page OCR inside PdfExtractor's existing `spawn_blocking` closure using PDFium bitmap rendering + Tesseract subprocess.

**Tech Stack:** Rust, pdfium-render (with `image` feature), Tesseract CLI subprocess, tempfile

**Spec:** `docs/superpowers/specs/2026-03-30-tesseract-ocr-fallback-design.md`

---

### Task 1: ExtractionOptions types + FormatExtractor trait change

**Files:**
- Modify: `crates/rag-core/src/extract.rs`
- Modify: `crates/rag-core/src/ingest.rs`

- [ ] **Step 1: Write failing test for ExtractionOptions default**

In `crates/rag-core/src/extract.rs`, add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn extraction_options_default_has_no_ocr() {
    let opts = ExtractionOptions::default();
    assert!(opts.ocr.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core extraction_options_default_has_no_ocr`
Expected: FAIL — `ExtractionOptions` not found.

- [ ] **Step 3: Add ExtractionOptions and OcrOptions types**

In `crates/rag-core/src/extract.rs`, add after the `ExtractionResult` struct (before the `checksum` function):

```rust
/// Options that control extraction behavior beyond raw content.
///
/// Non-PDF extractors ignore these; `PdfExtractor` uses the `ocr` field
/// to decide whether and how to run Tesseract on scanned pages.
#[derive(Debug, Clone, Default)]
pub struct ExtractionOptions {
    pub ocr: Option<OcrOptions>,
}

/// Per-document OCR overrides, typically sourced from sidecar metadata.
#[derive(Debug, Clone, Default)]
pub struct OcrOptions {
    pub force: bool,
    pub language_hints: Vec<String>,
    pub timeout_secs: Option<u64>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core extraction_options_default_has_no_ocr`
Expected: PASS

- [ ] **Step 5: Change FormatExtractor trait signature**

In `crates/rag-core/src/extract.rs`, change the `FormatExtractor` trait:

```rust
/// Async-capable extractor interface for one or more file types.
pub trait FormatExtractor: Send + Sync {
    fn supported_types(&self) -> &'static [FileType];

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>>;
}
```

- [ ] **Step 6: Update TextExtractor and MarkdownExtractor**

Update `TextExtractor`:

```rust
impl FormatExtractor for TextExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Text]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "plain text") })
    }
}
```

Update `MarkdownExtractor` identically (add `_options: &'a ExtractionOptions` parameter, otherwise unchanged).

- [ ] **Step 7: Update PdfExtractor**

Update `PdfExtractor`'s `FormatExtractor` impl — add the `_options` parameter but don't use it yet:

```rust
impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        // ... existing body unchanged ...
    }
}
```

- [ ] **Step 8: Update ExtractorRegistry::extract**

```rust
/// Extract normalized text for the given file type.
pub async fn extract(
    &self,
    file_type: FileType,
    content: &[u8],
    options: &ExtractionOptions,
) -> Result<ExtractionResult> {
    let Some(index) = self.by_type.get(&file_type).copied() else {
        if file_type == FileType::Pdf {
            bail!(
                "no extractor registered for PDF files; \
                 set pdfium_library_path in config/app.toml \
                 or PDFIUM_LIBRARY_PATH env var to enable PDF support"
            );
        }
        bail!("no extractor registered for file type {file_type:?}");
    };

    self.extractors[index]
        .extract(content, options)
        .await
        .map_err(|err| {
            err.context(format!(
                "extracting content for file type {file_type:?}"
            ))
        })
}
```

- [ ] **Step 9: Update ingest.rs call site**

In `crates/rag-core/src/ingest.rs`, in the `extract_and_checksum` method, update the `self.extractors.extract()` call (around line 276):

```rust
let result = self
    .extractors
    .extract(file_type, &content, &extract::ExtractionOptions::default())
    .await
    .with_context(|| {
        format!("extracting text from {}", req.path.display())
    })?;
```

Add `extract` to the existing use statement if not already imported (currently `use crate::extract::{self, ExtractorRegistry, FileType};` — `ExtractionOptions` will be used via `extract::ExtractionOptions`).

- [ ] **Step 10: Update existing extract tests**

All existing tests in `extract.rs` that call `registry.extract(type, bytes)` now need the third parameter. Update each to pass `&ExtractionOptions::default()`:

```rust
// In text_registry_extracts_utf8_content:
let result = registry
    .extract(FileType::Text, b"hello world", &ExtractionOptions::default())
    .await
    .expect("text extraction should succeed");

// In markdown_registry_extracts_utf8_content:
let result = registry
    .extract(FileType::Markdown, b"# Title\n\nBody", &ExtractionOptions::default())
    .await
    .expect("markdown extraction should succeed");

// In registry_without_pdf_returns_config_hint:
let err = registry
    .extract(FileType::Pdf, b"%PDF-1.7", &ExtractionOptions::default())
    .await
    .expect_err("PDF extraction should fail when not configured");

// In invalid_utf8_errors_keep_decode_cause_visible:
let err = registry
    .extract(FileType::Text, &[0xff, 0xfe, 0xfd], &ExtractionOptions::default())
    .await
    .expect_err("invalid UTF-8 should fail");
```

- [ ] **Step 11: Verify everything compiles and tests pass**

Run: `cargo check -p rag-core`
Run: `cargo test -p rag-core`
Expected: All tests pass.

- [ ] **Step 12: Commit**

```bash
git add crates/rag-core/src/extract.rs crates/rag-core/src/ingest.rs
git commit -m "feat(extract): add ExtractionOptions parameter to FormatExtractor trait

Expands the extractor interface to accept per-document options.
Non-PDF extractors ignore the parameter; PdfExtractor will use
the ocr field for Tesseract fallback in a follow-up commit."
```

---

### Task 2: Configuration + PdfExtractor struct expansion

**Files:**
- Modify: `crates/rag-core/src/config.rs`
- Modify: `crates/rag-core/src/extract.rs`
- Modify: `config/app.toml`

- [ ] **Step 1: Write failing test for OCR config defaults**

In `crates/rag-core/src/config.rs`, add to the existing `from_env_defaults_and_override` test, after the `pdfium_library_path` assertion (around line 683):

```rust
// OCR defaults
assert!(
    cfg.tessdata_dir.is_none(),
    "tessdata_dir should default to None"
);
assert_eq!(cfg.ocr_timeout_secs, 30);
assert_eq!(cfg.ocr_default_language, "eng");
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core from_env_defaults_and_override`
Expected: FAIL — `tessdata_dir` field not found on `AppConfig`.

- [ ] **Step 3: Add OCR fields to ExtractSection and AppConfig**

In `crates/rag-core/src/config.rs`, update `ExtractSection`:

```rust
#[derive(Deserialize, Default)]
struct ExtractSection {
    pdfium_library_path: Option<String>,
    tessdata_dir: Option<String>,
    ocr_timeout_secs: Option<u64>,
    ocr_default_language: Option<String>,
}
```

Add fields to `AppConfig` struct, after `pdfium_library_path`:

```rust
    // Extraction
    pub pdfium_library_path: Option<std::path::PathBuf>,
    // OCR (Tesseract)
    pub tessdata_dir: Option<std::path::PathBuf>,
    pub ocr_timeout_secs: u64,
    pub ocr_default_language: String,
```

In `from_current_env()`, after the `pdfium_library_path` resolution (around line 518), add:

```rust
let tessdata_dir: Option<std::path::PathBuf> = env_string("TESSDATA_PREFIX")
    .or_else(|| extract_settings.tessdata_dir.clone())
    .map(std::path::PathBuf::from);

let ocr_timeout_secs = env_parsed("OCR_TIMEOUT_SECS")?
    .or(extract_settings.ocr_timeout_secs)
    .unwrap_or(30);

let ocr_default_language = env_string("OCR_DEFAULT_LANGUAGE")
    .or_else(|| extract_settings.ocr_default_language.clone())
    .unwrap_or_else(|| "eng".to_owned());
```

Add the new fields to the `Ok(Self { ... })` block:

```rust
    tessdata_dir,
    ocr_timeout_secs,
    ocr_default_language,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core from_env_defaults_and_override`
Expected: PASS

- [ ] **Step 5: Write failing test for TESSDATA_PREFIX env override**

In the same test function, add after the "Part 4: pdfium_library_path from env" section:

```rust
// -- Part 5: TESSDATA_PREFIX env override --
unsafe { std::env::set_var("TESSDATA_PREFIX", "/opt/tessdata") };
let cfg = AppConfig::from_current_env()
    .expect("from_env with TESSDATA_PREFIX");
assert_eq!(
    cfg.tessdata_dir.as_deref(),
    Some(std::path::Path::new("/opt/tessdata")),
    "TESSDATA_PREFIX env should set tessdata_dir"
);
unsafe { std::env::remove_var("TESSDATA_PREFIX") };
```

Also add `"TESSDATA_PREFIX"`, `"OCR_TIMEOUT_SECS"`, and `"OCR_DEFAULT_LANGUAGE"` to `clear_config_env()`.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p rag-core from_env_defaults_and_override`
Expected: PASS (this should already pass from step 3's implementation).

- [ ] **Step 7: Expand PdfExtractor struct and constructor**

In `crates/rag-core/src/extract.rs`, update the `PdfExtractor` struct:

```rust
#[derive(Debug)]
pub struct PdfExtractor {
    library_path: std::path::PathBuf,
    tessdata_dir: Option<std::path::PathBuf>,
    ocr_timeout_secs: u64,
    ocr_default_language: String,
}
```

Update the constructor:

```rust
impl PdfExtractor {
    /// Create a new `PdfExtractor` with a validated PDFium library path
    /// and optional OCR configuration.
    ///
    /// When `tessdata_dir` is `Some`, validates that the default language's
    /// `.traineddata` file exists. Eagerly loads PDFium to catch
    /// misconfiguration at startup.
    pub fn new(
        library_path: std::path::PathBuf,
        tessdata_dir: Option<std::path::PathBuf>,
        ocr_timeout_secs: u64,
        ocr_default_language: String,
    ) -> Result<Self> {
        // Validate PDFium library is loadable (existing logic).
        drop(pdfium_render::prelude::Pdfium::new(
            pdfium_render::prelude::Pdfium::bind_to_library(
                library_path.to_str().with_context(|| {
                    format!(
                        "pdfium_library_path is not valid UTF-8: {}",
                        library_path.display()
                    )
                })?,
            )
            .with_context(|| {
                format!(
                    "failed to load PDFium native library from {}: \
                     verify the path points to a valid PDFium binary \
                     (.dylib on macOS, .so on Linux, .dll on Windows)",
                    library_path.display()
                )
            })?,
        ));

        // Validate tessdata if configured.
        if let Some(ref dir) = tessdata_dir {
            let traineddata =
                dir.join(format!("{ocr_default_language}.traineddata"));
            if !traineddata.exists() {
                bail!(
                    "tessdata_dir {} does not contain \
                     {ocr_default_language}.traineddata; \
                     install the language pack or adjust \
                     ocr_default_language",
                    dir.display()
                );
            }
        }

        Ok(Self {
            library_path,
            tessdata_dir,
            ocr_timeout_secs,
            ocr_default_language,
        })
    }
}
```

- [ ] **Step 8: Update ExtractorRegistry::with_defaults**

```rust
pub fn with_defaults(config: &crate::config::AppConfig) -> Result<Self> {
    let mut extractors: Vec<Box<dyn FormatExtractor>> =
        vec![Box::new(MarkdownExtractor), Box::new(TextExtractor)];

    if let Some(ref path) = config.pdfium_library_path {
        let pdf = PdfExtractor::new(
            path.clone(),
            config.tessdata_dir.clone(),
            config.ocr_timeout_secs,
            config.ocr_default_language.clone(),
        )
        .context("configuring PDF extractor")?;
        extractors.push(Box::new(pdf));
    }

    Self::new(extractors)
}
```

- [ ] **Step 9: Update config/app.toml**

In `config/app.toml`, add after the existing `[extract]` section's `pdfium_library_path` comment:

```toml
# Tesseract OCR fallback for scanned PDF pages.
# Path to tessdata directory (contains *.traineddata files).
# Omit to disable OCR fallback. TESSDATA_PREFIX env var overrides this.
# tessdata_dir = "/usr/local/share/tessdata"
# Default timeout for Tesseract subprocess per page (seconds).
# ocr_timeout_secs = 30
# Default OCR language (Tesseract language code).
# ocr_default_language = "eng"
```

- [ ] **Step 10: Verify everything compiles and tests pass**

Run: `cargo check -p rag-core`
Run: `cargo test -p rag-core`
Expected: All tests pass.

- [ ] **Step 11: Commit**

```bash
git add crates/rag-core/src/config.rs crates/rag-core/src/extract.rs config/app.toml
git commit -m "feat(config): add OCR configuration fields and PdfExtractor expansion

Adds tessdata_dir, ocr_timeout_secs, ocr_default_language to AppConfig
with TESSDATA_PREFIX env override. PdfExtractor validates traineddata
file at startup when tessdata is configured."
```

---

### Task 3: Sidecar OCR config + ingest wiring + OCR decision logic

**Files:**
- Modify: `crates/rag-core/src/sidecar.rs`
- Modify: `crates/rag-core/src/ingest.rs`
- Modify: `crates/rag-core/src/extract.rs`

- [ ] **Step 1: Write failing test for sidecar OCR parsing**

In `crates/rag-core/src/sidecar.rs`, add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
#[allow(clippy::disallowed_methods)]
fn parses_sidecar_with_ocr_overrides() {
    let json = r#"{
        "schema_version": 1,
        "document": { "title": "T", "category": "c" },
        "source": { "url": "u", "domain": "d", "publisher": "p" },
        "language": "en",
        "tags": ["t"],
        "acl": { "allow_roles": ["*"] },
        "security": {
            "classification": "public",
            "requires_evidence_pack": false
        },
        "provenance": { "retrieved_at": "now", "retrieved_by": "me" },
        "ingestion": {
            "ocr": {
                "force": true,
                "language_hints": ["eng", "deu"],
                "timeout_secs": 60
            }
        }
    }"#;
    let sc = Sidecar::from_json(json.as_bytes())
        .expect("should parse with OCR overrides");
    let ocr = sc
        .ingestion
        .expect("ingestion should be Some")
        .ocr
        .expect("ocr should be Some");
    assert!(ocr.force);
    assert_eq!(ocr.language_hints, vec!["eng", "deu"]);
    assert_eq!(ocr.timeout_secs, Some(60));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rag-core parses_sidecar_with_ocr_overrides`
Expected: FAIL — `ocr` field not found on `IngestionConfig`.

- [ ] **Step 3: Add OcrConfig to sidecar**

In `crates/rag-core/src/sidecar.rs`, add the struct after `ChunkingOverride`:

```rust
/// Per-document OCR overrides carried in sidecar metadata.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OcrConfig {
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub language_hints: Vec<String>,
    pub timeout_secs: Option<u64>,
}
```

Add the `ocr` field to `IngestionConfig`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestionConfig {
    pub collection: Option<String>,
    pub chunking: Option<ChunkingOverride>,
    pub ocr: Option<OcrConfig>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rag-core parses_sidecar_with_ocr_overrides`
Expected: PASS

- [ ] **Step 5: Wire sidecar OCR into ExtractionOptions in ingest.rs**

In `crates/rag-core/src/ingest.rs`, in `extract_and_checksum`, replace the `self.extractors.extract(...)` call with one that constructs `ExtractionOptions` from the sidecar:

```rust
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
    .with_context(|| {
        format!("extracting text from {}", req.path.display())
    })?;
```

- [ ] **Step 6: Write failing tests for OCR decision logic**

In `crates/rag-core/src/extract.rs`, add to the test module:

```rust
#[test]
fn page_needs_ocr_when_forced() {
    assert!(page_needs_ocr(true, "has native text"));
}

#[test]
fn page_needs_ocr_when_text_empty() {
    assert!(page_needs_ocr(false, ""));
    assert!(page_needs_ocr(false, "   "));
    assert!(page_needs_ocr(false, "\n\t "));
}

#[test]
fn page_does_not_need_ocr_for_native_text() {
    assert!(!page_needs_ocr(false, "some content"));
}

#[test]
fn resolve_ocr_settings_uses_defaults() {
    let resolved = resolve_ocr_settings(None, "eng", 30);
    assert!(!resolved.force);
    assert_eq!(resolved.language, "eng");
    assert_eq!(resolved.timeout.as_secs(), 30);
}

#[test]
fn resolve_ocr_settings_joins_language_hints() {
    let opts = OcrOptions {
        force: true,
        language_hints: vec!["eng".into(), "deu".into()],
        timeout_secs: Some(60),
    };
    let resolved = resolve_ocr_settings(Some(&opts), "fra", 30);
    assert!(resolved.force);
    assert_eq!(resolved.language, "eng+deu");
    assert_eq!(resolved.timeout.as_secs(), 60);
}

#[test]
fn resolve_ocr_settings_empty_hints_uses_default_language() {
    let opts = OcrOptions {
        force: false,
        language_hints: vec![],
        timeout_secs: None,
    };
    let resolved = resolve_ocr_settings(Some(&opts), "fra", 45);
    assert_eq!(resolved.language, "fra");
    assert_eq!(resolved.timeout.as_secs(), 45);
}
```

- [ ] **Step 7: Run tests to verify they fail**

Run: `cargo test -p rag-core page_needs_ocr`
Run: `cargo test -p rag-core resolve_ocr_settings`
Expected: FAIL — functions not found.

- [ ] **Step 8: Implement page_needs_ocr and resolve_ocr_settings**

In `crates/rag-core/src/extract.rs`, add before the `#[cfg(test)]` block:

```rust
// ---------------------------------------------------------------------------
// OCR decision helpers
// ---------------------------------------------------------------------------

/// Determine whether a page needs OCR processing.
fn page_needs_ocr(force: bool, native_text: &str) -> bool {
    force || native_text.trim().is_empty()
}

/// Pre-loop OCR settings resolved from sidecar options + config defaults.
struct ResolvedOcrSettings {
    force: bool,
    language: String,
    timeout: std::time::Duration,
}

/// Resolve effective OCR settings once before the page loop.
fn resolve_ocr_settings(
    options: Option<&OcrOptions>,
    default_language: &str,
    default_timeout_secs: u64,
) -> ResolvedOcrSettings {
    let opts = options.cloned().unwrap_or_default();

    let language = if opts.language_hints.is_empty() {
        default_language.to_string()
    } else {
        opts.language_hints.join("+")
    };

    let force = opts.force;
    let timeout_override = opts.timeout_secs;

    let timeout = std::time::Duration::from_secs(
        timeout_override.unwrap_or(default_timeout_secs),
    );

    ResolvedOcrSettings { force, language, timeout }
}
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p rag-core page_needs_ocr`
Run: `cargo test -p rag-core resolve_ocr_settings`
Expected: PASS

- [ ] **Step 10: Verify everything compiles and tests pass**

Run: `cargo check -p rag-core`
Run: `cargo test -p rag-core`
Expected: All tests pass.

- [ ] **Step 11: Commit**

```bash
git add crates/rag-core/src/sidecar.rs crates/rag-core/src/ingest.rs crates/rag-core/src/extract.rs
git commit -m "feat(ocr): add sidecar OcrConfig, ingest wiring, and decision logic

Sidecar metadata now accepts an optional ocr block with force,
language_hints, and timeout_secs. IngestService maps these to
ExtractionOptions. Decision helpers (page_needs_ocr,
resolve_ocr_settings) factor out testable per-page OCR logic."
```

---

### Task 4: Tesseract subprocess + page rendering + PdfExtractor integration

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/rag-core/Cargo.toml`
- Modify: `crates/rag-core/src/extract.rs`
- Create: `crates/rag-core/tests/fixtures/hello_ocr.png`
- Create: `crates/rag-core/tests/fixtures/hello_ocr.pdf`

- [ ] **Step 1: Add image dependency**

In workspace `Cargo.toml`, update `pdfium-render` and add `image`:

```toml
image = "0.24"
pdfium-render = { version = "0.8", features = ["image"] }
```

In `crates/rag-core/Cargo.toml`, add `image` to `[dependencies]` (tempfile stays as dev-dep only):

```toml
[dependencies]
# ... existing deps ...
image.workspace = true
```

- [ ] **Step 2: Implement render_page_to_png function**

In `crates/rag-core/src/extract.rs`, add after `resolve_ocr_settings`:

```rust
/// DPI used for rendering PDF pages to bitmaps before OCR.
const OCR_RENDER_DPI: f32 = 300.0;

/// Render a PDF page to in-memory PNG bytes for OCR processing.
///
/// Uses PDFium's bitmap rendering at 300 DPI, then encodes to PNG
/// via the `image` crate.
///
/// Must be called from a blocking context (inside `spawn_blocking`).
fn render_page_to_png(
    page: &pdfium_render::prelude::PdfPage<'_>,
) -> Result<Vec<u8>> {
    let scale = OCR_RENDER_DPI / 72.0;
    let width = (page.width().value * scale) as i32;
    let height = (page.height().value * scale) as i32;

    let config = pdfium_render::prelude::PdfRenderConfig::new()
        .set_target_width(width)
        .set_maximum_height(height);

    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("rendering PDF page to bitmap: {e}"))?;

    let image = bitmap.as_image();
    let mut cursor = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .context("encoding page bitmap as PNG")?;

    Ok(cursor.into_inner())
}
```

- [ ] **Step 3: Implement run_tesseract function (stdin-based)**

Add after `render_page_to_png`:

```rust
/// Run Tesseract OCR on PNG image bytes via stdin and return text.
///
/// Pipes `png_bytes` to `tesseract stdin stdout`, reads extracted text
/// from stdout. Uses deadline-based timeout with `try_wait()` polling
/// and explicit `child.kill()` — `tokio::time::timeout` does NOT kill
/// child processes.
///
/// Must be called from a blocking context (inside `spawn_blocking`).
fn run_tesseract(
    png_bytes: &[u8],
    language: &str,
    timeout: std::time::Duration,
    tessdata_dir: &std::path::Path,
) -> Result<String> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("tesseract");
    cmd.arg("stdin")
        .arg("stdout")
        .arg("-l")
        .arg(language)
        .arg("--tessdata-dir")
        .arg(tessdata_dir);
    cmd.env("TESSDATA_PREFIX", tessdata_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .context("spawning tesseract subprocess")?;

    // Take all three pipes before spawning threads.
    let mut stdin_pipe = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("missing tesseract stdin pipe"))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("missing tesseract stdout pipe"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("missing tesseract stderr pipe"))?;

    // Write PNG to stdin on a separate thread to avoid deadlock.
    let owned_bytes = png_bytes.to_vec();
    let stdin_thread = std::thread::spawn(move || {
        let result =
            std::io::Write::write_all(&mut stdin_pipe, &owned_bytes);
        drop(stdin_pipe); // Close stdin to signal EOF.
        result
    });

    // Read stdout/stderr on separate threads.
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stdout_pipe, &mut buf)
            .map(|_| buf)
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stderr_pipe, &mut buf)
            .map(|_| buf)
    });

    // Poll with try_wait() — tokio::time::timeout does NOT kill
    // child processes.
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .context("waiting for tesseract")?
        {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                child.kill().ok();
                child.wait().ok();
                let _ = stdin_thread.join();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!(
                    "tesseract timed out (exceeded {} ms)",
                    timeout.as_millis()
                );
            }
            None => {
                std::thread::sleep(
                    std::time::Duration::from_millis(100),
                );
            }
        }
    };

    stdin_thread
        .join()
        .map_err(|_| anyhow!("tesseract stdin writer panicked"))?
        .context("writing PNG to tesseract stdin")?;

    let stdout_bytes = stdout_thread
        .join()
        .map_err(|_| anyhow!("tesseract stdout reader panicked"))?
        .context("reading tesseract stdout")?;
    let stderr_bytes = stderr_thread
        .join()
        .map_err(|_| anyhow!("tesseract stderr reader panicked"))?
        .context("reading tesseract stderr")?;

    if !status.success() {
        bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&stderr_bytes)
        );
    }

    let text = String::from_utf8(stdout_bytes)
        .context("tesseract output is not valid UTF-8")?;
    Ok(text.trim().to_string())
}
```

- [ ] **Step 4: Integrate per-page OCR into PdfExtractor::extract**

Replace the `PdfExtractor`'s `FormatExtractor::extract` implementation:

```rust
impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        let owned_bytes = content.to_vec();
        let lib_path = self.library_path.clone();
        let tessdata_dir = self.tessdata_dir.clone();
        let default_language = self.ocr_default_language.clone();
        let default_timeout = self.ocr_timeout_secs;
        let ocr_options = options.ocr.clone();

        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let pdfium = pdfium_render::prelude::Pdfium::new(
                    pdfium_render::prelude::Pdfium::bind_to_library(
                        lib_path.to_str().with_context(|| {
                            format!(
                                "pdfium_library_path is not valid \
                                 UTF-8: {}",
                                lib_path.display()
                            )
                        })?,
                    )
                    .with_context(|| {
                        format!(
                            "binding PDFium library from {}",
                            lib_path.display()
                        )
                    })?,
                );

                let doc = pdfium
                    .load_pdf_from_byte_vec(owned_bytes, None)
                    .map_err(|e| {
                        anyhow!("loading PDF document: {e}")
                    })?;

                // Resolve OCR settings once before page loop.
                let settings = resolve_ocr_settings(
                    ocr_options.as_ref(),
                    &default_language,
                    default_timeout,
                );
                let ocr_available = tessdata_dir.is_some();

                // Fail early if forced but unavailable.
                if settings.force && !ocr_available {
                    bail!(
                        "OCR forced but Tesseract not configured \
                         (tessdata_dir not set)"
                    );
                }

                let mut pages = Vec::new();
                let mut ocr_skipped_pages: u32 = 0;

                for page in doc.pages().iter() {
                    let native_text = page
                        .text()
                        .map_err(|e| {
                            anyhow!(
                                "extracting text from PDF page: {e}"
                            )
                        })?
                        .all();

                    if page_needs_ocr(
                        settings.force,
                        &native_text,
                    ) {
                        if !ocr_available {
                            ocr_skipped_pages += 1;
                            pages.push(native_text);
                            continue;
                        }

                        let tessdata = tessdata_dir
                            .as_ref()
                            .ok_or_else(|| {
                                anyhow!("tessdata_dir is None")
                            })?;

                        let png_bytes =
                            render_page_to_png(&page)?;

                        let ocr_text = run_tesseract(
                            &png_bytes,
                            &settings.language,
                            settings.timeout,
                            tessdata,
                        )?;

                        pages.push(ocr_text);
                    } else {
                        pages.push(native_text);
                    }
                }

                if ocr_skipped_pages > 0 {
                    tracing::warn!(
                        skipped_pages = ocr_skipped_pages,
                        "OCR fallback skipped for \
                         {ocr_skipped_pages} page(s) because \
                         Tesseract is not configured"
                    );
                }

                Ok(ExtractionResult {
                    text: pages.join("\u{000C}"),
                })
            })
            .await
            .context("PDF extraction task panicked")?
        })
    }
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p rag-core`
Expected: Compiles without errors. If `pdfium-render` API details differ (e.g., `page.width().value` vs `page.width().0` or render config method names), fix based on compiler errors.

- [ ] **Step 6: Run existing tests**

Run: `cargo test -p rag-core`
Expected: All existing tests pass. OCR code paths are not exercised by existing tests (no PDFium in CI, no Tesseract).

- [ ] **Step 7: Create test fixtures for integration tests**

Create test fixtures: a PNG image with recognizable text, and a scanned PDF (image-only, no text layer) containing the same image. Run these commands (requires ImageMagick and img2pdf):

```bash
mkdir -p crates/rag-core/tests/fixtures

# PNG with known text for run_tesseract unit test
convert -size 400x100 xc:white \
    -font Helvetica -pointsize 48 -fill black \
    -gravity center -annotate +0+0 'HELLO OCR' \
    crates/rag-core/tests/fixtures/hello_ocr.png

# Scanned PDF: image-only (no text layer) for PdfExtractor test
img2pdf crates/rag-core/tests/fixtures/hello_ocr.png \
    -o crates/rag-core/tests/fixtures/hello_ocr.pdf
```

If `img2pdf` is unavailable, use `convert hello_ocr.png hello_ocr.pdf` instead. The critical property is that PDFium's `page.text().all()` returns empty string on this PDF — it has no embedded text, only a raster image.

- [ ] **Step 8: Write test helper for finding tessdata**

In `crates/rag-core/src/extract.rs`, add to the test module:

```rust
/// Find a usable tessdata directory from common system locations.
#[cfg(test)]
fn find_tessdata_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("TESSDATA_PREFIX") {
        let path = std::path::PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    for dir in &[
        "/opt/homebrew/share/tessdata",
        "/usr/local/share/tessdata",
        "/usr/share/tesseract-ocr/5/tessdata",
        "/usr/share/tesseract-ocr/4.00/tessdata",
        "/usr/share/tessdata",
    ] {
        let path = std::path::PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Check whether tesseract is installed and tessdata is available.
/// Returns (tessdata_dir) or None if either is missing.
#[cfg(test)]
fn tesseract_available() -> Option<std::path::PathBuf> {
    if std::process::Command::new("tesseract")
        .arg("--version")
        .output()
        .is_err()
    {
        return None;
    }
    find_tessdata_dir()
}
```

- [ ] **Step 9: Write run_tesseract integration test (subprocess only)**

```rust
#[test]
#[ignore] // requires tesseract installed
#[allow(clippy::disallowed_methods)]
fn run_tesseract_extracts_text_from_png_bytes() {
    let tessdata = match tesseract_available() {
        Some(dir) => dir,
        None => {
            eprintln!("skipping: tesseract or tessdata not found");
            return;
        }
    };

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hello_ocr.png");
    if !fixture.exists() {
        eprintln!(
            "skipping: fixture not found at {}",
            fixture.display()
        );
        return;
    }

    let png_bytes = std::fs::read(&fixture)
        .expect("reading PNG fixture");

    let text = run_tesseract(
        &png_bytes,
        "eng",
        std::time::Duration::from_secs(30),
        &tessdata,
    )
    .expect("tesseract should succeed on fixture");

    assert!(
        text.to_uppercase().contains("HELLO"),
        "OCR output should contain 'HELLO', got: {text:?}"
    );
}
```

- [ ] **Step 10: Write PdfExtractor-level integration test (primary)**

This is the approved spec test — exercises the full path: PDFium page text empty → render to PNG → Tesseract OCR → extracted text contains known token.

```rust
#[tokio::test]
#[ignore] // requires PDFium + Tesseract installed
#[allow(clippy::disallowed_methods)]
async fn pdf_extractor_ocr_fallback_on_scanned_page() {
    let tessdata = match tesseract_available() {
        Some(dir) => dir,
        None => {
            eprintln!(
                "skipping: tesseract or tessdata not found"
            );
            return;
        }
    };

    // Locate PDFium library.
    let pdfium_path = std::env::var("PDFIUM_LIBRARY_PATH")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists());
    let pdfium_path = match pdfium_path {
        Some(p) => p,
        None => {
            eprintln!(
                "skipping: PDFIUM_LIBRARY_PATH not set or \
                 not found"
            );
            return;
        }
    };

    let extractor = PdfExtractor::new(
        pdfium_path,
        Some(tessdata.clone()),
        30,
        "eng".to_string(),
    )
    .expect("PdfExtractor should construct");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hello_ocr.pdf");
    if !fixture.exists() {
        eprintln!(
            "skipping: PDF fixture not found at {}",
            fixture.display()
        );
        return;
    }

    let pdf_bytes = std::fs::read(&fixture)
        .expect("reading PDF fixture");

    // Default options — OCR triggers automatically on empty text.
    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("extraction should succeed with OCR fallback");

    assert!(
        result.text.to_uppercase().contains("HELLO"),
        "OCR fallback output should contain 'HELLO', got: \
         {:?}",
        result.text
    );
}

#[tokio::test]
#[ignore] // requires PDFium installed (no Tesseract needed)
#[allow(clippy::disallowed_methods)]
async fn pdf_extractor_forced_ocr_without_tessdata_errors() {
    let pdfium_path = std::env::var("PDFIUM_LIBRARY_PATH")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists());
    let pdfium_path = match pdfium_path {
        Some(p) => p,
        None => {
            eprintln!(
                "skipping: PDFIUM_LIBRARY_PATH not set or \
                 not found"
            );
            return;
        }
    };

    let extractor = PdfExtractor::new(
        pdfium_path,
        None, // no tessdata
        30,
        "eng".to_string(),
    )
    .expect("PdfExtractor should construct without tessdata");

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hello_ocr.pdf");
    if !fixture.exists() {
        eprintln!(
            "skipping: PDF fixture not found at {}",
            fixture.display()
        );
        return;
    }

    let pdf_bytes = std::fs::read(&fixture)
        .expect("reading PDF fixture");

    let options = ExtractionOptions {
        ocr: Some(OcrOptions {
            force: true,
            language_hints: vec![],
            timeout_secs: None,
        }),
    };

    let err = extractor
        .extract(&pdf_bytes, &options)
        .await
        .expect_err("forced OCR without tessdata should fail");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("not configured"),
        "error should mention OCR not configured, got: {msg}"
    );
}
```

- [ ] **Step 11: Run integration tests (locally only)**

Run: `cargo test -p rag-core pdf_extractor_ocr -- --ignored`
Run: `cargo test -p rag-core run_tesseract_extracts -- --ignored`
Expected: PASS if PDFium + Tesseract installed and fixtures present; skipped otherwise.

- [ ] **Step 12: Verify full test suite**

Run: `cargo check -p rag-core`
Run: `cargo test -p rag-core`
Expected: All non-ignored tests pass.

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml crates/rag-core/Cargo.toml crates/rag-core/src/extract.rs crates/rag-core/tests/fixtures/
git commit -m "feat(ocr): Tesseract subprocess + per-page OCR in PdfExtractor

Renders scanned pages to in-memory PNG via PDFium bitmap API, pipes
to tesseract stdin with try_wait() deadline timeout. Integration tests
at PdfExtractor level (ignored, require PDFium + Tesseract)."
```
