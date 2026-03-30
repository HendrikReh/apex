//! Integration tests for PDF text extraction via pdfium-render.
//!
//! These tests require the PDFium native library to be installed and the
//! `PDFIUM_LIBRARY_PATH` env var to point to it. When unset, tests are
//! skipped (not failed).

use std::path::PathBuf;

fn pdfium_library_path() -> Option<PathBuf> {
    std::env::var("PDFIUM_LIBRARY_PATH").ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Build a minimal `AppConfig` from the current environment with
/// `pdfium_library_path` set to the given path.
#[allow(clippy::disallowed_methods)] // test helper
fn test_app_config(lib_path: PathBuf) -> rag_core::config::AppConfig {
    // Point config at a non-existent TOML so defaults are used, then
    // override just the pdfium path via env.
    unsafe { std::env::set_var("APP_CONFIG_PATH", "/tmp/nonexistent-apex-config.toml") };
    unsafe { std::env::set_var("PDFIUM_LIBRARY_PATH", lib_path.to_str().unwrap()) };
    let cfg = rag_core::config::AppConfig::from_env().expect("test AppConfig");
    unsafe { std::env::remove_var("PDFIUM_LIBRARY_PATH") };
    cfg
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // test assertions
async fn pdf_extractor_extracts_two_pages_with_formfeed_separator() {
    let Some(lib_path) = pdfium_library_path() else {
        eprintln!(
            "SKIP: PDFIUM_LIBRARY_PATH not set — \
             set it to the PDFium native library path to run PDF tests"
        );
        return;
    };

    let config = test_app_config(lib_path);
    let extractor =
        rag_core::extract::PdfExtractor::new(&config).expect("PdfExtractor::new should succeed");

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
    let pdf_bytes = std::fs::read(&fixture).expect("reading test fixture PDF");

    use rag_core::extract::{ExtractionOptions, FormatExtractor};
    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("PDF extraction should succeed");

    // Verify form-feed separator between pages
    let pages: Vec<&str> = result.text.split('\u{000C}').collect();
    assert_eq!(pages.len(), 2, "expected 2 pages separated by form-feed, got {}", pages.len());

    // Verify extracted text contains expected content
    assert!(
        pages[0].contains("Page one") || pages[0].contains("hello"),
        "page 1 should contain expected text, got: {:?}",
        pages[0]
    );
    assert!(
        pages[1].contains("Page two") || pages[1].contains("extraction"),
        "page 2 should contain expected text, got: {:?}",
        pages[1]
    );
}

#[test]
#[allow(clippy::disallowed_methods)] // test assertions
fn pdf_extractor_supported_types() {
    let Some(lib_path) = pdfium_library_path() else {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set");
        return;
    };

    let config = test_app_config(lib_path);
    let extractor =
        rag_core::extract::PdfExtractor::new(&config).expect("PdfExtractor::new should succeed");

    use rag_core::extract::FormatExtractor;
    assert_eq!(extractor.supported_types(), &[rag_core::extract::FileType::Pdf]);
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // test assertions
async fn pdf_extractor_rejects_corrupt_pdf() {
    let Some(lib_path) = pdfium_library_path() else {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set");
        return;
    };

    let config = test_app_config(lib_path);
    let extractor =
        rag_core::extract::PdfExtractor::new(&config).expect("PdfExtractor::new should succeed");

    use rag_core::extract::{ExtractionOptions, FormatExtractor};
    let result = extractor.extract(b"this is not a PDF", &ExtractionOptions::default()).await;
    assert!(result.is_err(), "corrupt PDF should produce an error");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // test assertions
async fn pdf_extractor_returns_native_metadata() {
    let Some(lib_path) = pdfium_library_path() else {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set");
        return;
    };

    let config = test_app_config(lib_path);
    let extractor =
        rag_core::extract::PdfExtractor::new(&config).expect("PdfExtractor::new should succeed");

    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf");
    let pdf_bytes = std::fs::read(&fixture).expect("reading test fixture PDF");

    use rag_core::extract::{ExtractionOptions, FormatExtractor};
    let result = extractor
        .extract(&pdf_bytes, &ExtractionOptions::default())
        .await
        .expect("PDF extraction should succeed");

    let metadata =
        result.native_metadata.as_ref().expect("two-pages.pdf should have native metadata");

    assert!(
        metadata.contains_key("creation_date"),
        "two-pages.pdf should have creation_date, got keys: {:?}",
        metadata.keys().collect::<Vec<_>>()
    );
}
