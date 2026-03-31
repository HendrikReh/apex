//! API key management endpoints (admin only).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::api_key;
use crate::state::{ApiError, AppState, Ctx};

// ---------------------------------------------------------------------------
// Create service account
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateServiceAccountRequest {
    pub name: String,
    pub role: String,
}

#[derive(Serialize)]
pub struct CreateServiceAccountResponse {
    pub id: Uuid,
    pub name: String,
    pub role: String,
}

pub async fn create_service_account(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Json(body): Json<CreateServiceAccountRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate and normalize role
    let role: crate::auth::Role = body.role.parse().map_err(|e| ApiError {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid role: {e}"),
    })?;
    let role_str = role.to_string();

    let id = api_key::create_service_account(
        state.stores.pg_pool(),
        ctx.tenant.as_str(),
        &body.name,
        &role_str,
    )
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to create service account: {e}"),
    })?;

    Ok((
        StatusCode::CREATED,
        Json(CreateServiceAccountResponse { id, name: body.name, role: role_str }),
    ))
}

// ---------------------------------------------------------------------------
// Generate API key
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateApiKeyRequest {
    pub service_account_id: Uuid,
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize)]
pub struct CreateApiKeyResponse {
    pub id: Uuid,
    /// The full API key — returned exactly once.
    pub key: String,
    pub prefix: String,
    pub service_account_id: Uuid,
    pub expires_at: Option<String>,
}

pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the service account belongs to the requesting tenant
    api_key::verify_service_account_tenant(
        state.stores.pg_pool(),
        body.service_account_id,
        ctx.tenant.as_str(),
    )
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to verify service account: {e}"),
    })?
    .ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        message: "service account not found in this tenant".into(),
    })?;

    if let Some(days) = body.expires_in_days
        && days <= 0
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "expires_in_days must be positive".into(),
        });
    }

    let (full_key, prefix, hash) = api_key::generate();

    let expires_at =
        body.expires_in_days.map(|days| chrono::Utc::now() + chrono::Duration::days(days));

    let id = api_key::insert_api_key(
        state.stores.pg_pool(),
        body.service_account_id,
        &prefix,
        &hash,
        expires_at,
    )
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to create API key: {e}"),
    })?;

    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse {
            id,
            key: full_key,
            prefix,
            service_account_id: body.service_account_id,
            expires_at: expires_at.map(|t| t.to_rfc3339()),
        }),
    ))
}

// ---------------------------------------------------------------------------
// Revoke API key
// ---------------------------------------------------------------------------

pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
    Path(key_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let revoked =
        api_key::revoke_api_key_scoped(state.stores.pg_pool(), key_id, ctx.tenant.as_str())
            .await
            .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("failed to revoke API key: {e}"),
        })?;

    if revoked {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError {
            status: StatusCode::NOT_FOUND,
            message: "API key not found or already revoked".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// List API keys
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ApiKeyListItem {
    pub id: Uuid,
    pub prefix: String,
    pub service_account_id: Uuid,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    Ctx(ctx): Ctx,
) -> Result<impl IntoResponse, ApiError> {
    #[derive(sqlx::FromRow)]
    struct KeyRow {
        id: Uuid,
        key_prefix: String,
        service_account_id: Uuid,
        created_at: chrono::DateTime<chrono::Utc>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
        last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    let rows: Vec<KeyRow> = sqlx::query_as(
        "SELECT k.id, k.key_prefix, k.service_account_id, k.created_at, \
         k.expires_at, k.revoked_at, k.last_used_at \
         FROM api_keys k \
         JOIN service_accounts sa ON sa.id = k.service_account_id \
         WHERE sa.tenant = $1 \
         ORDER BY k.created_at DESC",
    )
    .bind(ctx.tenant.as_str())
    .fetch_all(state.stores.pg_pool())
    .await
    .map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("failed to list API keys: {e}"),
    })?;

    let items: Vec<ApiKeyListItem> = rows
        .into_iter()
        .map(|r| ApiKeyListItem {
            id: r.id,
            prefix: r.key_prefix,
            service_account_id: r.service_account_id,
            created_at: r.created_at.to_rfc3339(),
            expires_at: r.expires_at.map(|t| t.to_rfc3339()),
            revoked_at: r.revoked_at.map(|t| t.to_rfc3339()),
            last_used_at: r.last_used_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(Json(items))
}
