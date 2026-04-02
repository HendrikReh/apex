#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::chat::ChatService;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::tenant::TenantId;
use tempfile::TempDir;
use uuid::Uuid;

fn unique_id() -> String {
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

async fn setup(ingest_root: &Path) -> Result<(IngestService, ChatService)> {
    setup_with_context_max_chunks(ingest_root, None).await
}

async fn setup_with_context_max_chunks(
    ingest_root: &Path,
    context_max_chunks: Option<usize>,
) -> Result<(IngestService, ChatService)> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    config.ingest_allowed_roots = vec![ingest_root.canonicalize()?];
    if let Some(limit) = context_max_chunks {
        config.context_max_chunks = limit;
    }
    config.llm_prompt_template_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/prompts/chat_system.hbs")
        .display()
        .to_string();
    let stores = Stores::new(&config).await?;
    let ingest = IngestService::new(stores.clone(), &config)?;
    let chat = ChatService::with_mock_llm(stores, &config, "Mock LLM response.".into())?;
    Ok((ingest, chat))
}

async fn ingest_fixtures(
    ingest: &IngestService,
    dir: &TempDir,
    tenant: &TenantId,
    collection: &str,
) -> Result<()> {
    write_fixture(
        dir.path(),
        "rust.md",
        "Rust is a systems programming language focused on safety and concurrency. \
         Tokio provides async task scheduling. \
         Ownership and borrowing prevent data races.",
    );
    write_sidecar(dir.path(), "rust");

    ingest
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant: tenant.clone(),
            collection_override: Some(collection.to_owned()),
            dry_run: false,
        })
        .await?;

    Ok(())
}

async fn ingest_multi_fixtures(
    ingest: &IngestService,
    dir: &TempDir,
    tenant: &TenantId,
    collection: &str,
) -> Result<()> {
    write_fixture(
        dir.path(),
        "rust.md",
        "Rust is a systems programming language focused on safety and concurrency. \
         Ownership and borrowing prevent data races.",
    );
    write_sidecar(dir.path(), "rust");
    write_fixture(
        dir.path(),
        "tokio.md",
        "Tokio is an async runtime for Rust. \
         It provides task scheduling, timers, and networking primitives.",
    );
    write_sidecar(dir.path(), "tokio");

    ingest
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant: tenant.clone(),
            collection_override: Some(collection.to_owned()),
            dry_run: false,
        })
        .await?;

    Ok(())
}

#[tokio::test]
#[ignore] // requires `just up`
async fn answer_single_shot_returns_grounded_answer() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, chat) = setup(dir.path()).await?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-single-shot-{suffix}").parse()?;
    let collection = format!("test_single_shot_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    let response =
        chat.answer_single_shot("What is Rust?", &collection, tenant.as_str(), None).await?;

    assert_eq!(response.answer, "Mock LLM response.");
    assert!(!response.citations.is_empty(), "citations should be present");
    assert!(!response.evidence.is_empty(), "evidence should be present");
    assert!(!response.model.is_empty(), "model should be populated");

    Ok(())
}

#[tokio::test]
#[ignore] // requires `just up`
async fn answer_single_shot_evidence_matches_cited_chunks_under_budget() -> Result<()> {
    let dir = TempDir::new()?;
    let (ingest, chat) = setup_with_context_max_chunks(dir.path(), Some(1)).await?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-single-shot-budget-{suffix}").parse()?;
    let collection = format!("test_single_shot_budget_{suffix}");

    ingest_multi_fixtures(&ingest, &dir, &tenant, &collection).await?;

    let response = chat
        .answer_single_shot("What are Rust and Tokio?", &collection, tenant.as_str(), None)
        .await?;

    assert!(!response.citations.is_empty(), "citations should be present");
    assert_eq!(
        response.evidence.len(),
        response.citations.len(),
        "evidence should only include chunks that survived context building",
    );

    let evidence_chunk_ids: Vec<&str> =
        response.evidence.iter().map(|chunk| chunk.chunk_id.as_str()).collect();
    let citation_chunk_ids: Vec<&str> =
        response.citations.iter().map(|citation| citation.chunk_id.as_str()).collect();

    assert_eq!(
        evidence_chunk_ids, citation_chunk_ids,
        "evidence should align with the chunks actually cited in the prompt",
    );

    Ok(())
}
