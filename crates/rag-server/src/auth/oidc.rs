//! OIDC JWT validation and JWKS management.
//!
//! Supports RS256, RS384, RS512 only. Rejects `alg=none` and symmetric algorithms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// JWKS types
// ---------------------------------------------------------------------------

/// A JSON Web Key from the JWKS endpoint.
#[derive(Debug, Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    #[allow(dead_code)]
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// JWKS response from the issuer.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// OIDC discovery document (only the fields we need).
#[derive(Debug, Deserialize)]
struct OidcDiscovery {
    jwks_uri: String,
}

// ---------------------------------------------------------------------------
// JWKS Cache
// ---------------------------------------------------------------------------

/// Cached JWKS state with bounded TTL.
pub struct JwksCache {
    inner: Arc<RwLock<CacheState>>,
    http: reqwest::Client,
    ttl: Duration,
}

struct CacheState {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
    #[allow(dead_code)]
    jwks_url: Option<String>,
}

impl Default for JwksCache {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(CacheState {
                keys: HashMap::new(),
                fetched_at: None,
                jwks_url: None,
            })),
            http: reqwest::Client::new(),
            ttl: Duration::from_secs(3600),
        }
    }
}

impl JwksCache {
    /// Get the decoding key for a given `kid`, refreshing the cache if needed.
    pub async fn get_key(
        &self,
        kid: &str,
        issuer: &str,
        jwks_url_override: Option<&str>,
    ) -> Result<DecodingKey> {
        // Check cache first — read lock is explicitly dropped before any
        // subsequent await so the future stays Send.
        let cached = {
            let state = self.inner.read().await;
            let still_fresh = state.fetched_at.map_or(false, |t| t.elapsed() < self.ttl);
            if still_fresh { state.keys.get(kid).cloned() } else { None }
        };
        if let Some(key) = cached {
            return Ok(key);
        }

        // Cache miss or expired — refresh
        self.refresh(issuer, jwks_url_override).await?;

        let key = {
            let state = self.inner.read().await;
            state.keys.get(kid).cloned()
        };
        key.ok_or_else(|| anyhow::anyhow!("no JWKS key found for kid={kid:?}"))
    }

    /// Fetch JWKS and update the cache.
    async fn refresh(&self, issuer: &str, jwks_url_override: Option<&str>) -> Result<()> {
        let jwks_url = if let Some(url) = jwks_url_override {
            url.to_string()
        } else {
            self.discover_jwks_url(issuer).await?
        };

        let jwks: JwksResponse = self
            .http
            .get(&jwks_url)
            .send()
            .await
            .context("fetching JWKS")?
            .json()
            .await
            .context("parsing JWKS response")?;

        let mut keys = HashMap::new();
        for jwk in &jwks.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            let kid = match &jwk.kid {
                Some(k) => k.clone(),
                None => continue,
            };
            let n = match &jwk.n {
                Some(n) => n,
                None => continue,
            };
            let e = match &jwk.e {
                Some(e) => e,
                None => continue,
            };
            if let Ok(dk) = DecodingKey::from_rsa_components(n, e) {
                keys.insert(kid, dk);
            }
        }

        let mut state = self.inner.write().await;
        state.keys = keys;
        state.fetched_at = Some(Instant::now());
        state.jwks_url = Some(jwks_url);
        Ok(())
    }

    /// Discover JWKS URL from OIDC well-known endpoint.
    async fn discover_jwks_url(&self, issuer: &str) -> Result<String> {
        let discovery_url =
            format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
        let discovery: OidcDiscovery = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .with_context(|| format!("fetching OIDC discovery from {discovery_url}"))?
            .json()
            .await
            .context("parsing OIDC discovery response")?;
        Ok(discovery.jwks_uri)
    }
}

// ---------------------------------------------------------------------------
// JWT Validation
// ---------------------------------------------------------------------------

/// Claims we extract from the OIDC JWT.
#[derive(Debug, Deserialize)]
pub struct ApexClaims {
    pub sub: String,
    pub iss: String,
    #[serde(default)]
    pub groups: Vec<String>,
}

/// Allowed algorithms — asymmetric RSA only.
const ALLOWED_ALGORITHMS: &[Algorithm] = &[Algorithm::RS256, Algorithm::RS384, Algorithm::RS512];

/// Validate an OIDC JWT and return the decoded claims.
pub async fn validate_token(
    token: &str,
    issuer: &str,
    audience: &str,
    groups_claim: &str,
    jwks_cache: &JwksCache,
    jwks_url_override: Option<&str>,
) -> Result<ApexClaims> {
    let header = decode_header(token).context("decoding JWT header")?;

    if !ALLOWED_ALGORITHMS.contains(&header.alg) {
        bail!("unsupported JWT algorithm: {:?} (allowed: RS256, RS384, RS512)", header.alg);
    }

    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("JWT header missing `kid` — cannot look up signing key"))?;

    let decoding_key = jwks_cache
        .get_key(kid, issuer, jwks_url_override)
        .await
        .context("resolving JWKS signing key")?;

    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);

    let token_data: TokenData<serde_json::Value> =
        decode(token, &decoding_key, &validation).context("validating JWT")?;

    let sub = token_data
        .claims
        .get("sub")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("JWT missing `sub` claim"))?
        .to_string();

    let iss = token_data
        .claims
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("JWT missing `iss` claim"))?
        .to_string();

    let groups: Vec<String> = token_data
        .claims
        .get(groups_claim)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    Ok(ApexClaims { sub, iss, groups })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_send<T: Send>() {}
    fn _assert_sync<T: Sync>() {}

    #[test]
    fn jwks_cache_is_send_sync() {
        _assert_send::<JwksCache>();
        _assert_sync::<JwksCache>();
    }

    #[test]
    fn validate_token_future_is_send() {
        fn assert_future_send<F: std::future::Future + Send>(_: F) {}
        let cache = JwksCache::default();
        assert_future_send(validate_token("tok", "iss", "aud", "groups", &cache, None));
    }

    #[test]
    fn jwks_cache_default_creates_empty() {
        let _cache = JwksCache::default();
    }

    #[test]
    fn allowed_algorithms_are_asymmetric_rsa() {
        assert!(ALLOWED_ALGORITHMS.contains(&Algorithm::RS256));
        assert!(ALLOWED_ALGORITHMS.contains(&Algorithm::RS384));
        assert!(ALLOWED_ALGORITHMS.contains(&Algorithm::RS512));
        assert!(!ALLOWED_ALGORITHMS.contains(&Algorithm::HS256));
    }
}
