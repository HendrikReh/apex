//! Authentication middleware — resolves credentials into a Principal.
//!
//! Evaluates `auth_mode` from config and dispatches to the appropriate
//! authenticator (none, api-key, oidc). Updates the RequestContext in
//! request extensions with the resolved principal.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use rag_core::TenantId;
use rag_core::config::AuthMode;

use crate::auth::api_key;
use crate::auth::oidc;
use crate::auth::principal::Principal;
use crate::auth::roles::Role;
use crate::state::{ApiError, AppState, RequestContext};

/// Authentication middleware. Must run after tenant extraction.
pub async fn authenticate(
    State(state): State<Arc<AppState>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let ctx = req.extensions().get::<RequestContext>().cloned().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "missing request context (auth middleware requires tenant middleware)".into(),
    })?;

    let auth_mode = state.config.auth_mode.clone();
    let principal = match auth_mode {
        AuthMode::None => Principal::anonymous(ctx.tenant.clone()),
        AuthMode::ApiKey => {
            let token = extract_token(&req).ok_or_else(|| ApiError {
                status: StatusCode::UNAUTHORIZED,
                message: "missing API key".into(),
            })?;
            let parsed = api_key::parse(&token).map_err(|_| ApiError {
                status: StatusCode::UNAUTHORIZED,
                message: "invalid API key format".into(),
            })?;
            let pool = state.stores.pg_pool();
            let lookup = api_key::lookup_by_prefix(pool, &parsed.prefix).await.map_err(|e| {
                tracing::error!(error = %e, "API key lookup failed");
                ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "authentication error".into(),
                }
            })?;
            let (key_row, sa_row) = lookup.ok_or_else(|| ApiError {
                status: StatusCode::UNAUTHORIZED,
                message: "invalid API key".into(),
            })?;
            if !api_key::verify_secret(&parsed.secret_raw, &key_row.key_hash) {
                return Err(ApiError {
                    status: StatusCode::UNAUTHORIZED,
                    message: "invalid API key".into(),
                });
            }
            if key_row.revoked_at.is_some() {
                return Err(ApiError {
                    status: StatusCode::UNAUTHORIZED,
                    message: "API key has been revoked".into(),
                });
            }
            if let Some(expires) = key_row.expires_at {
                if expires < chrono::Utc::now() {
                    return Err(ApiError {
                        status: StatusCode::UNAUTHORIZED,
                        message: "API key has expired".into(),
                    });
                }
            }
            if sa_row.disabled_at.is_some() {
                return Err(ApiError {
                    status: StatusCode::UNAUTHORIZED,
                    message: "service account is disabled".into(),
                });
            }
            let role: Role = sa_row.role.parse().map_err(|e| {
                tracing::error!(error = %e, role = %sa_row.role, "invalid role in service_account");
                ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "authentication error".into(),
                }
            })?;
            let pool_clone = pool.clone();
            let key_id = key_row.id;
            tokio::spawn(async move { api_key::touch_last_used(&pool_clone, key_id).await });
            // Bind principal to the service account's DB tenant, NOT the
            // request header tenant. The has_tenant_access check below will
            // reject cross-tenant key usage.
            let sa_tenant = TenantId::new(&sa_row.tenant).map_err(|e| {
                tracing::error!(tenant = %sa_row.tenant, error = %e, "invalid tenant in service_account row");
                ApiError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "authentication error".into(),
                }
            })?;
            Principal::from_api_key(
                key_row.id,
                key_row.service_account_id,
                &key_row.key_prefix,
                sa_tenant,
                role,
            )
        }
        AuthMode::Oidc => {
            let token = extract_token(&req).ok_or_else(|| ApiError {
                status: StatusCode::UNAUTHORIZED,
                message: "missing Authorization: Bearer <jwt>".into(),
            })?;
            authenticate_oidc(state.clone(), token, ctx.tenant.clone()).await?
        }
    };

    // Validate tenant access
    if !principal.has_tenant_access(&ctx.tenant) {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            message: "principal does not have access to this tenant".into(),
        });
    }

    // Update the request context with the authenticated principal
    let updated_ctx = RequestContext { request_id: ctx.request_id, tenant: ctx.tenant, principal };
    req.extensions_mut().insert(updated_ctx);

    Ok(next.run(req).await)
}

/// Extract a bearer token from Authorization header or X-Api-Key header.
fn extract_token(req: &Request<axum::body::Body>) -> Option<String> {
    // Try Authorization: Bearer <token>
    if let Some(auth) = req.headers().get("authorization") {
        if let Ok(value) = auth.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return Some(token.trim().to_string());
            }
        }
    }
    // Try X-Api-Key: <token>
    if let Some(key) = req.headers().get("x-api-key") {
        if let Ok(value) = key.to_str() {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Authenticate via OIDC JWT.
///
/// Takes `Arc<AppState>` by value (not `&AppState`) so the compiler can prove the
/// returned future is `Send` — no borrowed references span await points.
async fn authenticate_oidc(
    state: Arc<AppState>,
    token: String,
    tenant: TenantId,
) -> Result<Principal, ApiError> {
    let issuer = state.config.oidc_issuer.clone().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "OIDC issuer not configured".into(),
    })?;
    let audience = state.config.oidc_audience.clone().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "OIDC audience not configured".into(),
    })?;
    let groups_claim = state.config.oidc_groups_claim.clone();
    let jwks_url = state.config.oidc_jwks_url.clone();
    let platform_groups = state.config.oidc_platform_operator_groups.clone();
    let pool = state.stores.pg_pool().clone();

    let claims = oidc::validate_token(
        &token,
        &issuer,
        &audience,
        &groups_claim,
        &state.auth.jwks_cache,
        jwks_url.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "OIDC token validation failed");
        ApiError { status: StatusCode::UNAUTHORIZED, message: "invalid or expired token".into() }
    })?;

    let is_platform = platform_groups.iter().any(|g| claims.groups.contains(g));
    let role = if is_platform {
        // Platform group membership grants PlatformOperator with full capabilities
        Role::PlatformOperator
    } else {
        lookup_oidc_role(&pool, &tenant, &claims.iss, &claims.sub).await?
    };

    Ok(Principal::from_oidc(&claims.iss, &claims.sub, tenant, role, is_platform))
}

/// Look up the OIDC principal's role from the database.
/// Falls back to `Viewer` if no mapping exists (open-read model).
/// Fails closed on DB errors rather than silently granting access.
async fn lookup_oidc_role(
    pool: &sqlx::PgPool,
    tenant: &TenantId,
    issuer: &str,
    subject: &str,
) -> Result<Role, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM oidc_principals \
         WHERE tenant = $1 AND issuer = $2 AND subject = $3 AND disabled_at IS NULL",
    )
    .bind(tenant.as_str())
    .bind(issuer)
    .bind(subject)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "OIDC role lookup failed");
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "authentication error".into(),
        }
    })?;

    match row {
        Some((role_str,)) => role_str.parse().map_err(|e| {
            tracing::error!(role = %role_str, error = %e, "invalid role in oidc_principals");
            ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "authentication error".into(),
            }
        }),
        // No mapping — default to Viewer (open-read model)
        None => Ok(Role::Viewer),
    }
}
