//! Tenant identity type with validation.
//!
//! `TenantId` is a newtype wrapper around `String` that enforces:
//! - Non-empty tenant identifiers
//! - Valid characters (alphanumeric, hyphens, underscores)
//! - Reasonable length limits (1-128 characters)
//!
//! This prevents accidental use of empty strings as tenant identifiers
//! and provides type safety at API boundaries.

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Maximum length for a tenant identifier.
pub const MAX_TENANT_LENGTH: usize = 128;

/// Error returned when tenant validation fails.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TenantIdError {
    #[error("tenant ID must not be empty")]
    Empty,
    #[error("tenant ID exceeds 128 characters")]
    TooLong,
    #[error("invalid character '{0}' at position {1}")]
    InvalidCharacter(char, usize),
}

/// A validated tenant identifier.
///
/// `TenantId` guarantees:
/// - Non-empty (at least 1 character)
/// - Maximum 128 characters
/// - Only ASCII alphanumeric characters, hyphens (`-`), and underscores (`_`)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    /// Create a new `TenantId` from a string, validating the input.
    ///
    /// # Errors
    ///
    /// Returns `TenantIdError` if:
    /// - The string is empty
    /// - The string exceeds 128 characters
    /// - The string contains characters other than ASCII alphanumeric, hyphen, or underscore
    pub fn new(s: impl Into<String>) -> Result<Self, TenantIdError> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    /// Validate a tenant string without creating a `TenantId`.
    ///
    /// # Errors
    ///
    /// Returns `TenantIdError` if validation fails.
    pub fn validate(s: &str) -> Result<(), TenantIdError> {
        if s.is_empty() {
            return Err(TenantIdError::Empty);
        }
        if s.len() > MAX_TENANT_LENGTH {
            return Err(TenantIdError::TooLong);
        }
        for (i, c) in s.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
                return Err(TenantIdError::InvalidCharacter(c, i));
            }
        }
        Ok(())
    }

    /// Get the tenant ID as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Create a `TenantId` for the default tenant (`"default"`).
    ///
    /// This is the recommended way to get a tenant ID when the caller
    /// doesn't specify one explicitly.
    pub fn default_tenant() -> Self {
        // SAFETY: "default" is a known-valid tenant identifier.
        // Using match instead of .unwrap() to satisfy clippy disallowed_methods.
        match Self::new("default") {
            Ok(tenant) => tenant,
            Err(err) => {
                panic!("\"default\" must be a valid tenant identifier: {err}");
            }
        }
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TenantId {
    type Err = TenantIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for TenantId {
    type Error = TenantIdError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for TenantId {
    type Error = TenantIdError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<TenantId> for String {
    fn from(tenant: TenantId) -> Self {
        tenant.0
    }
}

impl AsRef<str> for TenantId {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for TenantId {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TenantId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        TenantId::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_tenant_ids() {
        let cases = ["default", "my-tenant", "tenant_123", "A"];
        for id in cases {
            assert!(TenantId::new(id).is_ok(), "expected {id:?} to be valid");
        }
    }

    #[test]
    fn empty_string_rejected() {
        assert_eq!(TenantId::new(""), Err(TenantIdError::Empty));
    }

    #[test]
    fn too_long_rejected() {
        let long = "a".repeat(129);
        assert_eq!(TenantId::new(long), Err(TenantIdError::TooLong));

        // Exactly at the limit should succeed.
        let at_limit = "a".repeat(128);
        assert!(TenantId::new(at_limit).is_ok());
    }

    #[test]
    fn invalid_characters_rejected() {
        let result = TenantId::new("has spaces");
        assert_eq!(result, Err(TenantIdError::InvalidCharacter(' ', 3)));

        let result = TenantId::new("has/slash");
        assert_eq!(result, Err(TenantIdError::InvalidCharacter('/', 3)));
    }

    #[test]
    fn default_tenant_returns_default() {
        let tenant = TenantId::default_tenant();
        assert_eq!(tenant.as_str(), "default");
    }

    #[test]
    fn from_str_roundtrip() {
        let tenant: TenantId = match "my-tenant".parse() {
            Ok(t) => t,
            Err(e) => panic!("parse failed: {e}"),
        };
        assert_eq!(tenant.to_string(), "my-tenant");
    }

    #[test]
    fn serde_roundtrip() {
        let tenant = match TenantId::new("my-tenant") {
            Ok(t) => t,
            Err(e) => panic!("new failed: {e}"),
        };
        let json = match serde_json::to_string(&tenant) {
            Ok(j) => j,
            Err(e) => panic!("serialize failed: {e}"),
        };
        assert_eq!(json, "\"my-tenant\"");

        let parsed: TenantId = match serde_json::from_str(&json) {
            Ok(t) => t,
            Err(e) => panic!("deserialize failed: {e}"),
        };
        assert_eq!(parsed, tenant);
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let result: Result<TenantId, _> = serde_json::from_str("\"\"");
        assert!(result.is_err());

        let result: Result<TenantId, _> = serde_json::from_str("\"has spaces\"");
        assert!(result.is_err());
    }

    #[test]
    fn try_from_string() {
        let tenant = TenantId::try_from(String::from("valid-id"));
        assert!(tenant.is_ok());

        let tenant = TenantId::try_from(String::from(""));
        assert!(tenant.is_err());
    }

    #[test]
    fn try_from_str_ref() {
        let tenant = TenantId::try_from("valid-id");
        assert!(tenant.is_ok());

        let tenant = TenantId::try_from("");
        assert!(tenant.is_err());
    }

    #[test]
    fn display_shows_inner_string() {
        let tenant = match TenantId::new("acme-corp") {
            Ok(t) => t,
            Err(e) => panic!("new failed: {e}"),
        };
        assert_eq!(format!("{tenant}"), "acme-corp");
    }

    #[test]
    fn deref_to_str() {
        let tenant = match TenantId::new("acme-corp") {
            Ok(t) => t,
            Err(e) => panic!("new failed: {e}"),
        };
        assert!(tenant.starts_with("acme"));
        assert_eq!(tenant.len(), 9);
    }
}
