//! Sidecar metadata schema: types, parsing, and validation.
//!
//! A sidecar JSON file accompanies each ingested document and carries
//! metadata such as document info, source provenance, access control,
//! security classification, and optional ingestion overrides.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Top-level sidecar envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct Sidecar {
    pub schema_version: u32,
    pub document: DocumentInfo,
    pub source: SourceInfo,
    pub language: String,
    pub tags: Vec<String>,
    pub acl: AclInfo,
    pub security: SecurityInfo,
    pub provenance: ProvenanceInfo,
    #[serde(default)]
    pub ingestion: Option<IngestionConfig>,
}

/// Core document metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct DocumentInfo {
    pub id: Option<String>,
    pub title: String,
    pub category: String,
    pub version: Option<String>,
    pub authors: Option<Vec<String>>,
    pub summary: Option<String>,
    pub published_at: Option<String>,
    pub refresh_cadence: Option<String>,
    pub content_owner: Option<String>,
    pub last_updated: Option<String>,
}

/// Where the document was obtained.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceInfo {
    pub url: String,
    pub domain: String,
    pub publisher: String,
    pub collection: Option<String>,
}

/// Access-control list.
#[derive(Debug, Clone, Deserialize)]
pub struct AclInfo {
    pub allow_roles: Vec<String>,
    pub deny_roles: Option<Vec<String>>,
    pub notes: Option<String>,
}

/// Security classification and related flags.
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityInfo {
    pub classification: String,
    pub requires_evidence_pack: bool,
    pub compartments: Option<Vec<String>>,
    pub contains_pii: Option<bool>,
    pub export_control: Option<String>,
}

/// How and when the document was retrieved.
#[derive(Debug, Clone, Deserialize)]
pub struct ProvenanceInfo {
    pub retrieved_at: String,
    pub retrieved_by: String,
    pub checksum_sha256: Option<String>,
}

/// Optional ingestion overrides carried in the sidecar.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestionConfig {
    pub collection: Option<String>,
    pub chunking: Option<ChunkingOverride>,
}

/// Per-document chunking parameter overrides.
#[derive(Debug, Clone, Deserialize)]
pub struct ChunkingOverride {
    pub strategy: Option<String>,
    pub max_tokens: Option<usize>,
    pub overlap_ratio: Option<f32>,
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

impl Sidecar {
    /// Deserialise from raw JSON bytes, then validate.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let sidecar: Self =
            serde_json::from_slice(bytes).context("failed to parse sidecar JSON")?;
        sidecar.validate()?;
        Ok(sidecar)
    }

