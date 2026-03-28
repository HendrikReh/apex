//! End-to-end integration tests for the chat service.
//!
//! Requires Postgres + Qdrant running (`just up`) and LLM_API_KEY set.
//! These tests exercise the full pipeline: ingest → chat → verify response.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rag_core::chat::{ChatRequest, ChatService};
use rag_core::config::AppConfig;
use rag_core::ingest::{IngestDirectoryRequest, IngestService};
use rag_core::stores::Stores;
use rag_core::stores::conversations::MessageRole;
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

async fn setup() -> Result<(IngestService, ChatService, Stores)> {
    let config = AppConfig::from_env()?;
    if config.llm_api_key.is_none() {
        anyhow::bail!("LLM_API_KEY required for chat integration tests");
    }
    let stores = Stores::new(&config).await?;
    let ingest = IngestService::new(stores.clone(), &config)?;
    let chat = ChatService::new(stores.clone(), &config)?;
    Ok((ingest, chat, stores))
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
         Async runtime tokio provides efficient task scheduling. \
         The borrow checker prevents data races at compile time.",
    );
    write_sidecar(dir.path(), "rust");
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
            collection_override: Some(collection.to_owned()),
        })
        .await?;

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
#[allow(clippy::disallowed_methods)] // write_fixture/write_sidecar helpers use .expect()
async fn chat_returns_answer_with_citations() -> Result<()> {
    let (ingest, chat, _stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let dir = TempDir::new()?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-chat-{suffix}").parse()?;
    let collection = format!("test_chat_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    let response = chat
        .chat(ChatRequest {
            query: "What is Rust?".into(),
            collection: Some(collection),
            tenant,
            conversation_id: None,
            language: None,
            history_limit: None,
        })
        .await?;

    assert!(!response.answer.is_empty(), "answer should not be empty");
    assert!(!response.citations.is_empty(), "citations should be present");
    assert!(!response.model.is_empty(), "model should be populated");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
#[allow(clippy::disallowed_methods)] // write_fixture/write_sidecar helpers use .expect()
async fn multi_turn_conversation_loads_history() -> Result<()> {
    let (ingest, chat, stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let dir = TempDir::new()?;
    let suffix = unique_id();
    let tenant: TenantId = format!("test-multi-{suffix}").parse()?;
    let collection = format!("test_multi_{suffix}");

    ingest_fixtures(&ingest, &dir, &tenant, &collection).await?;

    // First turn creates conversation.
    let first = chat
        .chat(ChatRequest {
            query: "Tell me about Rust.".into(),
            collection: Some(collection.clone()),
            tenant: tenant.clone(),
            conversation_id: None,
            language: None,
            history_limit: None,
        })
        .await?;
    let conv_id = first.conversation_id;

    // Second turn resumes conversation.
    let second = chat
        .chat(ChatRequest {
            query: "What about its type system?".into(),
            collection: None, // should use stored collection
            tenant: tenant.clone(),
            conversation_id: Some(conv_id),
            language: None,
            history_limit: None,
        })
        .await?;
    assert_eq!(second.conversation_id, conv_id);

    // Verify 4 messages persisted (2 user + 2 assistant).
    let messages = stores.get_messages(tenant.as_str(), conv_id, 100).await?;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[2].role, MessageRole::User);
    assert_eq!(messages[3].role, MessageRole::Assistant);

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres + Qdrant + LLM_API_KEY
#[allow(clippy::disallowed_methods)] // expect_err() used in test assertion
async fn collection_mismatch_rejected() -> Result<()> {
    let (_ingest, chat, stores) = match setup().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            return Ok(());
        }
    };
    let suffix = unique_id();
    let tenant: TenantId = format!("test-mismatch-{suffix}").parse()?;

    // Create conversation with collection_a.
    let conv = stores.create_conversation(tenant.as_str(), None, Some("collection_a")).await?;

    // Try to chat with collection_b on the same conversation.
    let result = chat
        .chat(ChatRequest {
            query: "Hello".into(),
            collection: Some("collection_b".into()),
            tenant,
            conversation_id: Some(conv.id),
            language: None,
            history_limit: None,
        })
        .await;

    assert!(result.is_err());
    let err_msg = result.expect_err("expected mismatch error").to_string();
    assert!(err_msg.contains("collection mismatch"), "error should mention mismatch: {err_msg}");

    Ok(())
}
