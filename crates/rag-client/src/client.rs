//! HTTP client with automatic tenant header injection.

use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ClientError;
use crate::tenant_id::TenantId;

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Tenant header name.
const TENANT_HEADER: &str = "x-tenant";

/// HTTP client that injects an `x-tenant` header on every request.
#[derive(Debug)]
pub struct TenantApiClient {
    client: reqwest::Client,
    base_url: Url,
    tenant: TenantId,
}

impl TenantApiClient {
    /// Create a new client with the default 60-second timeout.
    pub fn new(base_url: &str, tenant: &str) -> Result<Self, ClientError> {
        Self::with_timeout(base_url, tenant, DEFAULT_TIMEOUT)
    }

    /// Create a new client with a custom timeout.
    pub fn with_timeout(
        base_url: &str,
        tenant: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        // Ensure trailing slash so Url::join treats paths as relative to the base.
        let normalized = if base_url.ends_with('/') {
            base_url.to_string()
        } else {
            format!("{base_url}/")
        };
        let url = Url::parse(&normalized)
            .map_err(|e| ClientError::InvalidBaseUrl(format!("{base_url}: {e}")))?;

        let tenant_id = TenantId::new(tenant).map_err(ClientError::InvalidTenant)?;

        let client =
            reqwest::Client::builder().timeout(timeout).build().map_err(ClientError::Transport)?;

        Ok(Self { client, base_url: url, tenant: tenant_id })
    }

    /// Build a request with the tenant header and resolved URL.
    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, ClientError> {
        // Strip leading slash so Url::join preserves the base path prefix
        // (e.g., base "https://host/rag/" + "health" → "https://host/rag/health")
        let relative = path.strip_prefix('/').unwrap_or(path);
        let url = self
            .base_url
            .join(relative)
            .map_err(|e| ClientError::InvalidBaseUrl(format!("cannot join path '{path}': {e}")))?;

        Ok(self.client.request(method, url).header(TENANT_HEADER, self.tenant.as_str()))
    }

    /// Check the response status and deserialize the body as JSON.
    async fn into_success<T: DeserializeOwned>(response: Response) -> Result<T, ClientError> {
        let status = response.status();
        let url = response.url().to_string();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::HttpStatus { status: status.as_u16(), url, body });
        }

        response.json::<T>().await.map_err(ClientError::Decode)
    }

    /// Send a GET request and deserialize the JSON response.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let response = self.request(Method::GET, path)?.send().await?;
        Self::into_success(response).await
    }

    /// Send a GET request and return the raw response (2xx only).
    pub async fn get_response(&self, path: &str) -> Result<Response, ClientError> {
        let response = self.request(Method::GET, path)?.send().await?;
        let status = response.status();
        if !status.is_success() {
            let url = response.url().to_string();
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::HttpStatus { status: status.as_u16(), url, body });
        }
        Ok(response)
    }

    /// Send a POST request with a JSON body and deserialize the response.
    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let response = self.request(Method::POST, path)?.json(body).send().await?;
        Self::into_success(response).await
    }

    /// Send a POST request with a per-request timeout override.
    pub async fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, ClientError> {
        let response = self.request(Method::POST, path)?.json(body).timeout(timeout).send().await?;
        Self::into_success(response).await
    }

    // ── Typed convenience methods ───────────────────────────────────────────

    /// Check server liveness. Returns `Ok(())` if the server responds with "ok".
    pub async fn health(&self) -> Result<(), ClientError> {
        let response = self.get_response("/health").await?;
        let body = response.text().await.map_err(ClientError::Decode)?;
        if body.trim() == "ok" {
            Ok(())
        } else {
            Err(ClientError::Validation(format!("unexpected health response: {body}")))
        }
    }

    /// Check server readiness (Postgres + Qdrant).
    ///
    /// Always attempts to parse the JSON body, even on 503, because the server
    /// returns a structured `ReadinessResponse` with `ready: false` when unhealthy.
    pub async fn readiness(&self) -> Result<crate::types::ReadinessResponse, ClientError> {
        let response = self.request(Method::GET, "/readiness")?.send().await?;
        response.json::<crate::types::ReadinessResponse>().await.map_err(ClientError::Decode)
    }

    /// Ingest files/directories. Uses a 300-second per-request timeout.
    pub async fn ingest(
        &self,
        req: &crate::types::IngestRequest,
    ) -> Result<crate::types::IngestResponse, ClientError> {
        self.post_json_with_timeout("/ingest", req, Duration::from_secs(300)).await
    }

    /// Dense vector search.
    pub async fn search_dense(
        &self,
        req: &crate::types::SearchRequest,
    ) -> Result<crate::types::SearchResponse, ClientError> {
        self.post_json("/search/dense", req).await
    }

    /// Sparse (BM25) search.
    pub async fn search_sparse(
        &self,
        req: &crate::types::SearchRequest,
    ) -> Result<crate::types::SearchResponse, ClientError> {
        self.post_json("/search/sparse", req).await
    }

    /// Hybrid search (dense + sparse with RRF fusion).
    pub async fn search_hybrid(
        &self,
        req: &crate::types::HybridSearchRequest,
    ) -> Result<crate::types::HybridSearchResponse, ClientError> {
        self.post_json("/search/hybrid", req).await
    }

    /// RAG chat.
    pub async fn chat(
        &self,
        req: &crate::types::ChatRequest,
    ) -> Result<crate::types::ChatResponse, ClientError> {
        self.post_json("/chat", req).await
    }

    /// Fetch collection statistics. Percent-encodes the collection name.
    pub async fn collection_stats(
        &self,
        collection: &str,
    ) -> Result<crate::types::CollectionStatsResponse, ClientError> {
        if collection.is_empty() {
            return Err(ClientError::Validation("collection must not be empty".into()));
        }
        let encoded =
            percent_encoding::utf8_percent_encode(collection, percent_encoding::NON_ALPHANUMERIC);
        let path = format!("/collections/{encoded}/stats");
        self.get_json(&path).await
    }
}

