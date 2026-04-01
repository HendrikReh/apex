//! Role and capability definitions for capability-based RBAC.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Named roles that bundle capabilities.
///
/// Roles are ordered by privilege level for convenience helpers, but the
/// primary authorization model uses capabilities, not role comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Editor,
    AgentOperator,
    Admin,
    PlatformOperator,
}

/// Named capabilities — the primary authorization primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "health.read")]
    HealthRead,
    #[serde(rename = "collections.read")]
    CollectionsRead,
    #[serde(rename = "search.read")]
    SearchRead,
    #[serde(rename = "chat.use")]
    ChatUse,
    #[serde(rename = "agents.operate")]
    AgentsOperate,
    #[serde(rename = "ingest.write")]
    IngestWrite,
    #[serde(rename = "auth.keys.manage")]
    AuthKeysManage,
    #[serde(rename = "platform.admin")]
    PlatformAdmin,
}

impl Role {
    /// Expand a role into its granted capabilities.
    pub fn capabilities(self) -> BTreeSet<Capability> {
        use Capability::*;
        let caps: &[Capability] = match self {
            Self::Viewer => &[HealthRead, CollectionsRead, SearchRead, ChatUse],
            Self::Editor => &[HealthRead, CollectionsRead, SearchRead, ChatUse, IngestWrite],
            Self::AgentOperator => {
                &[HealthRead, CollectionsRead, SearchRead, ChatUse, AgentsOperate, IngestWrite]
            }
            Self::Admin => &[
                HealthRead,
                CollectionsRead,
                SearchRead,
                ChatUse,
                AgentsOperate,
                IngestWrite,
                AuthKeysManage,
            ],
            Self::PlatformOperator => &[
                HealthRead,
                CollectionsRead,
                SearchRead,
                ChatUse,
                AgentsOperate,
                IngestWrite,
                AuthKeysManage,
                PlatformAdmin,
            ],
        };
        caps.iter().copied().collect()
    }
}

impl std::str::FromStr for Role {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "viewer" => Ok(Self::Viewer),
            "editor" => Ok(Self::Editor),
            "agent_operator" => Ok(Self::AgentOperator),
            "admin" => Ok(Self::Admin),
            "platform_operator" => Ok(Self::PlatformOperator),
            other => Err(anyhow::anyhow!("unknown role: {other:?}")),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Viewer => f.write_str("viewer"),
            Self::Editor => f.write_str("editor"),
            Self::AgentOperator => f.write_str("agent_operator"),
            Self::Admin => f.write_str("admin"),
            Self::PlatformOperator => f.write_str("platform_operator"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_has_read_capabilities_only() {
        let caps = Role::Viewer.capabilities();
        assert!(caps.contains(&Capability::HealthRead));
        assert!(caps.contains(&Capability::CollectionsRead));
        assert!(caps.contains(&Capability::SearchRead));
        assert!(caps.contains(&Capability::ChatUse));
        assert!(!caps.contains(&Capability::AgentsOperate));
        assert!(!caps.contains(&Capability::IngestWrite));
        assert!(!caps.contains(&Capability::AuthKeysManage));
        assert!(!caps.contains(&Capability::PlatformAdmin));
    }

    #[test]
    fn editor_adds_ingest_write() {
        let caps = Role::Editor.capabilities();
        assert!(caps.contains(&Capability::IngestWrite));
        assert!(!caps.contains(&Capability::AgentsOperate));
        assert!(!caps.contains(&Capability::AuthKeysManage));
    }

    #[test]
    fn agent_operator_adds_agent_operations() {
        let caps = Role::AgentOperator.capabilities();
        assert!(caps.contains(&Capability::AgentsOperate));
        assert!(caps.contains(&Capability::IngestWrite));
        assert!(!caps.contains(&Capability::AuthKeysManage));
    }

    #[test]
    fn admin_includes_key_management() {
        let caps = Role::Admin.capabilities();
        assert!(caps.contains(&Capability::IngestWrite));
        assert!(caps.contains(&Capability::AuthKeysManage));
        assert!(!caps.contains(&Capability::PlatformAdmin));
    }

    #[test]
    fn platform_operator_has_all_capabilities() {
        let caps = Role::PlatformOperator.capabilities();
        assert_eq!(caps.len(), 8, "platform_operator should have all 8 capabilities");
        assert!(caps.contains(&Capability::PlatformAdmin));
    }

    #[test]
    fn editor_is_superset_of_viewer() {
        let viewer = Role::Viewer.capabilities();
        let editor = Role::Editor.capabilities();
        assert!(viewer.is_subset(&editor));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn role_roundtrip_parsing() {
        for role in
            [Role::Viewer, Role::Editor, Role::AgentOperator, Role::Admin, Role::PlatformOperator]
        {
            let s = role.to_string();
            let parsed: Role = s.parse().expect("should parse");
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn role_parsing_is_case_insensitive() {
        assert_eq!("ADMIN".parse::<Role>().ok(), Some(Role::Admin));
        assert_eq!("Platform_Operator".parse::<Role>().ok(), Some(Role::PlatformOperator));
    }

    #[test]
    fn unknown_role_returns_error() {
        assert!("superadmin".parse::<Role>().is_err());
    }
}
