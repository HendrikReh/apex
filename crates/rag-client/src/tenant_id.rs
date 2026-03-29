//! Tenant identity type with validation (client-local).
//!
//! Mirrors the validation rules of `rag_core::TenantId` but is defined
//! independently so `rag-client` has no workspace dependencies.

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

/// Maximum length for a tenant identifier.
const MAX_TENANT_LENGTH: usize = 128;

/// A validated tenant identifier.
///
/// Guarantees: non-empty, max 128 chars, ASCII alphanumeric + `-` + `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Create a new `TenantId`, validating the input.
    pub fn new(s: impl Into<String>) -> Result<Self, String> {
        let s = s.into();
        if s.is_empty() {
            return Err("tenant ID must not be empty".into());
        }
        if s.len() > MAX_TENANT_LENGTH {
            return Err(format!("tenant ID exceeds {MAX_TENANT_LENGTH} characters"));
        }
        for (i, c) in s.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
                return Err(format!("invalid character '{c}' at position {i}"));
            }
        }
        Ok(Self(s))
    }

    /// Get the tenant ID as a string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TenantId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Deref for TenantId {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for TenantId {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids() {
        for id in ["default", "my-tenant", "tenant_123", "A"] {
            assert!(TenantId::new(id).is_ok(), "expected {id:?} to be valid");
        }
    }

    #[test]
    fn empty_rejected() {
        assert!(TenantId::new("").is_err());
    }

    #[test]
    fn too_long_rejected() {
        assert!(TenantId::new("a".repeat(129)).is_err());
        assert!(TenantId::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn invalid_chars_rejected() {
        assert!(TenantId::new("has spaces").is_err());
        assert!(TenantId::new("has/slash").is_err());
        assert!(TenantId::new("has.dot").is_err());
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn display_and_deref() {
        let t = TenantId::new("acme").expect("test value");
        assert_eq!(format!("{t}"), "acme");
        assert_eq!(t.as_str(), "acme");
        assert!(t.starts_with("ac"));
    }
}
