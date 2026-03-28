//! Integration tests for conversation and message persistence.
//!
//! Requires Postgres running (`just up`). No Qdrant or LLM needed.

use anyhow::Result;
use rag_core::config::{AppConfig, EmbedderKind};
use rag_core::stores::Stores;
use rag_core::stores::conversations::MessageRole;
use uuid::Uuid;

async fn setup() -> Result<Stores> {
    let mut config = AppConfig::from_env()?;
    config.embedder = EmbedderKind::Mock;
    Stores::new(&config).await
}

fn unique_tenant() -> String {
    format!("test-conv-{}", &Uuid::new_v4().to_string()[..8])
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect() used intentionally in test assertions
async fn create_and_get_conversation() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv =
        stores.create_conversation(&tenant, Some("Test title"), Some("my_collection")).await?;

    assert_eq!(conv.tenant, tenant);
    assert_eq!(conv.title.as_deref(), Some("Test title"));
    assert_eq!(conv.collection.as_deref(), Some("my_collection"));

    let fetched =
        stores.get_conversation(&tenant, conv.id).await?.expect("conversation should exist");
    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.tenant, tenant);

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn get_conversation_wrong_tenant_returns_none() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, None).await?;

    let result = stores.get_conversation("other-tenant", conv.id).await?;
    assert!(result.is_none(), "wrong tenant should not see conversation");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn insert_and_get_messages_role_roundtrip() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, Some("coll")).await?;

    let user_msg =
        stores.insert_message(&tenant, conv.id, MessageRole::User, "Hello", None).await?;
    assert_eq!(user_msg.role, MessageRole::User);
    assert_eq!(user_msg.content, "Hello");

    let asst_msg =
        stores.insert_message(&tenant, conv.id, MessageRole::Assistant, "Hi there", None).await?;
    assert_eq!(asst_msg.role, MessageRole::Assistant);
    assert_eq!(asst_msg.content, "Hi there");

    let messages = stores.get_messages(&tenant, conv.id, 10).await?;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content, "Hi there");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn system_role_roundtrip() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, Some("coll")).await?;

    let system_msg =
        stores.insert_message(&tenant, conv.id, MessageRole::System, "System note", None).await?;
    assert_eq!(system_msg.role, MessageRole::System);

    let messages = stores.get_messages(&tenant, conv.id, 10).await?;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, MessageRole::System);
    assert_eq!(messages[0].content, "System note");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn get_messages_limit_respected() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, None).await?;

    for i in 0..5 {
        stores
            .insert_message(&tenant, conv.id, MessageRole::User, &format!("Message {i}"), None)
            .await?;
    }

    let messages = stores.get_messages(&tenant, conv.id, 3).await?;
    assert_eq!(messages.len(), 3);
    // Should be the latest 3, returned oldest-first.
    assert_eq!(messages[0].content, "Message 2");
    assert_eq!(messages[1].content, "Message 3");
    assert_eq!(messages[2].content, "Message 4");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn tenant_isolation_on_insert() -> Result<()> {
    let stores = setup().await?;
    let tenant_a = unique_tenant();

    let conv = stores.create_conversation(&tenant_a, None, Some("coll")).await?;

    let result =
        stores.insert_message("other-tenant", conv.id, MessageRole::User, "Sneaky", None).await;
    assert!(result.is_err(), "should not insert into other tenant's conversation");
    let err_msg = result.expect_err("expected error").to_string();
    assert!(
        err_msg.contains("conversation not found for tenant"),
        "error should be explicit: {err_msg}"
    );

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn tenant_isolation_on_get_messages() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();

    let conv = stores.create_conversation(&tenant, None, Some("coll")).await?;
    stores.insert_message(&tenant, conv.id, MessageRole::User, "Secret", None).await?;

    let messages = stores.get_messages("other-tenant", conv.id, 10).await?;
    assert!(messages.is_empty(), "other tenant should see no messages");

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn insert_into_nonexistent_conversation() -> Result<()> {
    let stores = setup().await?;
    let fake_id = Uuid::new_v4();

    let result =
        stores.insert_message("any-tenant", fake_id, MessageRole::User, "Hello", None).await;
    assert!(result.is_err());
    let err_msg = result.expect_err("expected error").to_string();
    assert!(
        err_msg.contains("conversation not found for tenant"),
        "error should be explicit: {err_msg}"
    );

    Ok(())
}

#[tokio::test]
#[ignore] // requires Postgres (`just up`)
#[allow(clippy::disallowed_methods)] // .expect_err() used intentionally in test assertions
async fn get_messages_limit_zero_rejected() -> Result<()> {
    let stores = setup().await?;
    let tenant = unique_tenant();
    let conv = stores.create_conversation(&tenant, None, None).await?;

    let result = stores.get_messages(&tenant, conv.id, 0).await;
    assert!(result.is_err());
    let err_msg = result.expect_err("expected error").to_string();
    assert!(err_msg.contains("limit must be > 0"), "error: {err_msg}");

    Ok(())
}
