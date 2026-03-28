//! Integration tests for retrieval and context assembly.
//!
//! Requires Postgres and Qdrant running (`just up`).
//! Uses UUID-scoped tenant and collection per test for isolation.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::context::{ContextBuilder, ContextConfig, DedupeStrategy};
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::retrieval::RetrievalService;
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

async fn setup() -> Result<(IngestService, RetrievalService, AppConfig)> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    let ingest_stores = Stores::new(&config).await?;
    let retrieval_stores = Stores::new(&config).await?;
    let ingest = IngestService::new(ingest_stores, &config)?;
    let retrieval = RetrievalService::new(retrieval_stores, &config)?;
    Ok((ingest, retrieval, config))
}

async fn ingest_fixtures(
    ingest: &IngestService,
    dir: &TempDir,
) -> Result<(TenantId, String)> {
    let suffix = unique_id();
    let tenant: TenantId = format!("test-{suffix}").parse()?;
    let collection = format!("test_coll_{suffix}");

    write_fixture(
        dir.path(),
        "rust.md",
        "Rust is a systems programming language focused on safety and concurrency. \
         Async runtime tokio provides efficient task scheduling. \
         The borrow checker prevents data races at compile time.",
    );
    write_sidecar(dir.path(), "rust");
    write_fixture(
        dir.path(),
        "python.md",
        "Python is a high-level programming language for data science and scripting. \
         NumPy and pandas provide efficient data manipulation. \
         The GIL limits true parallelism in CPython.",
    );
    write_sidecar(dir.path(), "python");
    write_fixture(
        dir.path(),
        "cooking.md",
        "Sourdough bread requires a starter culture of wild yeast. \
         Fermentation time depends on ambient temperature. \
         A Dutch oven creates steam for a crispy crust.",
    );
    write_sidecar(dir.path(), "cooking");

    ingest
        .ingest_directory(IngestDirectoryRequest {
            path: dir.path().to_owned(),
            tenant: tenant.clone(),
            collection_override: Some(collection.clone()),
        })
        .await?;

    Ok((tenant, collection))
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn dense_search_returns_relevant_results_with_tenant_isolation() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let results = retrieval
        .search_dense(&collection, "rust async programming", tenant.as_str(), 10)
        .await?;

    assert!(!results.is_empty(), "dense search should return results");
    for chunk in &results {
        assert!(!chunk.chunk_id.is_empty(), "chunk_id should be populated");
        assert!(!chunk.document_id.is_empty(), "document_id should be populated");
        assert!(!chunk.text.is_empty(), "text should be populated");
    }

    let other_tenant: TenantId = format!("other-{}", unique_id()).parse()?;
    let isolated = retrieval
        .search_dense(&collection, "rust", other_tenant.as_str(), 10)
        .await?;
    assert!(isolated.is_empty(), "other tenant should see no results");

    Ok(())
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn sparse_search_returns_results_with_tenant_isolation() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let results = retrieval
        .search_sparse(&collection, "sourdough bread fermentation", tenant.as_str(), 10)
        .await?;

    assert!(!results.is_empty(), "sparse search should return results");
    assert_eq!(
        results[0].document_id, "cooking",
        "top sparse result for 'sourdough bread fermentation' should be from cooking.md"
    );

    let other_tenant: TenantId = format!("other-{}", unique_id()).parse()?;
    let isolated = retrieval
        .search_sparse(&collection, "sourdough", other_tenant.as_str(), 10)
        .await?;
    assert!(isolated.is_empty(), "other tenant should see no results");

    Ok(())
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
#[allow(clippy::disallowed_methods)]
async fn hybrid_search_fuses_dense_and_sparse() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval
        .search_hybrid(&collection, "rust programming", tenant.as_str(), None)
        .await?;

    assert!(!fused.is_empty(), "hybrid search should return results");
    for chunk in &fused {
        assert!(!chunk.chunk_id.is_empty());
        assert!(!chunk.document_id.is_empty());
        assert!(chunk.fused_score > 0.0, "fused score should be positive");
    }

    let multi_source = fused
        .iter()
        .find(|c| c.sources.len() > 1)
        .expect("hybrid search should produce at least one chunk found by both dense and sparse");
    if let Some(single) = fused.iter().find(|c| c.sources.len() == 1) {
        assert!(
            multi_source.fused_score > single.fused_score,
            "multi-source chunk ({}) should score higher than single-source chunk ({})",
            multi_source.fused_score,
            single.fused_score,
        );
    }
    for chunk in &fused {
        assert!(!chunk.sources.is_empty());
        for source in &chunk.sources {
            assert!(chunk.source_scores.contains_key(source));
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore] // requires running Postgres + Qdrant (`just up`)
async fn context_assembly_respects_token_budget() -> Result<()> {
    let (ingest, retrieval, _config) = setup().await?;
    let dir = TempDir::new()?;
    let (tenant, collection) = ingest_fixtures(&ingest, &dir).await?;

    let fused = retrieval
        .search_hybrid(&collection, "programming language", tenant.as_str(), None)
        .await?;

    assert!(!fused.is_empty(), "need results for context test");

    let config = ContextConfig {
        max_tokens: 20,
        max_chunks: 50,
        dedupe_strategy: DedupeStrategy::None,
        include_citations: true,
    };

    let result = ContextBuilder::new().build(fused, &config);

    assert!(result.stats.final_count <= result.stats.input_count);
    if result.stats.input_count > result.stats.final_count {
        assert!(result.stats.budget_dropped > 0);
    }
    if result.stats.final_count > 0 {
        assert!(!result.text.is_empty());
        assert_eq!(result.citations.len(), result.stats.final_count);
    }

    Ok(())
}
