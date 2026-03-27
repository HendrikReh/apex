//! Shared test helpers for workspace crates.
//!
//! Centralizes common integration-test primitives (ephemeral Axum server
//! startup) so crates can reuse the same setup logic.

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Ephemeral HTTP server helper for integration-style tests.
pub struct TestServer {
    addr: SocketAddr,
    handle: JoinHandle<()>,
}

impl TestServer {
    /// Return the bound socket address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Return the base URL (e.g., `http://127.0.0.1:12345`).
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Spawn an Axum app on an ephemeral local port.
pub async fn spawn_app(app: Router) -> std::io::Result<TestServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server = axum::serve(listener, app.into_make_service());
    let handle = tokio::spawn(async move {
        let _ = server.await;
    });
    Ok(TestServer { addr, handle })
}
