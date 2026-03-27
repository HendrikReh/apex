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

/// Compute a SHA-256 checksum over extracted text.
pub fn checksum(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Async-capable extractor interface for one or more file types.
pub trait FormatExtractor: Send + Sync {
    fn supported_types(&self) -> &'static [FileType];

    fn extract<'a>(&'a self, content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>>;
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

    /// Construct the default registry used by the initial extraction pipeline.
    pub fn with_defaults() -> Result<Self> {
        Self::new(vec![
            Box::new(PdfExtractor),
            Box::new(MarkdownExtractor),
            Box::new(TextExtractor),
        ])
    }

    /// Extract normalized text for the given file type.
    pub async fn extract(&self, file_type: FileType, content: &[u8]) -> Result<ExtractionResult> {
        let Some(index) = self.by_type.get(&file_type).copied() else {
            bail!("no extractor registered for file type {file_type:?}");
        };

        self.extractors[index]
            .extract(content)
            .await
            .map_err(|err| anyhow!("extracting content for file type {file_type:?}: {err}"))
    }
}

#[derive(Debug, Default)]
pub struct TextExtractor;

impl FormatExtractor for TextExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Text]
    }

    fn extract<'a>(&'a self, content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "plain text") })
    }
}

#[derive(Debug, Default)]
pub struct MarkdownExtractor;

impl FormatExtractor for MarkdownExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Markdown]
    }

    fn extract<'a>(&'a self, content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move { extract_utf8_passthrough(content, "markdown") })
    }
}

#[derive(Debug, Default)]
pub struct PdfExtractor;

impl FormatExtractor for PdfExtractor {
    fn supported_types(&self) -> &'static [FileType] {
        &[FileType::Pdf]
    }

    fn extract<'a>(&'a self, _content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>> {
        Box::pin(async move {
            Err(anyhow!(
                "PDF extraction is not implemented yet; wire in a pdfium-backed extractor next"
            ))
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

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn text_registry_extracts_utf8_content() {
        let registry = ExtractorRegistry::with_defaults().expect("default registry should build");

        let result = registry
            .extract(FileType::Text, b"hello world")
            .await
            .expect("text extraction should succeed");

        assert_eq!(result.text, "hello world");
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // test assertions
    async fn markdown_registry_extracts_utf8_content() {
        let registry = ExtractorRegistry::with_defaults().expect("default registry should build");

        let result = registry
            .extract(FileType::Markdown, b"# Title\n\nBody")
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

            fn extract<'a>(&'a self, content: &'a [u8]) -> BoxFuture<'a, Result<ExtractionResult>> {
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
    async fn pdf_extractor_is_explicitly_unimplemented() {
        let registry = ExtractorRegistry::with_defaults().expect("default registry should build");

        let err = registry
            .extract(FileType::Pdf, b"%PDF-1.7")
            .await
            .expect_err("pdf extraction should fail until pdfium is wired in");

        assert!(
            err.to_string().contains("not implemented"),
            "expected unimplemented pdf extraction error, got {err}"
        );
    }

    #[tokio::test]
    async fn invalid_utf8_errors_keep_decode_cause_visible() {
        let registry = ExtractorRegistry::with_defaults().expect("default registry should build");

        let err = registry
            .extract(FileType::Text, &[0xff, 0xfe, 0xfd])
            .await
            .expect_err("invalid UTF-8 should fail");

        assert!(
            err.to_string().contains("decoding plain text content as UTF-8"),
            "expected UTF-8 decode cause in error, got {err}"
        );
    }

    #[test]
    fn checksum_is_stable_for_extracted_text() {
        assert_eq!(
            checksum("hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
