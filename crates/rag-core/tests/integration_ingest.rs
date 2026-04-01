//! Integration tests for the ingest pipeline.
//!
//! Requires Postgres and Qdrant running (`just up`).
//! Each test uses a UUID-scoped tenant and collection to ensure isolation
//! across runs without needing cleanup.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::ingest::{IngestDirectoryRequest, IngestFileRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::tenant::TenantId;
use tempfile::TempDir;
use uuid::Uuid;

async fn setup() -> Result<(IngestService, Stores, TempDir)> {
    let dir = TempDir::new()?;
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    config.ingest_allowed_roots = vec![dir.path().to_path_buf()];
    let stores = Stores::new(&config).await?;
    let service = IngestService::new(stores.clone(), &config)?;
    Ok((service, stores, dir))
}

/// Generate a unique test tenant/collection suffix per run.
fn unique_suffix() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

#[allow(clippy::disallowed_methods)]
fn write_fixture(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).expect("writing fixture");
}

#[allow(clippy::disallowed_methods)]
fn write_sidecar(dir: &Path, stem: &str) {
    let json = format!(
        r#"{{
            "schema_version": 1,
            "document": {{ "id": "{stem}", "title": "Test {stem}", "category": "report" }},
            "source": {{ "url": "https://example.com/{stem}", "domain": "example.com", "publisher": "Test" }},
            "language": "en",
            "tags": ["test"],
            "acl": {{ "allow_roles": ["*"] }},
            "security": {{ "classification": "public", "requires_evidence_pack": false }},
            "provenance": {{ "retrieved_at": "2026-03-27T00:00:00Z", "retrieved_by": "test" }}
        }}"#
    );
    fs::write(dir.join(format!("{stem}.metadata.json")), json).expect("writing sidecar");
}

#[allow(clippy::disallowed_methods)]
fn write_sidecar_with_chunking(dir: &Path, stem: &str, max_tokens: usize) {
    let json = format!(
        r#"{{
            "schema_version": 1,
            "document": {{ "id": "{stem}", "title": "Test {stem}", "category": "report" }},
            "source": {{ "url": "https://example.com/{stem}", "domain": "example.com", "publisher": "Test" }},
            "language": "en",
            "tags": ["test"],
            "acl": {{ "allow_roles": ["*"] }},
            "security": {{ "classification": "public", "requires_evidence_pack": false }},
            "provenance": {{ "retrieved_at": "2026-03-27T00:00:00Z", "retrieved_by": "test" }},
            "ingestion": {{
                "chunking": {{ "max_tokens": {max_tokens} }}
            }}
        }}"#
    );
    fs::write(dir.join(format!("{stem}.metadata.json")), json).expect("writing sidecar");
}

/// Copy a test fixture from the fixtures directory into the given temp directory.
#[allow(clippy::disallowed_methods)]
fn copy_fixture(dir: &Path, fixture_name: &str) {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(fixture_name);
    fs::copy(&src, dir.join(fixture_name)).expect("copying fixture");
}

