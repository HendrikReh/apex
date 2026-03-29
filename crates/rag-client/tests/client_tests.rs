#![allow(clippy::disallowed_methods)] // Tests use .expect() and .unwrap()

use rag_client::{ClientError, TenantApiClient};

#[test]
fn invalid_base_url_rejected() {
    let err = TenantApiClient::new("not a url", "default").unwrap_err();
    assert!(matches!(err, ClientError::InvalidBaseUrl(_)));
}

#[test]
fn invalid_tenant_rejected() {
    let err = TenantApiClient::new("http://localhost:8080", "").unwrap_err();
    assert!(matches!(err, ClientError::InvalidTenant(_)));

    let err = TenantApiClient::new("http://localhost:8080", "a/b").unwrap_err();
    assert!(matches!(err, ClientError::InvalidTenant(_)));
}

#[test]
fn valid_construction() {
    let client = TenantApiClient::new("http://localhost:8080", "my-tenant");
    assert!(client.is_ok());
}
