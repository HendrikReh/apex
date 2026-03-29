//! Integration tests for PDF text extraction via pdfium-render.
//!
//! These tests require the PDFium native library to be installed and the
//! `PDFIUM_LIBRARY_PATH` env var to point to it. When unset, tests are
//! skipped (not failed).

use std::path::PathBuf;

fn pdfium_library_path() -> Option<PathBuf> {
    std::env::var("PDFIUM_LIBRARY_PATH").ok().filter(|v| !v.is_empty()).map(PathBuf::from)
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

    let extractor =
        rag_core::extract::PdfExtractor::new(lib_path).expect("PdfExtractor::new should succeed");

    let pdf_bytes = std::fs::read("crates/rag-core/tests/fixtures/two-pages.pdf")
        .expect("reading test fixture PDF");

    use rag_core::extract::FormatExtractor;
    let result = extractor.extract(&pdf_bytes).await.expect("PDF extraction should succeed");

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

    let extractor =
        rag_core::extract::PdfExtractor::new(lib_path).expect("PdfExtractor::new should succeed");

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

    let extractor =
        rag_core::extract::PdfExtractor::new(lib_path).expect("PdfExtractor::new should succeed");

    use rag_core::extract::FormatExtractor;
    let result = extractor.extract(b"this is not a PDF").await;
    assert!(result.is_err(), "corrupt PDF should produce an error");
}
