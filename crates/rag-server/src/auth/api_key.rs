//! API key parsing, hashing, constant-time verification, and generation.
//!
//! Key format: `apex_<8-char-prefix>_<base64url-secret>`
//! - prefix: 8 random alphanumeric characters (stored plaintext for lookup)
//! - secret: 32 random bytes, base64url-encoded (hashed at rest)

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const KEY_MARKER: &str = "apex";
const PREFIX_LEN: usize = 8;
const SECRET_BYTES: usize = 32;
const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A parsed API key split into its components.
#[derive(Debug)]
pub struct ParsedApiKey {
    pub prefix: String,
    pub secret_raw: String,
}

/// Parse a presented token into prefix and secret components.
///
/// Expected format: `apex_<8-char-prefix>_<base64url-secret>`
pub fn parse(token: &str) -> Result<ParsedApiKey> {
    let parts: Vec<&str> = token.splitn(3, '_').collect();
    if parts.len() != 3 || parts[0] != KEY_MARKER {
        bail!("invalid API key format: must be apex_<prefix>_<secret>");
    }
    let prefix = parts[1];
    if prefix.len() != PREFIX_LEN {
        bail!("invalid API key prefix: expected {PREFIX_LEN} characters, got {}", prefix.len());
    }
    let secret = parts[2];
    if secret.is_empty() {
        bail!("invalid API key: empty secret");
    }
    Ok(ParsedApiKey { prefix: prefix.to_string(), secret_raw: secret.to_string() })
}

/// Hash the secret portion of an API key using SHA-256.
pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

/// Verify a presented secret against a stored hash using constant-time comparison.
pub fn verify_secret(secret: &str, stored_hash: &str) -> bool {
    let computed = hash_secret(secret);
    constant_time_eq(computed.as_bytes(), stored_hash.as_bytes())
}

/// Generate a new API key. Returns `(full_display_key, key_prefix, secret_hash)`.
///
/// The full display key is returned to the caller exactly once. Only the prefix
/// and hash should be persisted.
pub fn generate() -> (String, String, String) {
    let mut rng = rand::rng();

    let prefix: String = (0..PREFIX_LEN)
        .map(|_| {
            let idx = rng.random_range(0..ALPHANUMERIC.len());
            ALPHANUMERIC[idx] as char
        })
        .collect();

    let mut secret_bytes = [0u8; SECRET_BYTES];
    rng.fill(&mut secret_bytes);
    let secret_encoded = URL_SAFE_NO_PAD.encode(secret_bytes);

    let full_key = format!("{KEY_MARKER}_{prefix}_{secret_encoded}");
    let hash = hash_secret(&secret_encoded);

    (full_key, prefix, hash)
}

// ---------------------------------------------------------------------------
// DB operations
// ---------------------------------------------------------------------------

/// Row returned by API key lookup.
#[derive(Debug, sqlx::FromRow)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub service_account_id: Uuid,
    pub key_prefix: String,
    pub key_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Row returned by service account lookup.
#[derive(Debug, sqlx::FromRow)]
pub struct ServiceAccountRow {
    pub id: Uuid,
    pub tenant: String,
    pub name: String,
    pub role: String,
    pub disabled_at: Option<DateTime<Utc>>,
}

