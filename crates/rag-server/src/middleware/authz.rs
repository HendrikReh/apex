//! Authorization middleware — enforces capability checks per route.
//!
//! Uses a static route→capability mapping. Routes without a mapping inside
//! the protected router are denied by default (fail closed).

use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::roles::Capability;
use crate::state::{ApiError, RequestContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutePolicy {
    Authorized(Capability),
    MethodNotAllowed,
    Unmapped,
}

/// Authorization middleware. Must run after authentication.
pub async fn authorize(req: Request<axum::body::Body>, next: Next) -> Result<Response, ApiError> {
    let ctx = req.extensions().get::<RequestContext>().ok_or_else(|| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "missing request context (authz middleware requires auth middleware)".into(),
    })?;

    match route_policy(req.method(), req.uri().path()) {
        RoutePolicy::Authorized(required) => {
            if !ctx.principal.has_capability(required) {
                return Err(ApiError {
                    status: StatusCode::FORBIDDEN,
                    message: format!("insufficient permissions: requires {required:?}",),
                });
            }
        }
        RoutePolicy::MethodNotAllowed => {}
        RoutePolicy::Unmapped => {
            return Err(ApiError {
                status: StatusCode::FORBIDDEN,
                message: "route is not authorized until an authz policy is added".into(),
            });
        }
    }

    Ok(next.run(req).await)
}

/// Single source of truth for protected-route policies.
///
/// Each entry maps a (method, path-pattern) to a capability. The path matching
/// is done in `route_policy`, which also handles the method-not-allowed and
/// unmapped cases. When adding a new protected route, add ONE entry here.
struct RouteEntry {
    method: &'static Method,
    capability: Capability,
}

/// Static route table. Path matching is handled by `route_policy` below.
/// Keep entries grouped by subsystem.
const STATIC_ROUTES: &[(&str, RouteEntry)] = &[
    // Agents
    ("/agents", RouteEntry { method: &Method::GET, capability: Capability::AgentsOperate }),
    ("/runs", RouteEntry { method: &Method::GET, capability: Capability::AgentsOperate }),
    // Ingest
    ("/ingest", RouteEntry { method: &Method::POST, capability: Capability::IngestWrite }),
    ("/ingest/upload", RouteEntry { method: &Method::POST, capability: Capability::IngestWrite }),
    // Search
    ("/search/dense", RouteEntry { method: &Method::POST, capability: Capability::SearchRead }),
    ("/search/sparse", RouteEntry { method: &Method::POST, capability: Capability::SearchRead }),
    ("/search/hybrid", RouteEntry { method: &Method::POST, capability: Capability::SearchRead }),
    // Chat
    ("/chat", RouteEntry { method: &Method::POST, capability: Capability::ChatUse }),
    // API key management
    (
        "/auth/service-accounts",
        RouteEntry { method: &Method::POST, capability: Capability::AuthKeysManage },
    ),
    (
        "/auth/api-keys",
        RouteEntry { method: &Method::POST, capability: Capability::AuthKeysManage },
    ),
    ("/auth/api-keys", RouteEntry { method: &Method::GET, capability: Capability::AuthKeysManage }),
];

/// Dynamic route patterns that cannot be expressed as static strings.
fn dynamic_route(method: &Method, path: &str) -> Option<Capability> {
    match (method, path) {
        (&Method::GET, p) if p.starts_with("/agents/") && !p.ends_with("/execute") => {
            Some(Capability::AgentsOperate)
        }
        (&Method::POST, p) if p.starts_with("/agents/") && p.ends_with("/execute") => {
            Some(Capability::AgentsOperate)
        }
        (&Method::GET, p) if p.starts_with("/runs/") => Some(Capability::AgentsOperate),
        (&Method::POST, p) if p.starts_with("/runs/") && p.ends_with("/approve") => {
            Some(Capability::AgentsOperate)
        }
        (&Method::POST, p) if p.starts_with("/runs/") && p.ends_with("/reject") => {
            Some(Capability::AgentsOperate)
        }
        (&Method::GET, p) if p.starts_with("/collections/") && p.ends_with("/stats") => {
            Some(Capability::CollectionsRead)
        }
        (&Method::DELETE, p) if p.starts_with("/auth/api-keys/") => {
            Some(Capability::AuthKeysManage)
        }
        _ => None,
    }
}

