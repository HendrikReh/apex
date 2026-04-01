//! Error types for the RAG client.

/// Errors returned by `TenantApiClient` operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The provided base URL could not be parsed.
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),

    /// The provided tenant identifier failed validation.
    #[error("invalid tenant: {0}")]
    InvalidTenant(String),

    /// A network-level failure (connection refused, timeout, DNS, etc.).
    #[error("transport error: {0}")]
    Transport(reqwest::Error),

    /// The server returned a non-2xx status code.
    #[error("server returned {status}: {body}")]
    HttpStatus { status: u16, url: String, body: String },

    /// The server returned a non-2xx status code, but the response body could
    /// not be read.
    #[error("server returned {status} and the error body could not be read from {url}: {source}")]
    HttpStatusBodyRead {
        status: u16,
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// The response body could not be decoded (invalid JSON, etc.).
    #[error("failed to decode response: {0}")]
    Decode(reqwest::Error),

    /// A client-side validation check failed before sending the request.
    #[error("{0}")]
    Validation(String),
}

impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        ClientError::Transport(err)
    }
}