/// Check whether PDF ingest tests can run (requires PDFIUM_LIBRARY_PATH + infra).
fn pdf_ingest_available() -> bool {
    std::env::var("PDFIUM_LIBRARY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| std::path::Path::new(&v).exists())
        .unwrap_or(false)
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn ingest_single_txt_file() {
    let (service, _stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "hello.txt", "Hello, world! This is a test document for ingestion.");
    write_sidecar(dir.path(), "hello");

    let tenant = TenantId::new(format!("test-ingest-{suffix}")).expect("tenant");
    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("hello.txt"),
            tenant,
            collection_override: Some(format!("test_ingest_{suffix}")),
            dry_run: false,
        })
        .await
        .expect("ingest should succeed");

    assert_eq!(outcome.document_id, "hello");
    assert!(!outcome.skipped);
    assert!(outcome.chunks_created > 0);
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn reingest_unchanged_file_is_skipped() {
    let (service, _stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "stable.txt", "Stable content that does not change between ingests.");
    write_sidecar(dir.path(), "stable");

    let tenant_str = format!("test-reingest-{suffix}");
    let collection = format!("test_reingest_{suffix}");
    let make_req = || IngestFileRequest {
        path: dir.path().join("stable.txt"),
        tenant: TenantId::new(&tenant_str).expect("tenant"),
        collection_override: Some(collection.clone()),
        dry_run: false,
    };

    let first = service.ingest_file(make_req()).await.expect("first ingest");
    assert!(!first.skipped);

    let second = service.ingest_file(make_req()).await.expect("second ingest");
    assert!(second.skipped);
    assert_eq!(second.chunks_created, 0);
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn reingest_modified_file_updates_chunks_in_place() {
    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    let tenant_str = format!("test-modify-{suffix}");
    let collection = format!("test_modify_{suffix}");
    let path = dir.path().join("mutable.txt");

    // Force smaller chunks so the initial ingest produces multiple rows.
    write_sidecar_with_chunking(dir.path(), "mutable", 3);
    write_fixture(
        dir.path(),
        "mutable.txt",
        "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
    );

    let make_req = || IngestFileRequest {
        path: path.clone(),
        tenant: TenantId::new(&tenant_str).expect("tenant"),
        collection_override: Some(collection.clone()),
        dry_run: false,
    };

    let first = service.ingest_file(make_req()).await.expect("first ingest");
    assert!(!first.skipped);
    assert!(first.chunks_created > 1, "initial ingest should produce multiple chunks");

    let first_chunks =
        stores.get_chunks_by_document(&tenant_str, "mutable").await.expect("first chunk fetch");
    assert_eq!(
        first_chunks.len(),
        first.chunks_created,
        "postgres chunk rows should match the reported chunk count"
    );

    // Rewrite the file with substantially shorter content so the chunk count shrinks.
    write_fixture(dir.path(), "mutable.txt", "alpha beta gamma");

    let second = service.ingest_file(make_req()).await.expect("second ingest");
    assert!(!second.skipped, "modified file should reingest");
    assert!(
        second.chunks_created < first.chunks_created,
        "modified content should produce fewer chunks"
    );

    let second_chunks =
        stores.get_chunks_by_document(&tenant_str, "mutable").await.expect("second chunk fetch");
    assert_eq!(
        second_chunks.len(),
        second.chunks_created,
        "stale trailing Postgres chunks should be removed on re-ingest"
    );
    assert!(
        second_chunks.iter().all(|chunk| !chunk.text.contains("delta")),
        "updated chunk set should no longer contain text from the old trailing chunks"
    );
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn ingest_directory_processes_all_files() {
    let (service, _stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "a.txt", "Document A content for testing batch ingestion.");
    write_sidecar(dir.path(), "a");
    write_fixture(dir.path(), "b.md", "# Document B\n\nMarkdown content for testing.");
    write_sidecar(dir.path(), "b");
    // unsupported file should be skipped
    fs::write(dir.path().join("skip.png"), "not a document").expect("write");

    let tenant = TenantId::new(format!("test-batch-{suffix}")).expect("tenant");
    let outcome = service
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant,
            collection_override: Some(format!("test_batch_{suffix}")),
            dry_run: false,
        })
        .await
        .expect("batch ingest");

    assert_eq!(outcome.documents, 2);
    assert!(outcome.chunks > 0);
    assert_eq!(outcome.skipped, 0);
    assert!(outcome.failures.is_empty());
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn dry_run_new_file_does_not_write() {
    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "dryrun.txt", "Content for dry-run test of a new file.");

    let tenant_str = format!("test-dry-new-{suffix}");
    let tenant = TenantId::new(&tenant_str).expect("tenant");
    let collection = format!("test_dry_new_{suffix}");

    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("dryrun.txt"),
            tenant: tenant.clone(),
            collection_override: Some(collection.clone()),
            dry_run: true,
        })
        .await
        .expect("dry-run ingest should succeed");

    assert!(!outcome.skipped, "new file should not be marked as skipped");
    assert_eq!(outcome.chunks_created, 0, "dry-run must not create chunks");

    // Verify nothing was persisted to Postgres.
    let checksum = stores
        .get_document_checksum(tenant.as_str(), &outcome.document_id)
        .await
        .expect("checksum query should succeed");
    assert!(checksum.is_none(), "dry-run must not write document to Postgres");

    // Verify nothing was written to Qdrant — the unique collection should not exist.
    let qdrant_exists = stores
        .collection_exists(&collection)
        .await
        .expect("collection_exists query should succeed");
    assert!(!qdrant_exists, "dry-run must not create Qdrant collection");
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn dry_run_unchanged_file_returns_skipped() {
    let (service, _stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(
        dir.path(),
        "existing.txt",
        "Content that will be ingested then dry-run checked.",
    );
    write_sidecar(dir.path(), "existing");

    let tenant_str = format!("test-dry-skip-{suffix}");
    let collection = format!("test_dry_skip_{suffix}");

    // First: real ingest to populate Postgres.
    let first = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("existing.txt"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection.clone()),
            dry_run: false,
        })
        .await
        .expect("initial ingest should succeed");
    assert!(!first.skipped);
    assert!(first.chunks_created > 0);

    // Second: dry-run on the same unchanged file.
    let second = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("existing.txt"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection),
            dry_run: true,
        })
        .await
        .expect("dry-run ingest should succeed");

    assert!(second.skipped, "unchanged file should be marked as skipped");
    assert_eq!(second.chunks_created, 0, "dry-run must not create chunks");
}

