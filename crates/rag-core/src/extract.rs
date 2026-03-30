//! Document format extraction primitives.
//!
//! The extractor interface is async from the start so OCR- or subprocess-based
//! extractors can be added later without a breaking trait change.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};

/// File formats currently recognized by the extraction layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    Pdf,
    Markdown,
    Text,
}

impl FileType {
    /// Infer a supported file type from a filename extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Normalized text extracted from a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionResult {
    pub text: String,
}

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

/// Compute a SHA-256 checksum over extracted text.
pub fn checksum(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Async-capable extractor interface for one or more file types.
pub trait FormatExtractor: Send + Sync {
    fn supported_types(&self) -> &'static [FileType];

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>>;
}

/// Registry that dispatches extraction requests by file type.
pub struct ExtractorRegistry {
    extractors: Vec<Box<dyn FormatExtractor>>,
    by_type: HashMap<FileType, usize>,
}

impl ExtractorRegistry {
    /// Build a registry from a concrete set of extractors.
    pub fn new(extractors: Vec<Box<dyn FormatExtractor>>) -> Result<Self> {
        let mut by_type = HashMap::new();

        for (index, extractor) in extractors.iter().enumerate() {
            for file_type in extractor.supported_types() {
                if by_type.insert(*file_type, index).is_some() {
                    bail!("duplicate extractor registration for file type {file_type:?}");
                }
            }
        }

        Ok(Self { extractors, by_type })
    }

    /// Construct the default registry, optionally including PDF support
    /// when `config.pdfium_library_path` is set.
    pub fn with_defaults(config: &crate::config::AppConfig) -> Result<Self> {
        let mut extractors: Vec<Box<dyn FormatExtractor>> =
            vec![Box::new(MarkdownExtractor), Box::new(TextExtractor)];

        if config.pdfium_library_path.is_some() {
            let pdf = PdfExtractor::new(config).context("configuring PDF extractor")?;
            extractors.push(Box::new(pdf));
        }

        Self::new(extractors)
    }

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
            .map_err(|err| err.context(format!("extracting content for file type {file_type:?}")))
    }
}

#[derive(Debug, Default)]
pub struct TextExtractor;

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

#[derive(Debug, Default)]
pub struct MarkdownExtractor;

impl FormatExtractor for MarkdownExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Markdown]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "markdown") })
    }
}

/// Extracts text from PDF files using the PDFium native library.
///
/// Stores the library path and creates a fresh PDFium binding on each
/// `extract()` call inside `spawn_blocking`. This avoids `Send + Sync`
/// issues with the C library handle.
#[derive(Debug)]
pub struct PdfExtractor {
    library_path: std::path::PathBuf,
    tessdata_dir: Option<std::path::PathBuf>,
    ocr_timeout_secs: u64,
    ocr_default_language: String,
}

impl PdfExtractor {
    /// Create a new `PdfExtractor` from application config.
    ///
    /// Consumes already-resolved config values -- `TESSDATA_PREFIX` env
    /// override happens in `AppConfig` loading, not here. Eagerly loads
    /// PDFium and validates tessdata (if configured) at startup.
    pub fn new(config: &crate::config::AppConfig) -> Result<Self> {
        let library_path = config
            .pdfium_library_path
            .clone()
            .ok_or_else(|| anyhow!("pdfium_library_path is required for PdfExtractor"))?;

        // Validate PDFium library is loadable at construction time.
        // Use bind_to_library() with the exact configured path — do NOT
        // use pdfium_platform_library_name_at_path(), which infers a
        // platform-default filename from a directory and would ignore
        // custom filenames, symlinks, or nonstandard install locations.
        drop(pdfium_render::prelude::Pdfium::new(
            pdfium_render::prelude::Pdfium::bind_to_library(library_path.to_str().with_context(
                || format!("pdfium_library_path is not valid UTF-8: {}", library_path.display()),
            )?)
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
        if let Some(ref dir) = config.tessdata_dir {
            let lang = &config.ocr_default_language;
            let traineddata = dir.join(format!("{lang}.traineddata"));
            if !traineddata.exists() {
                bail!(
                    "tessdata_dir {} does not contain {lang}.traineddata; \
                     install the language pack or adjust ocr_default_language",
                    dir.display()
                );
            }
        }

        Ok(Self {
            library_path,
            tessdata_dir: config.tessdata_dir.clone(),
            ocr_timeout_secs: config.ocr_timeout_secs,
            ocr_default_language: config.ocr_default_language.clone(),
        })
    }
}

impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    // tracing::warn! internally uses .expect()
    #[allow(clippy::disallowed_methods)]
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
                                "pdfium_library_path is not valid UTF-8: {}",
                                lib_path.display()
                            )
                        })?,
                    )
                    .with_context(|| {
                        format!("binding PDFium library from {}", lib_path.display())
                    })?,
                );

                let doc = pdfium
                    .load_pdf_from_byte_vec(owned_bytes, None)
                    .map_err(|e| anyhow!("loading PDF document: {e}"))?;

                // Resolve OCR settings once before page loop.
                let settings =
                    resolve_ocr_settings(ocr_options.as_ref(), &default_language, default_timeout);
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
                        .map_err(|e| anyhow!("extracting text from PDF page: {e}"))?
                        .all();

                    match page_ocr_decision(settings.force, &native_text, ocr_available) {
                        PageOcrDecision::UseNativeText => {
                            pages.push(native_text);
                        }
                        PageOcrDecision::SkipUnavailable => {
                            ocr_skipped_pages += 1;
                            pages.push(native_text);
                        }
                        PageOcrDecision::PerformOcr => {
                            let tessdata = tessdata_dir
                                .as_ref()
                                .ok_or_else(|| anyhow!("tessdata_dir is None"))?;

                            let png_bytes = render_page_to_png(&page)?;

                            let ocr_text = run_tesseract(
                                &png_bytes,
                                &settings.language,
                                settings.timeout,
                                tessdata,
                            )?;

                            pages.push(ocr_text);
                        }
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

                Ok(ExtractionResult { text: pages.join("\u{000C}") })
            })
            .await
            .context("PDF extraction task panicked")?
        })
    }
}