    /// Validate business-rule invariants.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!(
                "unsupported sidecar schema_version: {} (expected 1)",
                self.schema_version
            );
        }

        if self.document.title.is_empty() {
            bail!("document.title must not be empty");
        }
        if self.document.category.is_empty() {
            bail!("document.category must not be empty");
        }
        if self.language.is_empty() {
            bail!("language must not be empty");
        }

        if self.source.url.is_empty() {
            bail!("source.url must not be empty");
        }
        if self.source.domain.is_empty() {
            bail!("source.domain must not be empty");
        }
        if self.source.publisher.is_empty() {
            bail!("source.publisher must not be empty");
        }

        if self.tags.is_empty() {
            bail!("tags must contain at least one entry");
        }

        if self.acl.allow_roles.is_empty() {
            bail!("acl.allow_roles must contain at least one entry");
        }

        if self.security.classification.is_empty() {
            bail!("security.classification must not be empty");
        }

        if self.provenance.retrieved_at.is_empty() {
            bail!("provenance.retrieved_at must not be empty");
        }
        if self.provenance.retrieved_by.is_empty() {
            bail!("provenance.retrieved_by must not be empty");
        }

        if let Some(ref ing) = self.ingestion {
            if let Some(ref chunking) = ing.chunking {
                if let Some(max) = chunking.max_tokens {
                    if max == 0 {
                        bail!("ingestion.chunking.max_tokens must be > 0");
                    }
                }
                if let Some(ratio) = chunking.overlap_ratio {
                    if !(0.0..1.0).contains(&ratio) {
                        bail!(
                            "ingestion.chunking.overlap_ratio must be in [0.0, 1.0), got {}",
                            ratio
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_sidecar_json() -> &'static str {
        r#"{
            "schema_version": 1,
            "document": {
                "title": "Test Document",
                "category": "report"
            },
            "source": {
                "url": "https://example.com/doc.pdf",
                "domain": "example.com",
                "publisher": "Example Inc"
            },
            "language": "en",
            "tags": ["test"],
            "acl": { "allow_roles": ["*"] },
            "security": {
                "classification": "public",
                "requires_evidence_pack": false
            },
            "provenance": {
                "retrieved_at": "2026-03-27T10:00:00Z",
                "retrieved_by": "manual"
            }
        }"#
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn parses_valid_minimal_sidecar() {
        let sc = Sidecar::from_json(valid_sidecar_json().as_bytes())
            .expect("valid sidecar should parse");
        assert_eq!(sc.document.title, "Test Document");
        assert_eq!(sc.document.category, "report");
        assert_eq!(sc.language, "en");
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let json = valid_sidecar_json().replace("\"schema_version\": 1", "\"schema_version\": 2");
        let err = Sidecar::from_json(json.as_bytes())
            .expect_err("schema_version 2 should be rejected");
        assert!(
            err.to_string().contains("unsupported sidecar schema_version"),
            "error should mention schema_version, got: {err}"
        );
    }

    #[test]
    fn rejects_empty_title() {
        let json = valid_sidecar_json().replace("\"Test Document\"", "\"\"");
        let err = Sidecar::from_json(json.as_bytes())
            .expect_err("empty title should be rejected");
        assert!(
            err.to_string().contains("document.title must not be empty"),
            "error should mention title, got: {err}"
        );
    }

    #[test]
    fn rejects_empty_tags() {
        let json = valid_sidecar_json().replace("[\"test\"]", "[]");
        let err =
            Sidecar::from_json(json.as_bytes()).expect_err("empty tags should be rejected");
        assert!(
            err.to_string().contains("tags must contain at least one entry"),
            "error should mention tags, got: {err}"
        );
    }

    #[test]
    fn rejects_invalid_overlap_ratio() {
        let json = r#"{
            "schema_version": 1,
            "document": { "title": "T", "category": "c" },
            "source": { "url": "u", "domain": "d", "publisher": "p" },
            "language": "en",
            "tags": ["t"],
            "acl": { "allow_roles": ["*"] },
            "security": { "classification": "public", "requires_evidence_pack": false },
            "provenance": { "retrieved_at": "now", "retrieved_by": "me" },
            "ingestion": {
                "chunking": { "overlap_ratio": 1.5 }
            }
        }"#;
        let err = Sidecar::from_json(json.as_bytes())
            .expect_err("overlap_ratio 1.5 should be rejected");
        assert!(
            err.to_string().contains("overlap_ratio must be in"),
            "error should mention overlap_ratio, got: {err}"
        );
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn parses_sidecar_with_ingestion_overrides() {
        let json = r#"{
            "schema_version": 1,
            "document": { "title": "T", "category": "c" },
            "source": { "url": "u", "domain": "d", "publisher": "p" },
            "language": "en",
            "tags": ["t"],
            "acl": { "allow_roles": ["*"] },
            "security": { "classification": "public", "requires_evidence_pack": false },
            "provenance": { "retrieved_at": "now", "retrieved_by": "me" },
            "ingestion": {
                "collection": "my-collection",
                "chunking": {
                    "strategy": "sliding_window",
                    "max_tokens": 512,
                    "overlap_ratio": 0.15
                }
            }
        }"#;
        let sc = Sidecar::from_json(json.as_bytes()).expect("should parse with ingestion");
        let ing = sc.ingestion.expect("ingestion should be Some");
        assert_eq!(ing.collection.as_deref(), Some("my-collection"));
        let ch = ing.chunking.expect("chunking should be Some");
        assert_eq!(ch.strategy.as_deref(), Some("sliding_window"));
        assert_eq!(ch.max_tokens, Some(512));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn ignores_unknown_fields() {
        let json = valid_sidecar_json().replace(
            "\"schema_version\": 1",
            "\"future_field\": true, \"schema_version\": 1",
        );
        let sc = Sidecar::from_json(json.as_bytes())
            .expect("unknown fields should be silently ignored");
        assert_eq!(sc.schema_version, 1);
    }
}
