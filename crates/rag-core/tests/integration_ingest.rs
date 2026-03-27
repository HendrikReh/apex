//! Integration tests for the ingest pipeline.
//!
//! Requires Postgres and Qdrant running (`just up`).
//! Each test uses a UUID-scoped tenant and collection to ensure isolation
//! across runs without needing cleanup.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::config::AppConfig;
use rag_core::ingest::{IngestDirectoryRequest, IngestFileRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::tenant::TenantId;
use tempfile::TempDir;
use uuid::Uuid;

async fn setup() -> Result<(IngestService, TempDir)> {
    // Force mock embedder for tests.
    unsafe { std::env::set_var("RAG_EMBEDDER", "mock") };
    let config = AppConfig::from_env()?;
    let stores = Stores::new(&config).await?;
    let service = IngestService::new(stores, &config)?;
    let dir = TempDir::new()?;
    Ok((service, dir))
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

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn ingest_single_txt_file() {
    let (service, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "hello.txt", "Hello, world! This is a test document for ingestion.");
    write_sidecar(dir.path(), "hello");

    let tenant = TenantId::new(format!("test-ingest-{suffix}")).expect("tenant");
    let outcome = service
        .ingest_file(IngestFileRequest {
            path: dir.path().join("hello.txt"),
            tenant,
            collection_override: Some(format!("test_ingest_{suffix}")),
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
    let (service, dir) = setup().await.expect("setup");
    let suffix = unique_suffix();
    write_fixture(dir.path(), "stable.txt", "Stable content that does not change between ingests.");
    write_sidecar(dir.path(), "stable");

    let tenant_str = format!("test-reingest-{suffix}");
    let collection = format!("test_reingest_{suffix}");
    let make_req = || IngestFileRequest {
        path: dir.path().join("stable.txt"),
        tenant: TenantId::new(&tenant_str).expect("tenant"),
        collection_override: Some(collection.clone()),
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
async fn ingest_directory_processes_all_files() {
    let (service, dir) = setup().await.expect("setup");
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
        })
        .await
        .expect("batch ingest");

    assert_eq!(outcome.documents, 2);
    assert!(outcome.chunks > 0);
    assert_eq!(outcome.skipped, 0);
    assert!(outcome.failures.is_empty());
}
