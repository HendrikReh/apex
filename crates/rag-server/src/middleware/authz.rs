//! Authorization middleware — enforces capability checks per route.
//!
//! Uses a static route→capability mapping. Routes without a mapping inside
//! the protected router are denied by default (fail closed).

use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::roles::Capability;
use crate::state::{ApiError, RequestContext};

/// Authorization middleware. Must run after authentication.
pub async fn authorize(req: Request<axum::body::Body>, next: Next) -> Result<Response, ApiError> {
    let ctx = req.extensions().get::<RequestContext>().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "missing request context (authz middleware requires auth middleware)".into(),
    })?;

    if let Some(required) = required_capability(req.method(), req.uri().path())
        && !ctx.principal.has_capability(required)
    {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            message: format!("insufficient permissions: requires {required:?}",),
        });
    }

    Ok(next.run(req).await)
}

/// Map a request method+path to the required capability.
///
/// This is the single source of truth for the current route policy.
/// Add new entries here when new protected routes are added.
fn required_capability(method: &Method, path: &str) -> Option<Capability> {
    match (method, path) {
        // Ingest
        (&Method::POST, "/ingest" | "/ingest/upload") => Some(Capability::IngestWrite),
        // Search
        (&Method::POST, "/search/dense" | "/search/sparse" | "/search/hybrid") => {
            Some(Capability::SearchRead)
        }
        // Chat
        (&Method::POST, "/chat") => Some(Capability::ChatUse),
        // Collections
        (&Method::GET, p) if p.starts_with("/collections/") && p.ends_with("/stats") => {
            Some(Capability::CollectionsRead)
        }
        // API key management
        (&Method::POST, "/auth/service-accounts") => Some(Capability::AuthKeysManage),
        (&Method::POST, "/auth/api-keys") => Some(Capability::AuthKeysManage),
        (&Method::DELETE, p) if p.starts_with("/auth/api-keys/") => {
            Some(Capability::AuthKeysManage)
        }
        (&Method::GET, "/auth/api-keys") => Some(Capability::AuthKeysManage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_requires_ingest_write() {
        assert_eq!(required_capability(&Method::POST, "/ingest"), Some(Capability::IngestWrite),);
        assert_eq!(
            required_capability(&Method::POST, "/ingest/upload"),
            Some(Capability::IngestWrite),
        );
    }

    #[test]
    fn search_requires_search_read() {
        assert_eq!(
            required_capability(&Method::POST, "/search/dense"),
            Some(Capability::SearchRead),
        );
        assert_eq!(
            required_capability(&Method::POST, "/search/sparse"),
            Some(Capability::SearchRead),
        );
        assert_eq!(
            required_capability(&Method::POST, "/search/hybrid"),
            Some(Capability::SearchRead),
        );
    }

    #[test]
    fn chat_requires_chat_use() {
        assert_eq!(required_capability(&Method::POST, "/chat"), Some(Capability::ChatUse),);
    }

    #[test]
    fn collection_stats_requires_collections_read() {
        assert_eq!(
            required_capability(&Method::GET, "/collections/my-collection/stats"),
            Some(Capability::CollectionsRead),
        );
    }

    #[test]
    fn api_key_management_requires_auth_keys_manage() {
        assert_eq!(
            required_capability(&Method::POST, "/auth/api-keys"),
            Some(Capability::AuthKeysManage),
        );
        assert_eq!(
            required_capability(&Method::DELETE, "/auth/api-keys/some-id"),
            Some(Capability::AuthKeysManage),
        );
    }

    #[test]
    fn unknown_route_returns_none() {
        assert_eq!(required_capability(&Method::GET, "/unknown"), None);
    }

    #[test]
    fn wrong_method_returns_none() {
        assert_eq!(required_capability(&Method::GET, "/ingest"), None);
    }
}