/// Delegates every `ApiClient` trait method to the identically-named inherent method.
/// Rust resolves `self.health()` to the inherent method (not the trait method), so there
/// is no infinite recursion.
macro_rules! impl_api_client {
    ($ty:ty) => {
        impl crate::trait_def::ApiClient for $ty {
            async fn health(&self) -> Result<(), ClientError> {
                self.health().await
            }
            async fn readiness(&self) -> Result<crate::types::ReadinessResponse, ClientError> {
                self.readiness().await
            }
            async fn ingest(
                &self,
                req: &crate::types::IngestRequest,
            ) -> Result<crate::types::IngestResponse, ClientError> {
                self.ingest(req).await
            }
            async fn search_dense(
                &self,
                req: &crate::types::SearchRequest,
            ) -> Result<crate::types::SearchResponse, ClientError> {
                self.search_dense(req).await
            }
            async fn search_sparse(
                &self,
                req: &crate::types::SearchRequest,
            ) -> Result<crate::types::SearchResponse, ClientError> {
                self.search_sparse(req).await
            }
            async fn search_hybrid(
                &self,
                req: &crate::types::HybridSearchRequest,
            ) -> Result<crate::types::HybridSearchResponse, ClientError> {
                self.search_hybrid(req).await
            }
            async fn chat(
                &self,
                req: &crate::types::ChatRequest,
            ) -> Result<crate::types::ChatResponse, ClientError> {
                self.chat(req).await
            }
            async fn collection_stats(
                &self,
                collection: &str,
            ) -> Result<crate::types::CollectionStatsResponse, ClientError> {
                self.collection_stats(collection).await
            }
        }
    };
}

impl_api_client!(TenantApiClient);

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn request_preserves_base_path_prefix() {
        let client = TenantApiClient::new("http://host/rag/v1", "default").unwrap();
        let req = client.request(Method::GET, "/health").unwrap();
        let url = req.build().unwrap().url().to_string();
        assert_eq!(url, "http://host/rag/v1/health");
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn request_works_without_path_prefix() {
        let client = TenantApiClient::new("http://host", "default").unwrap();
        let req = client.request(Method::GET, "/health").unwrap();
        let url = req.build().unwrap().url().to_string();
        assert_eq!(url, "http://host/health");
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn base_url_trailing_slash_normalized() {
        let with_slash = TenantApiClient::new("http://host/api/", "default").unwrap();
        let without_slash = TenantApiClient::new("http://host/api", "default").unwrap();
        assert_eq!(with_slash.base_url, without_slash.base_url);
    }
}