fn extract_utf8_passthrough(content: &[u8], format_name: &str) -> Result<ExtractionResult> {
    let text = std::str::from_utf8(content)
        .with_context(|| format!("decoding {format_name} content as UTF-8"))?
        .to_owned();

    Ok(ExtractionResult { text })
}

// ---------------------------------------------------------------------------
// OCR decision helpers
// ---------------------------------------------------------------------------

/// Outcome of the per-page OCR decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageOcrDecision {
    /// Page has native text and force is off -- use PDFium output.
    UseNativeText,
    /// Page needs OCR and Tesseract is available -- render + OCR.
    PerformOcr,
    /// Page needs OCR but Tesseract is unavailable -- increment
    /// skip counter and use native (empty) text.
    SkipUnavailable,
}

/// Determine the OCR outcome for a single page.
///
/// Note: the `force + !ocr_available` hard-error case is handled
/// before the page loop (early bail). This function is only called
/// when that combination is impossible, so `SkipUnavailable` only
/// arises for automatic (non-forced) OCR triggers.
fn page_ocr_decision(force: bool, native_text: &str, ocr_available: bool) -> PageOcrDecision {
    let needs_ocr = force || native_text.trim().is_empty();
    if !needs_ocr {
        PageOcrDecision::UseNativeText
    } else if ocr_available {
        PageOcrDecision::PerformOcr
    } else {
        PageOcrDecision::SkipUnavailable
    }
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

    let timeout = std::time::Duration::from_secs(timeout_override.unwrap_or(default_timeout_secs));

    ResolvedOcrSettings { force, language, timeout }
}

/// DPI used for rendering PDF pages to bitmaps before OCR.
const OCR_RENDER_DPI: f32 = 300.0;

/// Render a PDF page to in-memory PNG bytes for OCR processing.
///
/// Uses PDFium's bitmap rendering at 300 DPI, then encodes to PNG
/// via the `image` crate.
///
/// Must be called from a blocking context (inside `spawn_blocking`).
fn render_page_to_png(page: &pdfium_render::prelude::PdfPage<'_>) -> Result<Vec<u8>> {
    let scale = OCR_RENDER_DPI / 72.0;
    let width_f = page.width().value * scale;
    let height_f = page.height().value * scale;

    if width_f < 1.0 || height_f < 1.0 || width_f > i32::MAX as f32 || height_f > i32::MAX as f32 {
        bail!(
            "PDF page dimensions out of range for OCR rendering \
             ({width_f:.0} x {height_f:.0} px at {OCR_RENDER_DPI} DPI)"
        );
    }

    let width = width_f as i32;
    let height = height_f as i32;

    let config = pdfium_render::prelude::PdfRenderConfig::new()
        .set_target_width(width)
        .set_maximum_height(height);

    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("rendering PDF page to bitmap: {e}"))?;

    let image = bitmap.as_image();
    let mut cursor = std::io::Cursor::new(Vec::new());
    image.write_to(&mut cursor, image::ImageFormat::Png).context("encoding page bitmap as PNG")?;

    Ok(cursor.into_inner())
}