/// Look up an API key by its prefix. Returns the key row and associated
/// service account if found.
pub async fn lookup_by_prefix(
    pool: &PgPool,
    prefix: &str,
) -> Result<Option<(ApiKeyRow, ServiceAccountRow)>> {
    let row: Option<ApiKeyRow> = sqlx::query_as(
        "SELECT id, service_account_id, key_prefix, key_hash, expires_at, revoked_at \
         FROM api_keys WHERE key_prefix = $1",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await
    .context("querying api_keys by prefix")?;

    let key_row = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let sa: ServiceAccountRow = sqlx::query_as(
        "SELECT id, tenant, name, role, disabled_at FROM service_accounts WHERE id = $1",
    )
    .bind(key_row.service_account_id)
    .fetch_one(pool)
    .await
    .context("querying service_account for api key")?;

    Ok(Some((key_row, sa)))
}

/// Create a new service account. Returns its UUID.
pub async fn create_service_account(
    pool: &PgPool,
    tenant: &str,
    name: &str,
    role: &str,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO service_accounts (id, tenant, name, role) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(tenant)
        .bind(name)
        .bind(role)
        .execute(pool)
        .await
        .context("inserting service_account")?;
    Ok(id)
}

/// Insert a new API key row. Returns the key UUID.
pub async fn insert_api_key(
    pool: &PgPool,
    service_account_id: Uuid,
    prefix: &str,
    hash: &str,
    expires_at: Option<DateTime<Utc>>,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys (id, service_account_id, key_prefix, key_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(service_account_id)
    .bind(prefix)
    .bind(hash)
    .bind(expires_at)
    .execute(pool)
    .await
    .context("inserting api_key")?;
    Ok(id)
}

/// Revoke an API key, scoped to a specific tenant via service_accounts join.
pub async fn revoke_api_key_scoped(pool: &PgPool, key_id: Uuid, tenant: &str) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at = now() \
         WHERE id = $1 AND revoked_at IS NULL \
         AND service_account_id IN (SELECT id FROM service_accounts WHERE tenant = $2)",
    )
    .bind(key_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("revoking api_key (tenant-scoped)")?;
    Ok(result.rows_affected() > 0)
}

/// Verify a service account belongs to the given tenant. Returns Some(id) if valid.
pub async fn verify_service_account_tenant(
    pool: &PgPool,
    service_account_id: Uuid,
    tenant: &str,
) -> Result<Option<Uuid>> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM service_accounts WHERE id = $1 AND tenant = $2")
            .bind(service_account_id)
            .bind(tenant)
            .fetch_optional(pool)
            .await
            .context("verifying service_account tenant ownership")?;
    Ok(row.map(|(id,)| id))
}

/// Touch `last_used_at` for the given key. Best-effort — errors are logged, not propagated.
pub async fn touch_last_used(pool: &PgPool, key_id: Uuid) {
    let result = sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
        .bind(key_id)
        .execute(pool)
        .await;
    if let Err(e) = result {
        tracing::warn!(key_id = %key_id, error = %e, "failed to update last_used_at");
    }
}

/// Constant-time byte comparison to prevent timing side-channels.
///
/// The early-return on length mismatch is acceptable here because both
/// operands are always SHA-256 hex digests (64 bytes). If this function
/// is reused for variable-length inputs, replace with `subtle::ConstantTimeEq`.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_key() {
        let parsed = parse("apex_AbCdEfGh_c29tZXNlY3JldGRhdGFoZXJl").expect("should parse");
        assert_eq!(parsed.prefix, "AbCdEfGh");
        assert_eq!(parsed.secret_raw, "c29tZXNlY3JldGRhdGFoZXJl");
    }

    #[test]
    fn parse_rejects_wrong_marker() {
        assert!(parse("beta_AbCdEfGh_secret").is_err());
    }

    #[test]
    fn parse_rejects_short_prefix() {
        assert!(parse("apex_short_secret").is_err());
    }

    #[test]
    fn parse_rejects_empty_secret() {
        assert!(parse("apex_AbCdEfGh_").is_err());
    }

    #[test]
    fn parse_rejects_malformed_input() {
        assert!(parse("notakey").is_err());
        assert!(parse("").is_err());
        assert!(parse("apex_").is_err());
    }

    #[test]
    fn hash_is_deterministic() {
        let h1 = hash_secret("test-secret");
        let h2 = hash_secret("test-secret");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn verify_correct_secret() {
        let hash = hash_secret("my-secret-data");
        assert!(verify_secret("my-secret-data", &hash));
    }

    #[test]
    fn verify_wrong_secret() {
        let hash = hash_secret("my-secret-data");
        assert!(!verify_secret("wrong-secret", &hash));
    }

    #[test]
    fn generate_produces_valid_format() {
        let (full_key, prefix, hash) = generate();

        assert!(full_key.starts_with("apex_"));
        assert_eq!(prefix.len(), 8);
        assert_eq!(hash.len(), 64);

        // Full key should be parseable
        let parsed = parse(&full_key).expect("generated key should parse");
        assert_eq!(parsed.prefix, prefix);

        // Hash should verify
        assert!(verify_secret(&parsed.secret_raw, &hash));
    }

    #[test]
    fn generate_produces_unique_keys() {
        let (key1, _, _) = generate();
        let (key2, _, _) = generate();
        assert_ne!(key1, key2);
    }

    #[test]
    fn constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"hello", b"hi"));
    }

    #[test]
    fn constant_time_eq_different_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }
}