#[tokio::test]
#[ignore] // requires PDFium + Postgres + Qdrant
#[allow(clippy::disallowed_methods)]
async fn pdf_with_sidecar_uses_sidecar_title() {
    if !pdf_ingest_available() {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set or not found");
        return;
    }

    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    copy_fixture(dir.path(), "titled.pdf");
    write_sidecar(dir.path(), "titled"); // title = "Test titled"

    let tenant_str = format!("test-pdf-sidecar-{suffix}");
    let collection = format!("test_pdf_sidecar_{suffix}");

    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("titled.pdf"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection),
            dry_run: false,
        })
        .await
        .expect("ingest should succeed");

    assert!(!outcome.skipped);

    let row = stores
        .get_document(&tenant_str, &outcome.document_id)
        .await
        .expect("get_document query")
        .expect("document row should exist");

    // Sidecar title wins over PDF native title
    assert_eq!(
        row.title, "Test titled",
        "sidecar title should take precedence over PDF native title"
    );

    // native.pdf metadata should still be present in JSONB
    let metadata = row.metadata.expect("metadata should be Some");
    let pdf_meta =
        metadata.get("native").and_then(|n| n.get("pdf")).expect("metadata should have native.pdf");

    assert_eq!(
        pdf_meta.get("title").and_then(|t| t.as_str()),
        Some("Apex Test Document"),
        "native.pdf.title should contain the PDF's own title"
    );
}

#[tokio::test]
#[ignore] // requires PDFium + Postgres + Qdrant
#[allow(clippy::disallowed_methods)]
async fn pdf_without_sidecar_uses_native_title() {
    if !pdf_ingest_available() {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set or not found");
        return;
    }

    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    copy_fixture(dir.path(), "titled.pdf");
    // No sidecar — title should fall back to PDF native

    let tenant_str = format!("test-pdf-native-{suffix}");
    let collection = format!("test_pdf_native_{suffix}");

    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("titled.pdf"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection),
            dry_run: false,
        })
        .await
        .expect("ingest should succeed");

    assert!(!outcome.skipped);

    let row = stores
        .get_document(&tenant_str, &outcome.document_id)
        .await
        .expect("get_document query")
        .expect("document row should exist");

    assert_eq!(
        row.title, "Apex Test Document",
        "without sidecar, title should fall back to PDF native title"
    );

    let metadata = row.metadata.expect("metadata should be Some");
    let pdf_meta =
        metadata.get("native").and_then(|n| n.get("pdf")).expect("metadata should have native.pdf");
    assert!(pdf_meta.get("title").is_some(), "native.pdf should contain title");
}

#[tokio::test]
#[ignore] // requires PDFium + Postgres + Qdrant
#[allow(clippy::disallowed_methods)]
async fn pdf_without_native_title_stores_empty_title() {
    if !pdf_ingest_available() {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set or not found");
        return;
    }

    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    copy_fixture(dir.path(), "two-pages.pdf");
    // No sidecar; two-pages.pdf has creation_date but no title

    let tenant_str = format!("test-pdf-notitle-{suffix}");
    let collection = format!("test_pdf_notitle_{suffix}");

    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("two-pages.pdf"),
            tenant: TenantId::new(&tenant_str).expect("tenant"),
            collection_override: Some(collection),
            dry_run: false,
        })
        .await
        .expect("ingest should succeed");

    assert!(!outcome.skipped);

    let row = stores
        .get_document(&tenant_str, &outcome.document_id)
        .await
        .expect("get_document query")
        .expect("document row should exist");

    assert_eq!(row.title, "", "PDF without native title and no sidecar should store empty title");

    // Should still have native.pdf with creation_date
    let metadata = row.metadata.expect("metadata should be Some");
    let pdf_meta =
        metadata.get("native").and_then(|n| n.get("pdf")).expect("metadata should have native.pdf");
    assert!(pdf_meta.get("creation_date").is_some(), "native.pdf should contain creation_date");
    assert!(
        pdf_meta.get("title").is_none(),
        "native.pdf should NOT contain title (two-pages.pdf has none)"
    );
}

#[tokio::test]
#[ignore] // requires PDFium + Postgres + Qdrant
#[allow(clippy::disallowed_methods)]
async fn pdf_reingest_unchanged_preserves_metadata() {
    if !pdf_ingest_available() {
        eprintln!("SKIP: PDFIUM_LIBRARY_PATH not set or not found");
        return;
    }

    let (service, stores, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    copy_fixture(dir.path(), "titled.pdf");

    let tenant_str = format!("test-pdf-reingest-{suffix}");
    let collection = format!("test_pdf_reingest_{suffix}");

    let make_req = || IngestFileRequest {
        path: dir.path().join("titled.pdf"),
        tenant: TenantId::new(&tenant_str).expect("tenant"),
        collection_override: Some(collection.clone()),
        dry_run: false,
    };

    // First ingest (normal persist path)
    let first = service.ingest_file(make_req()).await.expect("first ingest");
    assert!(!first.skipped);

    let row_first =
        stores.get_document(&tenant_str, &first.document_id).await.expect("query").expect("row");

    // Second ingest (skip-on-unchanged path)
    let second = service.ingest_file(make_req()).await.expect("second ingest");
    assert!(second.skipped);

    let row_second =
        stores.get_document(&tenant_str, &second.document_id).await.expect("query").expect("row");

    // Both paths should produce the same title and metadata shape
    assert_eq!(
        row_first.title, row_second.title,
        "title should be identical across persist and skip paths"
    );
    assert_eq!(
        row_first.metadata, row_second.metadata,
        "metadata JSONB should be identical across persist and skip paths"
    );
}