fn is_dynamic_path(path: &str) -> bool {
    (path.starts_with("/agents/"))
        || (path.starts_with("/runs/"))
        || (path.starts_with("/collections/") && path.ends_with("/stats"))
        || path.starts_with("/auth/api-keys/")
}

fn route_policy(method: &Method, path: &str) -> RoutePolicy {
    let effective_method = if *method == Method::HEAD { &Method::GET } else { method };

    // Check static routes first.
    let mut path_known = false;
    for (route_path, entry) in STATIC_ROUTES {
        if *route_path == path {
            path_known = true;
            if entry.method == effective_method {
                return RoutePolicy::Authorized(entry.capability);
            }
        }
    }

    // Check dynamic patterns.
    if let Some(cap) = dynamic_route(effective_method, path) {
        return RoutePolicy::Authorized(cap);
    }
    if is_dynamic_path(path) {
        path_known = true;
    }

    if path_known { RoutePolicy::MethodNotAllowed } else { RoutePolicy::Unmapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_requires_ingest_write() {
        assert_eq!(
            route_policy(&Method::POST, "/ingest"),
            RoutePolicy::Authorized(Capability::IngestWrite),
        );
        assert_eq!(
            route_policy(&Method::POST, "/ingest/upload"),
            RoutePolicy::Authorized(Capability::IngestWrite),
        );
    }

    #[test]
    fn agents_require_agents_operate() {
        assert_eq!(
            route_policy(&Method::GET, "/agents"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::HEAD, "/agents"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::GET, "/agents/rag_spike"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::HEAD, "/agents/rag_spike"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::POST, "/agents/rag_spike/execute"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::GET, "/runs"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::HEAD, "/runs"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::GET, "/runs/00000000-0000-0000-0000-000000000000"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::HEAD, "/runs/00000000-0000-0000-0000-000000000000"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
        assert_eq!(
            route_policy(&Method::POST, "/runs/00000000-0000-0000-0000-000000000000/approve"),
            RoutePolicy::Authorized(Capability::AgentsOperate),
        );
    }

    #[test]
    fn search_requires_search_read() {
        assert_eq!(
            route_policy(&Method::POST, "/search/dense"),
            RoutePolicy::Authorized(Capability::SearchRead),
        );
        assert_eq!(
            route_policy(&Method::POST, "/search/sparse"),
            RoutePolicy::Authorized(Capability::SearchRead),
        );
        assert_eq!(
            route_policy(&Method::POST, "/search/hybrid"),
            RoutePolicy::Authorized(Capability::SearchRead),
        );
    }

    #[test]
    fn chat_requires_chat_use() {
        assert_eq!(
            route_policy(&Method::POST, "/chat"),
            RoutePolicy::Authorized(Capability::ChatUse),
        );
    }

    #[test]
    fn collection_stats_requires_collections_read() {
        assert_eq!(
            route_policy(&Method::GET, "/collections/my-collection/stats"),
            RoutePolicy::Authorized(Capability::CollectionsRead),
        );
    }

    #[test]
    fn api_key_management_requires_auth_keys_manage() {
        assert_eq!(
            route_policy(&Method::POST, "/auth/api-keys"),
            RoutePolicy::Authorized(Capability::AuthKeysManage),
        );
        assert_eq!(
            route_policy(&Method::DELETE, "/auth/api-keys/some-id"),
            RoutePolicy::Authorized(Capability::AuthKeysManage),
        );
        assert_eq!(
            route_policy(&Method::GET, "/auth/api-keys"),
            RoutePolicy::Authorized(Capability::AuthKeysManage),
        );
    }

    #[test]
    fn unknown_route_is_unmapped() {
        assert_eq!(route_policy(&Method::GET, "/unknown"), RoutePolicy::Unmapped);
    }

    #[test]
    fn wrong_method_on_static_route_is_method_not_allowed() {
        assert_eq!(route_policy(&Method::GET, "/ingest"), RoutePolicy::MethodNotAllowed);
    }

    #[test]
    fn wrong_method_on_dynamic_route_is_method_not_allowed() {
        assert_eq!(
            route_policy(&Method::POST, "/collections/test/stats"),
            RoutePolicy::MethodNotAllowed,
        );
    }
}
