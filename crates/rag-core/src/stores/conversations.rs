//! Conversation and message persistence (Postgres).
//!
//! Operates on the `conversations` and `messages` tables from migration 0002.
//! All operations are tenant-scoped.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use uuid::Uuid;

use super::Stores;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Role for persisted messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }

    fn from_db(s: &str) -> Result<Self> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "system" => Ok(Self::System),
            other => bail!("unknown message role from DB: {other:?}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: Uuid,
    pub tenant: String,
    pub title: Option<String>,
    pub collection: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct ConversationDbRow {
    id: Uuid,
    tenant: String,
    title: Option<String>,
    collection: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<ConversationDbRow> for ConversationRow {
    fn from(r: ConversationDbRow) -> Self {
        Self {
            id: r.id,
            tenant: r.tenant,
            title: r.title,
            collection: r.collection,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub metadata: Option<JsonValue>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct MessageDbRow {
    id: Uuid,
    conversation_id: Uuid,
    role: String,
    content: String,
    metadata: Option<JsonValue>,
    created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Store methods
// ---------------------------------------------------------------------------

impl Stores {
    /// Create a new conversation for the given tenant.
    pub async fn create_conversation(
        &self,
        tenant: &str,
        title: Option<&str>,
        collection: Option<&str>,
    ) -> Result<ConversationRow> {
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, ConversationDbRow>(
            "INSERT INTO conversations (id, tenant, title, collection)
             VALUES ($1, $2, $3, $4)
             RETURNING id, tenant, title, collection, created_at",
        )
        .bind(id)
        .bind(tenant)
        .bind(title)
        .bind(collection)
        .fetch_one(self.pg_pool())
        .await
        .context("creating conversation")?;

        Ok(row.into())
    }

    /// Get a conversation by ID, scoped to tenant.
    pub async fn get_conversation(
        &self,
        tenant: &str,
        conversation_id: Uuid,
    ) -> Result<Option<ConversationRow>> {
        let row = sqlx::query_as::<_, ConversationDbRow>(
            "SELECT id, tenant, title, collection, created_at
             FROM conversations
             WHERE id = $1 AND tenant = $2",
        )
        .bind(conversation_id)
        .bind(tenant)
        .fetch_optional(self.pg_pool())
        .await
        .context("getting conversation")?;

        Ok(row.map(ConversationRow::from))
    }

    /// Insert a message into a conversation, validating tenant ownership.
    ///
    /// Returns an error if the conversation does not exist or belongs to a
    /// different tenant.
    pub async fn insert_message(
        &self,
        tenant: &str,
        conversation_id: Uuid,
        role: MessageRole,
        content: &str,
        metadata: Option<JsonValue>,
    ) -> Result<MessageRow> {
        let id = Uuid::new_v4();
        let role_str = role.as_str();

        // Validate tenant ownership by selecting through the conversation.
        let row = sqlx::query_as::<_, MessageDbRow>(
            "INSERT INTO messages (id, conversation_id, role, content, metadata)
             SELECT $1, c.id, $2, $3, $4
             FROM conversations c
             WHERE c.id = $5 AND c.tenant = $6
             RETURNING id, conversation_id, role, content, metadata, created_at",
        )
        .bind(id)
        .bind(role_str)
        .bind(content)
        .bind(&metadata)
        .bind(conversation_id)
        .bind(tenant)
        .fetch_optional(self.pg_pool())
        .await
        .context("inserting message")?;

        match row {
            Some(db_row) => Ok(MessageRow {
                id: db_row.id,
                conversation_id: db_row.conversation_id,
                role: MessageRole::from_db(&db_row.role)?,
                content: db_row.content,
                metadata: db_row.metadata,
                created_at: db_row.created_at,
            }),
            None => bail!(
                "conversation not found for tenant: conversation_id={conversation_id}, \
                 tenant={tenant}"
            ),
        }
    }

    /// Atomically insert a user + assistant message pair in a single transaction.
    ///
    /// Ensures both messages are persisted together — if one insert fails, neither
    /// is committed. Validates tenant ownership on the first insert; the second
    /// reuses the same transaction (and therefore the same tenant check).
    pub async fn insert_chat_turn(
        &self,
        tenant: &str,
        conversation_id: Uuid,
        user_content: &str,
        assistant_content: &str,
    ) -> Result<(MessageRow, MessageRow)> {
        let mut tx = self.pg_pool().begin().await.context("starting chat turn transaction")?;

        let user_id = Uuid::new_v4();
        let user_row = sqlx::query_as::<_, MessageDbRow>(
            "INSERT INTO messages (id, conversation_id, role, content, metadata)
             SELECT $1, c.id, $2, $3, NULL
             FROM conversations c
             WHERE c.id = $4 AND c.tenant = $5
             RETURNING id, conversation_id, role, content, metadata, created_at",
        )
        .bind(user_id)
        .bind(MessageRole::User.as_str())
        .bind(user_content)
        .bind(conversation_id)
        .bind(tenant)
        .fetch_optional(&mut *tx)
        .await
        .context("inserting user message")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "conversation not found for tenant: conversation_id={conversation_id}, \
                 tenant={tenant}"
            )
        })?;

        let assistant_id = Uuid::new_v4();
        let assistant_row = sqlx::query_as::<_, MessageDbRow>(
            "INSERT INTO messages (id, conversation_id, role, content, metadata)
             VALUES ($1, $2, $3, $4, NULL)
             RETURNING id, conversation_id, role, content, metadata, created_at",
        )
        .bind(assistant_id)
        .bind(conversation_id)
        .bind(MessageRole::Assistant.as_str())
        .bind(assistant_content)
        .fetch_one(&mut *tx)
        .await
        .context("inserting assistant message")?;

        tx.commit().await.context("committing chat turn transaction")?;

        let user = MessageRow {
            id: user_row.id,
            conversation_id: user_row.conversation_id,
            role: MessageRole::from_db(&user_row.role)?,
            content: user_row.content,
            metadata: user_row.metadata,
            created_at: user_row.created_at,
        };
        let assistant = MessageRow {
            id: assistant_row.id,
            conversation_id: assistant_row.conversation_id,
            role: MessageRole::from_db(&assistant_row.role)?,
            content: assistant_row.content,
            metadata: assistant_row.metadata,
            created_at: assistant_row.created_at,
        };

        Ok((user, assistant))
    }

    /// Get the most recent N messages for a conversation, returned oldest-first.
    ///
    /// Validates `limit > 0`. Tenant is checked via join to `conversations`.
    pub async fn get_messages(
        &self,
        tenant: &str,
        conversation_id: Uuid,
        limit: i64,
    ) -> Result<Vec<MessageRow>> {
        if limit <= 0 {
            bail!("message limit must be > 0, got {limit}");
        }

        let rows = sqlx::query_as::<_, MessageDbRow>(
            "SELECT m.id, m.conversation_id, m.role, m.content, m.metadata, m.created_at
             FROM (
                 SELECT m2.*
                 FROM messages m2
                 JOIN conversations c ON c.id = m2.conversation_id
                 WHERE m2.conversation_id = $1 AND c.tenant = $2
                 ORDER BY m2.created_at DESC, m2.id DESC
                 LIMIT $3
             ) m
             ORDER BY m.created_at ASC, m.id ASC",
        )
        .bind(conversation_id)
        .bind(tenant)
        .bind(limit)
        .fetch_all(self.pg_pool())
        .await
        .context("getting messages")?;

        rows.into_iter()
            .map(|r| {
                Ok(MessageRow {
                    id: r.id,
                    conversation_id: r.conversation_id,
                    role: MessageRole::from_db(&r.role)?,
                    content: r.content,
                    metadata: r.metadata,
                    created_at: r.created_at,
                })
            })
            .collect()
    }
}