/// Run Tesseract OCR on PNG image bytes via stdin and return text.
///
/// Pipes `png_bytes` to `tesseract stdin stdout`, reads extracted text
/// from stdout. Uses deadline-based timeout with `try_wait()` polling
/// and explicit `child.kill()` -- `tokio::time::timeout` does NOT kill
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
    cmd.arg("stdin").arg("stdout").arg("-l").arg(language).arg("--tessdata-dir").arg(tessdata_dir);
    cmd.env("TESSDATA_PREFIX", tessdata_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning tesseract subprocess")?;

    // Take all three pipes before spawning threads.
    let mut stdin_pipe =
        child.stdin.take().ok_or_else(|| anyhow!("missing tesseract stdin pipe"))?;
    let mut stdout_pipe =
        child.stdout.take().ok_or_else(|| anyhow!("missing tesseract stdout pipe"))?;
    let mut stderr_pipe =
        child.stderr.take().ok_or_else(|| anyhow!("missing tesseract stderr pipe"))?;

    // Write PNG to stdin on a separate thread to avoid deadlock.
    let owned_bytes = png_bytes.to_vec();
    let stdin_thread = std::thread::spawn(move || {
        let result = std::io::Write::write_all(&mut stdin_pipe, &owned_bytes);
        drop(stdin_pipe); // Close stdin to signal EOF.
        result
    });

    // Read stdout/stderr on separate threads.
    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stdout_pipe, &mut buf).map(|_| buf)
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut stderr_pipe, &mut buf).map(|_| buf)
    });

    // Poll with try_wait() -- tokio::time::timeout does NOT kill
    // child processes.
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().context("waiting for tesseract")? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                child.kill().ok();
                child.wait().ok();
                let _ = stdin_thread.join();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!("tesseract timed out (exceeded {} ms)", timeout.as_millis());
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(100));
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
        bail!("tesseract failed: {}", String::from_utf8_lossy(&stderr_bytes));
    }

    let text = String::from_utf8(stdout_bytes).context("tesseract output is not valid UTF-8")?;
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a test registry with text + markdown extractors (no PDF — no native lib in CI).
    #[allow(clippy::disallowed_methods)] // test helper
    fn test_registry() -> ExtractorRegistry {
        ExtractorRegistry::new(vec![Box::new(MarkdownExtractor), Box::new(TextExtractor)])
            .expect("test registry should build")
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn text_registry_extracts_utf8_content() {
        let registry = test_registry();

        let result = registry
            .extract(FileType::Text, b"hello world", &ExtractionOptions::default())
            .await
            .expect("text extraction should succeed");

        assert_eq!(result.text, "hello world");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn markdown_registry_extracts_utf8_content() {
        let registry = test_registry();

        let result = registry
            .extract(FileType::Markdown, b"# Title\n\nBody", &ExtractionOptions::default())
            .await
            .expect("markdown extraction should succeed");

        assert_eq!(result.text, "# Title\n\nBody");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn registry_rejects_duplicate_file_type_registration() {
        struct DuplicateTextExtractor;

        impl FormatExtractor for DuplicateTextExtractor {
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

        let err = match ExtractorRegistry::new(vec![
            Box::new(TextExtractor),
            Box::new(DuplicateTextExtractor),
        ]) {
            Ok(_) => panic!("duplicate file type registration should fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("duplicate extractor registration"),
            "expected duplicate registration error, got {err}"
        );
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn registry_without_pdf_returns_config_hint() {
        // No PDF extractor — simulates the unconfigured case.
        let registry = test_registry();

        let err = registry
            .extract(FileType::Pdf, b"%PDF-1.7", &ExtractionOptions::default())
            .await
            .expect_err("PDF extraction should fail when not configured");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("pdfium_library_path"),
            "error should name the missing config option, got: {msg}"
        );
        assert!(msg.contains("PDFIUM_LIBRARY_PATH"), "error should name the env var, got: {msg}");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn invalid_utf8_errors_keep_decode_cause_visible() {
        let registry = test_registry();

        let err = registry
            .extract(FileType::Text, &[0xff, 0xfe, 0xfd], &ExtractionOptions::default())
            .await
            .expect_err("invalid UTF-8 should fail");

        assert!(
            format!("{err:#}").contains("decoding plain text content as UTF-8"),
            "expected UTF-8 decode cause in error chain, got {err:#}"
        );
        assert!(
            err.chain().any(|cause| cause.is::<std::str::Utf8Error>()),
            "expected underlying Utf8Error to remain in the chain, got {err:#}"
        );
    }

    #[test]
    fn extraction_options_default_has_no_ocr() {
        let opts = ExtractionOptions::default();
        assert!(opts.ocr.is_none());
    }

    #[test]
    fn checksum_is_stable_for_extracted_text() {
        assert_eq!(
            checksum("hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // --- page_ocr_decision tests ---

    #[test]
    fn page_ocr_decision_native_text_no_force() {
        assert_eq!(page_ocr_decision(false, "content", true), PageOcrDecision::UseNativeText,);
        assert_eq!(page_ocr_decision(false, "content", false), PageOcrDecision::UseNativeText,);
    }

    #[test]
    fn page_ocr_decision_empty_text_ocr_available() {
        assert_eq!(page_ocr_decision(false, "", true), PageOcrDecision::PerformOcr,);
        assert_eq!(page_ocr_decision(false, "   ", true), PageOcrDecision::PerformOcr,);
        assert_eq!(page_ocr_decision(false, "\n\t ", true), PageOcrDecision::PerformOcr,);
    }

    #[test]
    fn page_ocr_decision_empty_text_ocr_unavailable() {
        assert_eq!(page_ocr_decision(false, "", false), PageOcrDecision::SkipUnavailable,);
    }

    #[test]
    fn page_ocr_decision_force_overrides_native_text() {
        assert_eq!(page_ocr_decision(true, "has content", true), PageOcrDecision::PerformOcr,);
    }

    // --- resolve_ocr_settings tests ---

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
        let opts = OcrOptions { force: false, language_hints: vec![], timeout_secs: None };
        let resolved = resolve_ocr_settings(Some(&opts), "fra", 45);
        assert_eq!(resolved.language, "fra");
        assert_eq!(resolved.timeout.as_secs(), 45);
    }

    // --- OCR integration test helpers ---

    /// Find a usable tessdata directory from common system locations.
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
    fn tesseract_available() -> Option<std::path::PathBuf> {
        if std::process::Command::new("tesseract").arg("--version").output().is_err() {
            return None;
        }
        find_tessdata_dir()
    }

    /// Build an AppConfig suitable for PdfExtractor integration tests.
    fn test_pdf_config(
        tessdata_dir: Option<std::path::PathBuf>,
    ) -> Option<crate::config::AppConfig> {
        let pdfium_path = std::env::var("PDFIUM_LIBRARY_PATH")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())?;

        let mut config = crate::config::AppConfig::from_env().ok()?;
        config.pdfium_library_path = Some(pdfium_path);
        config.tessdata_dir = tessdata_dir;
        config.ocr_timeout_secs = 30;
        config.ocr_default_language = "eng".to_string();
        Some(config)
    }

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

        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.png");
        if !fixture.exists() {
            eprintln!("skipping: fixture not found at {}", fixture.display());
            return;
        }

        let png_bytes = std::fs::read(&fixture).expect("reading PNG fixture");

        let text = run_tesseract(&png_bytes, "eng", std::time::Duration::from_secs(30), &tessdata)
            .expect("tesseract should succeed on fixture");

        assert!(
            text.to_uppercase().contains("HELLO"),
            "OCR output should contain 'HELLO', got: {text:?}"
        );
    }

    #[tokio::test]
    #[ignore] // requires PDFium + Tesseract installed
    #[allow(clippy::disallowed_methods)]
    async fn pdf_extractor_ocr_fallback_on_scanned_page() {
        let tessdata = match tesseract_available() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: tesseract or tessdata not found");
                return;
            }
        };

        let config = match test_pdf_config(Some(tessdata)) {
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
            eprintln!("skipping: PDF fixture not found at {}", fixture.display());
            return;
        }

        let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

        let result = extractor
            .extract(&pdf_bytes, &ExtractionOptions::default())
            .await
            .expect("extraction should succeed with OCR fallback");

        assert!(
            result.text.to_uppercase().contains("HELLO"),
            "OCR fallback output should contain 'HELLO', got: {:?}",
            result.text
        );
    }

    #[tokio::test]
    #[ignore] // requires PDFium installed (no Tesseract needed)
    #[allow(clippy::disallowed_methods)]
    async fn pdf_extractor_forced_ocr_without_tessdata_errors() {
        let config = match test_pdf_config(None) {
            Some(c) => c,
            None => {
                eprintln!("skipping: PDFIUM_LIBRARY_PATH not set or not found");
                return;
            }
        };

        let extractor =
            PdfExtractor::new(&config).expect("PdfExtractor should construct without tessdata");

        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_ocr.pdf");
        if !fixture.exists() {
            eprintln!("skipping: PDF fixture not found at {}", fixture.display());
            return;
        }

        let pdf_bytes = std::fs::read(&fixture).expect("reading PDF fixture");

        let options = ExtractionOptions {
            ocr: Some(OcrOptions { force: true, language_hints: vec![], timeout_secs: None }),
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
}
