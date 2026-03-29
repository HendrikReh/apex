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

        if let Some(ref path) = config.pdfium_library_path {
            let pdf = PdfExtractor::new(path.clone()).context("configuring PDF extractor")?;
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
}

impl PdfExtractor {
    /// Create a new `PdfExtractor` with a validated PDFium library path.
    ///
    /// Eagerly loads the library to catch misconfiguration at startup.
    /// The loaded binding is immediately dropped — each `extract()` call
    /// creates its own.
    pub fn new(library_path: std::path::PathBuf) -> Result<Self> {
        // Validate the library is loadable at construction time.
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

        Ok(Self { library_path })
    }
}

impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    fn extract<'a>(
        &'a self,
        content: &'a [u8],
        _options: &'a ExtractionOptions,
    ) -> BoxFuture<'a, Result<ExtractionResult>> {
        // Copy input bytes to an owned Vec — spawn_blocking requires 'static.
        let owned_bytes = content.to_vec();
        let lib_path = self.library_path.clone();

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

                let mut pages = Vec::new();
                for page in doc.pages().iter() {
                    let text = page
                        .text()
                        .map_err(|e| anyhow!("extracting text from PDF page: {e}"))?
                        .all();
                    pages.push(text);
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
}
