//! Rate limiting middleware — global and per-tenant/principal.
//!
//! Uses an in-memory backend behind a trait for future Redis support.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use dashmap::DashMap;
use tokio::sync::Semaphore;

use axum::response::IntoResponse;

use crate::state::{ApiError, AppState, RequestContext};

// ---------------------------------------------------------------------------
// Token bucket
// ---------------------------------------------------------------------------

/// Simple token bucket for rate limiting.
struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(rps: u32, burst: u32) -> Self {
        Self {
            rate: f64::from(rps),
            burst: f64::from(burst),
            tokens: f64::from(burst),
            last_refill: Instant::now(),
        }
    }

    fn try_acquire(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.last_refill = now;
    }
}

// ---------------------------------------------------------------------------
// Rate limiter state
// ---------------------------------------------------------------------------

/// In-memory rate limiter state held in AppState.
pub struct RateLimiterState {
    /// Global concurrency semaphore.
    pub global_semaphore: Arc<Semaphore>,
    /// Global RPS token bucket.
    global_bucket: Arc<parking_lot::Mutex<TokenBucket>>,
    /// Per-key token buckets (keyed by `{tenant}:{principal}:{route_class}`).
    /// NOTE: entries are never evicted — add TTL-based cleanup for long-running
    /// servers with many distinct tenants/principals.
    tenant_buckets: DashMap<String, parking_lot::Mutex<TokenBucket>>,
    /// Default per-tenant RPS and burst.
    tenant_rps: u32,
    tenant_burst: u32,
}

impl RateLimiterState {
    /// Create a new rate limiter from config values.
    pub fn new(
        global_rps: u32,
        global_burst: u32,
        global_concurrency: u32,
        tenant_rps: u32,
        tenant_burst: u32,
    ) -> Self {
        Self {
            global_semaphore: Arc::new(Semaphore::new(global_concurrency as usize)),
            global_bucket: Arc::new(parking_lot::Mutex::new(TokenBucket::new(
                global_rps,
                global_burst,
            ))),
            tenant_buckets: DashMap::new(),
            tenant_rps,
            tenant_burst,
        }
    }

    /// Check global rate limit. Returns `true` if allowed.
    pub fn check_global(&self) -> bool {
        self.global_bucket.lock().try_acquire()
    }

    /// Check per-tenant/principal rate limit. Returns `true` if allowed.
    pub fn check_tenant(&self, key: &str) -> bool {
        let entry = self.tenant_buckets.entry(key.to_string()).or_insert_with(|| {
            parking_lot::Mutex::new(TokenBucket::new(self.tenant_rps, self.tenant_burst))
        });
        entry.lock().try_acquire()
    }
}

impl Default for RateLimiterState {
    fn default() -> Self {
        Self::new(1000, 200, 100, 100, 50)
    }
}

// ---------------------------------------------------------------------------
// Route class
// ---------------------------------------------------------------------------

/// Classify a request path into a rate-limit route class.
fn route_class(path: &str) -> &'static str {
    if path.starts_with("/search/") {
        "search"
    } else if path == "/chat" {
        "chat"
    } else if path.starts_with("/ingest") {
        "ingest"
    } else {
        "default"
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Build a 429 response with a `Retry-After` header.
#[allow(clippy::disallowed_methods)] // serde_json::json! internally uses .expect()
fn too_many_requests(message: &str) -> Response {
    let body = serde_json::json!({ "error": message });
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (axum::http::header::CONTENT_TYPE, "application/json"),
            (axum::http::header::RETRY_AFTER, "1"),
        ],
        body.to_string(),
    )
        .into_response()
}

/// Rate limiting middleware. Must run after authentication.
pub async fn rate_limit(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    // Global rate check
    if !state.auth.rate_limiter.check_global() {
        return Ok(too_many_requests("global rate limit exceeded"));
    }

    // Per-tenant/principal rate check
    if let Some(ctx) = req.extensions().get::<RequestContext>() {
        let class = route_class(req.uri().path());
        let key = format!("{}:{}:{}", ctx.tenant.as_str(), ctx.principal.rate_limit_key, class);
        if !state.auth.rate_limiter.check_tenant(&key) {
            return Ok(too_many_requests("rate limit exceeded"));
        }
    }

    // Global concurrency check
    let Ok(_permit) = state.auth.rate_limiter.global_semaphore.clone().try_acquire_owned() else {
        return Ok(too_many_requests("server at maximum concurrency"));
    };

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_allows_burst() {
        let mut bucket = TokenBucket::new(10, 5);
        for _ in 0..5 {
            assert!(bucket.try_acquire());
        }
        assert!(!bucket.try_acquire());
    }

    #[test]
    fn rate_limiter_state_global_check() {
        let limiter = RateLimiterState::new(10, 5, 100, 10, 5);
        for _ in 0..5 {
            assert!(limiter.check_global());
        }
        assert!(!limiter.check_global());
    }

    #[test]
    fn rate_limiter_state_tenant_isolation() {
        let limiter = RateLimiterState::new(1000, 1000, 100, 2, 2);
        assert!(limiter.check_tenant("tenant-a:sa-1:search"));
        assert!(limiter.check_tenant("tenant-a:sa-1:search"));
        assert!(!limiter.check_tenant("tenant-a:sa-1:search"));
        // Different key should still have budget
        assert!(limiter.check_tenant("tenant-b:sa-2:search"));
    }

    #[test]
    fn route_class_mapping() {
        assert_eq!(route_class("/search/dense"), "search");
        assert_eq!(route_class("/search/hybrid"), "search");
        assert_eq!(route_class("/chat"), "chat");
        assert_eq!(route_class("/ingest"), "ingest");
        assert_eq!(route_class("/ingest/upload"), "ingest");
        assert_eq!(route_class("/collections/foo/stats"), "default");
    }
}
