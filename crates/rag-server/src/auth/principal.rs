//! Core identity types for all authenticated actors.

use std::collections::BTreeSet;

use rag_core::TenantId;
use uuid::Uuid;

use super::roles::{Capability, Role};

/// The resolved identity of a request's actor.
#[derive(Debug, Clone)]
pub struct Principal {
    pub subject: Subject,
    pub tenant_scope: TenantScope,
    pub roles: BTreeSet<Role>,
    pub capabilities: BTreeSet<Capability>,
    pub auth_method: AuthMethod,
    pub rate_limit_key: String,
}

/// Who the actor is.
#[derive(Debug, Clone)]
pub enum Subject {
    /// No authentication — local development only.
    Anonymous,
    /// Machine-to-machine API key.
    ApiKey { key_id: Uuid, service_account_id: Uuid, key_prefix: String },
    /// Human user authenticated via OIDC.
    Oidc { issuer: String, subject: String },
}

/// Which tenants the principal may access.
#[derive(Debug, Clone)]
pub enum TenantScope {
    SingleTenant(TenantId),
    Platform,
}

/// How the principal was authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    ApiKey,
    Oidc,
}

impl Principal {
    /// Build an anonymous principal for `auth_mode=none`.
    ///
    /// Granted `Admin` role (tenant-scoped), not `PlatformOperator`.
    pub fn anonymous(tenant: TenantId) -> Self {
        let roles: BTreeSet<Role> = [Role::Admin].into();
        let capabilities = roles.iter().flat_map(|r| r.capabilities()).collect();
        let rate_limit_key = format!("anon:{}", tenant.as_str());
        Self {
            subject: Subject::Anonymous,
            tenant_scope: TenantScope::SingleTenant(tenant),
            roles,
            capabilities,
            auth_method: AuthMethod::None,
            rate_limit_key,
        }
    }

    /// Build a principal from an authenticated API key.
    pub fn from_api_key(
        key_id: Uuid,
        service_account_id: Uuid,
        key_prefix: &str,
        tenant: TenantId,
        role: Role,
    ) -> Self {
        let roles: BTreeSet<Role> = [role].into();
        let capabilities = roles.iter().flat_map(|r| r.capabilities()).collect();
        let rate_limit_key = format!("sa:{}:{}", service_account_id, tenant.as_str());
        Self {
            subject: Subject::ApiKey {
                key_id,
                service_account_id,
                key_prefix: key_prefix.to_string(),
            },
            tenant_scope: TenantScope::SingleTenant(tenant),
            roles,
            capabilities,
            auth_method: AuthMethod::ApiKey,
            rate_limit_key,
        }
    }

    /// Build a principal from a validated OIDC token.
    pub fn from_oidc(
        issuer: &str,
        subject: &str,
        tenant: TenantId,
        role: Role,
        is_platform: bool,
    ) -> Self {
        let roles: BTreeSet<Role> = [role].into();
        let capabilities = roles.iter().flat_map(|r| r.capabilities()).collect();
        let rate_limit_key = format!("oidc:{}:{}", subject, tenant.as_str());
        let tenant_scope =
            if is_platform { TenantScope::Platform } else { TenantScope::SingleTenant(tenant) };
        Self {
            subject: Subject::Oidc { issuer: issuer.to_string(), subject: subject.to_string() },
            tenant_scope,
            roles,
            capabilities,
            auth_method: AuthMethod::Oidc,
            rate_limit_key,
        }
    }

    /// Check whether this principal has a specific capability.
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// Check whether this principal may act within the given tenant.
    pub fn has_tenant_access(&self, tenant: &TenantId) -> bool {
        match &self.tenant_scope {
            TenantScope::SingleTenant(t) => t == tenant,
            TenantScope::Platform => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods)]
    fn test_tenant() -> TenantId {
        TenantId::new("test-tenant").expect("valid tenant")
    }

    #[test]
    fn anonymous_principal_has_admin_role() {
        let p = Principal::anonymous(test_tenant());
        assert!(p.roles.contains(&Role::Admin));
        assert!(!p.roles.contains(&Role::PlatformOperator));
        assert_eq!(p.auth_method, AuthMethod::None);
    }

    #[test]
    fn anonymous_principal_has_admin_capabilities() {
        let p = Principal::anonymous(test_tenant());
        assert!(p.has_capability(Capability::IngestWrite));
        assert!(p.has_capability(Capability::AuthKeysManage));
        assert!(!p.has_capability(Capability::PlatformAdmin));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn anonymous_tenant_access() {
        let tenant = test_tenant();
        let p = Principal::anonymous(tenant.clone());
        assert!(p.has_tenant_access(&tenant));
        let other = TenantId::new("other").expect("valid");
        assert!(!p.has_tenant_access(&other));
    }

    #[test]
    fn api_key_principal_viewer() {
        let p = Principal::from_api_key(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "abcd1234",
            test_tenant(),
            Role::Viewer,
        );
        assert!(p.has_capability(Capability::SearchRead));
        assert!(!p.has_capability(Capability::IngestWrite));
        assert_eq!(p.auth_method, AuthMethod::ApiKey);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn oidc_platform_principal_accesses_any_tenant() {
        let p = Principal::from_oidc(
            "https://auth.example.com",
            "user-123",
            test_tenant(),
            Role::PlatformOperator,
            true,
        );
        assert!(p.has_tenant_access(&test_tenant()));
        let other = TenantId::new("other").expect("valid");
        assert!(p.has_tenant_access(&other));
        assert!(p.has_capability(Capability::PlatformAdmin));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn oidc_tenant_scoped_principal() {
        let p = Principal::from_oidc(
            "https://auth.example.com",
            "user-456",
            test_tenant(),
            Role::Editor,
            false,
        );
        assert!(p.has_tenant_access(&test_tenant()));
        let other = TenantId::new("other").expect("valid");
        assert!(!p.has_tenant_access(&other));
    }

    #[test]
    fn rate_limit_key_formats() {
        let tenant = test_tenant();
        let anon = Principal::anonymous(tenant.clone());
        assert!(anon.rate_limit_key.starts_with("anon:"));

        let sa_id = Uuid::new_v4();
        let api =
            Principal::from_api_key(Uuid::new_v4(), sa_id, "pref", tenant.clone(), Role::Editor);
        assert!(api.rate_limit_key.starts_with("sa:"));
        assert!(api.rate_limit_key.contains(&sa_id.to_string()));

        let oidc = Principal::from_oidc("iss", "sub-1", tenant, Role::Viewer, false);
        assert!(oidc.rate_limit_key.starts_with("oidc:sub-1:"));
    }
}
